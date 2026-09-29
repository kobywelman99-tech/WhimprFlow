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
    private(set) var hasAPIKey: Bool

    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let keychain: KeychainStore

    init(defaults: UserDefaults = .standard, keychain: KeychainStore = KeychainStore()) {
        self.defaults = defaults
        self.keychain = keychain
        enabled = defaults.object(forKey: "cleanupEnabled") as? Bool ?? true
        level = (defaults.string(forKey: "cleanupLevel")).flatMap(Self.level(named:)) ?? defaultLevel()
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
    private let makeModel: (String) -> any CleanupModel

    init(
        settings: CleanupSettings,
        vocabulary: @escaping (String) -> [Vocab] = { _ in [] },
        makeModel: @escaping (String) -> any CleanupModel = { AnthropicClient(apiKey: $0) }
    ) {
        self.settings = settings
        self.vocabulary = vocabulary
        self.makeModel = makeModel
    }

    /// `level` overrides the default for one dictation (the keyboard's pill).
    func clean(_ raw: String, level override: Level? = nil) async -> CleanupResult {
        guard settings.enabled else { return CleanupResult(text: raw, cleaned: false) }
        let level = override ?? settings.level
        let request = prepareCleanup(raw: raw, level: level, vocab: vocabulary(raw), appBundleId: nil, styleSummary: nil)
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
