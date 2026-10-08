import Foundation
import Observation
import os

private let log = Logger(subsystem: "com.whimpr.whimprflow", category: "cleanup")

/// Anything that turns a prepared cleanup into model text. The app uses
/// AnthropicClient; tests use a fake.
protocol CleanupModel: Sendable {
    func clean(_ request: CleanupRequest) async throws -> String
}

extension AnthropicClient: CleanupModel {}

/// Cleanup preferences, kept in the app's own defaults. The API key is in the
/// Keychain, never here.
@MainActor
@Observable
final class CleanupSettings {
    static let apiKeyAccount = "anthropic_api_key"

    var enabled: Bool {
        didSet { defaults.set(enabled, forKey: "cleanupEnabled") }
    }
    var level: Level {
        didSet { defaults.set(Self.name(of: level), forKey: "cleanupLevel") }
    }
    /// Re-read each dictation in its conversation and fix words that don't make sense.
    var reviewEnabled: Bool {
        didSet { defaults.set(reviewEnabled, forKey: "reviewEnabled") }
    }
    private(set) var hasAPIKey: Bool

    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let keychain: KeychainStore

    init(defaults: UserDefaults = .standard, keychain: KeychainStore = KeychainStore()) {
        self.defaults = defaults
        self.keychain = keychain
        enabled = defaults.object(forKey: "cleanupEnabled") as? Bool ?? true
        level = (defaults.string(forKey: "cleanupLevel")).flatMap(Self.level(named:)) ?? defaultLevel()
        reviewEnabled = defaults.object(forKey: "reviewEnabled") as? Bool ?? true
        hasAPIKey = keychain.read(Self.apiKeyAccount) != nil
    }

    var apiKey: String? { keychain.read(Self.apiKeyAccount) }

    func saveAPIKey(_ key: String) throws {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        try keychain.save(trimmed, for: Self.apiKeyAccount)
        hasAPIKey = true
    }

    func removeAPIKey() throws {
        try keychain.delete(Self.apiKeyAccount)
        hasAPIKey = false
    }

    private static func name(of level: Level) -> String {
        switch level {
        case .off: "off"
        case .light: "light"
        case .medium: "medium"
        case .high: "high"
        }
    }

    private static func level(named name: String) -> Level? {
        ["off": .off, "light": .light, "medium": .medium, "high": .high][name]
    }
}

extension Level {
    init(_ level: DictationLevel) {
        self = switch level {
        case .off: .off
        case .light: .light
        case .medium: .medium
        case .high: .high
        }
    }
}

extension DictationLevel {
    init(_ level: Level) {
        self = switch level {
        case .off: .off
        case .light: .light
        case .medium: .medium
        case .high: .high
        }
    }
}

struct CleanupResult: Equatable {
    var text: String
    /// True when the model's text was used, false for the raw transcript.
    var cleaned: Bool
    /// Why cleanup didn't happen, when that's worth telling the user.
    var note: String?
    /// Words the sense check changed because they didn't fit the conversation.
    var fixes: [ReviewChange] = []
}

/// Raw transcript → cleaned text, the way the desktop does it
/// (src-tauri/src/hotkey.rs, clean_transcript): Rust prepares the prompt,
/// the model answers, Rust post-processes and gates. Any failure falls back to
/// the raw words, so dictation never gets stuck on cleanup.
@MainActor
final class CleanupPipeline {
    private let settings: CleanupSettings
    /// Dictionary entries relevant to an utterance, for the prompt.
    private let vocabulary: (String) -> [Vocab]
    /// The learned description of how the user writes, if there's one yet.
    private let styleSummary: () -> String?
    private let makeModel: (String) -> any CleanupModel
    /// Makes the model for the sense check (a shorter timeout than cleanup: it's
    /// a bonus, never worth keeping the keyboard waiting). Nil turns the check off.
    private let makeReviewModel: ((String) -> any CleanupModel)?
    /// Known mishears → right spellings, from voice calibration and the
    /// dictionary. Runs before everything else, so it works at every level.
    private let replacements: (String) -> String

    init(
        settings: CleanupSettings,
        vocabulary: @escaping (String) -> [Vocab] = { _ in [] },
        replacements: @escaping (String) -> String = { $0 },
        styleSummary: @escaping () -> String? = { nil },
        makeModel: @escaping (String) -> any CleanupModel = { AnthropicClient(apiKey: $0) },
        makeReviewModel: ((String) -> any CleanupModel)? = nil
    ) {
        self.settings = settings
        self.vocabulary = vocabulary
        self.replacements = replacements
        self.styleSummary = styleSummary
        self.makeModel = makeModel
        self.makeReviewModel = makeReviewModel
    }

    /// The level a dictation with this override is cleaned at; nil when
    /// cleanup is turned off.
    func level(for override: Level?) -> Level? {
        settings.enabled ? (override ?? settings.level) : nil
    }

    /// `level` overrides the default for one dictation (the keyboard's pill).
    /// `context` is the text already in the box, for the sense check.
    func clean(_ heard: String, level override: Level? = nil, context: String? = nil) async -> CleanupResult {
        let raw = replacements(heard)
        guard settings.enabled else { return CleanupResult(text: raw, cleaned: false) }
        let level = override ?? settings.level
        let cleaned = await cleanup(raw, level: level)
        // Raw means exactly what was said, so it isn't second-guessed.
        guard level != .off, settings.reviewEnabled, makeReviewModel != nil else { return cleaned }
        return await review(cleaned, heard: raw, context: context)
    }

    /// The last read: does the message make sense where it is going? Anything that
    /// goes wrong leaves the message as it was.
    private func review(_ result: CleanupResult, heard: String, context: String?) async -> CleanupResult {
        guard let key = settings.apiKey, let makeReviewModel else { return result }
        let request = prepareReview(heard: heard, draft: result.text, context: context, vocab: vocabulary(heard))
        if request.skipModel { return result }
        let started = ContinuousClock.now
        do {
            let output = try await makeReviewModel(key).clean(request)
            let outcome = finishReview(modelOutput: output, draft: result.text, heard: heard)
            let elapsed = ContinuousClock.now - started
            if let rejection = outcome.rejection {
                log.info("Sense check fix refused (\(rejection, privacy: .public)) in \(elapsed, privacy: .public)")
            }
            guard outcome.changed else {
                log.info("Sense check: fine as written (\(elapsed, privacy: .public))")
                return result
            }
            log.info("Sense check fixed \(outcome.changes.count) word(s) in \(elapsed, privacy: .public)")
            var fixed = result
            fixed.text = outcome.text
            fixed.fixes = outcome.changes
            return fixed
        } catch {
            log.error("Sense check failed: \(error.localizedDescription, privacy: .public); keeping the message")
            return result
        }
    }

    private func cleanup(_ raw: String, level: Level) async -> CleanupResult {
        let request = prepareCleanup(raw: raw, level: level, vocab: vocabulary(raw), appBundleId: nil, styleSummary: styleSummary())
        if request.skipModel {
            return CleanupResult(text: request.fallback, cleaned: false)
        }
        guard let key = settings.apiKey else {
            return CleanupResult(text: request.fallback, cleaned: false, note: "Add your Anthropic API key in Settings to clean up dictation.")
        }

        let started = ContinuousClock.now
        do {
            let output = try await makeModel(key).clean(request)
            let outcome = finishCleanup(modelOutput: output, fallback: request.fallback, level: level)
            let elapsed = ContinuousClock.now - started
            if outcome.accepted {
                log.info("Cleanup accepted in \(elapsed, privacy: .public)")
                return CleanupResult(text: outcome.text, cleaned: true)
            }
            log.info("Cleanup rejected by gates (\(outcome.rejection ?? "?", privacy: .public)); using raw")
            return CleanupResult(text: outcome.text, cleaned: false, note: "Cleanup changed too much, so your exact words were kept.")
        } catch {
            log.error("Cleanup failed: \(error.localizedDescription, privacy: .public); using raw")
            return CleanupResult(text: request.fallback, cleaned: false, note: error.localizedDescription)
        }
    }
}
