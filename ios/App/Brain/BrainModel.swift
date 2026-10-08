import Foundation
import Observation

/// What WhimprFlow has learned about how you sound (Rust `VoiceBrain`), for
/// SwiftUI. It learns from calibration and from corrections you make to past
/// dictations; nothing here is a list you maintain.
///
/// It reaches dictation in three places: the speech recogniser leans toward the
/// words it has heard you say, the recogniser's alternative transcriptions are
/// weighed against your sound patterns (`rescore`), and word pairs it has seen
/// repeat are fixed in the text (`correct`).
@MainActor
@Observable
final class BrainModel {
    private(set) var overview: BrainOverview
    @ObservationIgnored private let brain: VoiceBrain

    init(fileURL: URL = BrainModel.defaultFileURL) {
        brain = VoiceBrain.open(path: fileURL.path)
        overview = brain.overview()
    }

    /// Application Support/brain.json.
    static var defaultFileURL: URL {
        URL.applicationSupportDirectory.appending(path: "brain.json")
    }

    struct SaveError: LocalizedError, Equatable {
        let errorDescription: String?
    }

    /// Learns from what you meant against what the recogniser heard.
    func learn(meant: String, heard: String) throws {
        try learn([CalibrationAttempt(expected: meant, heard: heard)])
    }

    func learn(_ attempts: [CalibrationAttempt]) throws {
        do {
            try brain.observeAll(attempts: attempts)
        } catch let error as BrainError {
            switch error {
            case .Io(let message): throw SaveError(errorDescription: "Couldn't save what was learned: \(message)")
            }
        }
        overview = brain.overview()
    }

    /// Picks among the recogniser's candidates (best first). With one candidate,
    /// or nothing learned yet, that is the answer.
    func rescore(_ candidates: [String]) -> String {
        guard candidates.count > 1 else { return candidates.first ?? "" }
        return brain.rescore(candidates: candidates).text
    }

    /// Word pairs the brain has seen repeat, fixed in `text`.
    func correct(_ text: String) -> String {
        brain.correct(text: text)
    }

    /// Spellings the recogniser should lean toward.
    var biasWords: [String] { brain.biasWords(limit: 60) }

    func reset() throws {
        do {
            try brain.reset()
        } catch let error as BrainError {
            switch error {
            case .Io(let message): throw SaveError(errorDescription: "Couldn't save: \(message)")
            }
        }
        overview = brain.overview()
    }
}
