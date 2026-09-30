import Foundation
import Observation
import os

private let log = Logger(subsystem: "com.whimpr.whimprflow", category: "style")

/// Koby's style profile (Rust `StyleProfile`, the desktop's passive learning)
/// for SwiftUI. Learns from every dictation and from imported writing; its
/// one-line summary goes into the cleanup prompt once it has 5 samples.
@MainActor
@Observable
final class StyleModel {
    private(set) var overview: StyleOverview
    var enabled: Bool {
        didSet { defaults.set(enabled, forKey: "styleLearning") }
    }

    @ObservationIgnored private let profile: StyleProfile
    @ObservationIgnored private let defaults: UserDefaults

    // whimpr-core now learns a separate voice per dictation mode (Email /
    // General / Essay-Report). iOS doesn't have a mode picker yet, so this
    // model always reads/writes the General bucket for now — see the iOS FFI
    // follow-up note in the desktop "Dictation Style Modes" plan.
    private let mode: StyleMode = .general

    init(fileURL: URL = StyleModel.defaultFileURL, defaults: UserDefaults = .standard) {
        profile = StyleProfile.open(path: fileURL.path)
        overview = profile.overview(mode: .general)
        self.defaults = defaults
        enabled = defaults.object(forKey: "styleLearning") as? Bool ?? true
    }

    /// Application Support/style.json, the desktop's file name and format.
    static var defaultFileURL: URL {
        URL.applicationSupportDirectory.appending(path: "style.json")
    }

    /// What goes into the cleanup prompt: nothing while learning is off or
    /// there isn't enough yet.
    var promptSummary: String? {
        enabled ? overview.promptSummary : nil
    }

    /// Learns from one dictation's raw words (while learning is on).
    func observe(_ raw: String) {
        guard enabled else { return }
        do {
            try profile.observe(text: raw, mode: mode)
            overview = profile.overview(mode: mode)
        } catch {
            log.error("\(error.localizedDescription, privacy: .public)")
        }
    }

    /// Learns from imported writing, however learning is set. Returns how
    /// many samples were used.
    @discardableResult
    func learn(from samples: [String]) throws -> Int {
        let learned = try readable { try profile.observeAll(samples: samples, mode: mode) }
        overview = profile.overview(mode: mode)
        return Int(learned)
    }

    func reset() throws {
        try readable { try profile.reset(mode: mode) }
        overview = profile.overview(mode: mode)
    }

    struct StyleSaveError: LocalizedError {
        let errorDescription: String?
    }

    /// The generated `StyleError` describes itself by its case name.
    private func readable<T>(_ body: () throws -> T) throws -> T {
        do {
            return try body()
        } catch let error as StyleError {
            switch error {
            case .Io(let message): throw StyleSaveError(errorDescription: "Couldn't save your style: \(message)")
            }
        }
    }
}
