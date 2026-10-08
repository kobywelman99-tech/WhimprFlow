import Foundation
import Observation

/// What a calibration session needs from the microphone: listen, then say what
/// the recogniser heard. DictationService does this in the app; tests use a fake.
@MainActor
protocol CalibrationListening {
    func startListening() async throws
    /// The recogniser's words, untouched.
    func stopListening() async throws -> String
    func cancelListening() async
    /// Called when the session ends, to give back a mic the session opened.
    func close()
}

/// Listens through the app's DictationService, opening the mic session only if
/// Flow hasn't already.
@MainActor
final class CalibrationRecorder: CalibrationListening {
    private let dictation: DictationService
    private var openedSession = false

    init(dictation: DictationService) {
        self.dictation = dictation
    }

    func startListening() async throws {
        if !dictation.sessionActive {
            try await dictation.startSession()
            openedSession = true
        }
        // The brain learns from what the recogniser really produced, so none of it is applied here.
        try await dictation.beginUtterance(useBrain: false)
    }

    func stopListening() async throws -> String {
        try await dictation.finishRawUtterance()
    }

    func cancelListening() async {
        await dictation.cancelUtterance()
    }

    func close() {
        if openedSession { dictation.endSession() }
        openedSession = false
    }
}

/// One calibration session: read some lines aloud, and the brain learns from
/// the gap between what you said and what the recogniser heard.
@MainActor
@Observable
final class CalibrationModel {
    enum Step: Equatable {
        case intro
        case reading
        /// Finished; the brain has learned from this many lines.
        case learned(Int)
    }

    enum Listening: Equatable {
        case idle
        case listening
        case working
    }

    struct Line: Identifiable, Equatable {
        let id = UUID()
        let tag: String
        let text: String
        var heard: String?
    }

    static let defaultLineCount = 10

    private(set) var step: Step = .intro
    private(set) var listening: Listening = .idle
    private(set) var lines: [Line] = []
    private(set) var current = 0
    private(set) var error: String?

    @ObservationIgnored private let recorder: any CalibrationListening
    @ObservationIgnored private let brain: BrainModel
    @ObservationIgnored private let lineCount: Int

    init(recorder: any CalibrationListening, brain: BrainModel, lineCount: Int = CalibrationModel.defaultLineCount) {
        self.recorder = recorder
        self.brain = brain
        self.lineCount = lineCount
    }

    var currentLine: Line? { lines.indices.contains(current) ? lines[current] : nil }
    var isLastLine: Bool { current >= lines.count - 1 }
    /// Lines that have been read aloud so far.
    var spokenCount: Int { lines.filter { $0.heard != nil }.count }

    func begin(seed: UInt64 = .random(in: 0...UInt64.max)) {
        lines = calibrationPrompts(count: UInt32(lineCount), seed: seed)
            .map { Line(tag: $0.tag, text: $0.text) }
        current = 0
        error = nil
        step = .reading
    }

    /// Adds a word or phrase of your own (a name, a term) to read next.
    func addOwn(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        lines.append(Line(tag: "yours", text: trimmed))
    }

    func startListening() async {
        guard listening == .idle else { return }
        error = nil
        do {
            try await recorder.startListening()
            listening = .listening
        } catch {
            self.error = error.localizedDescription
        }
    }

    func stopListening() async {
        guard listening == .listening, lines.indices.contains(current) else { return }
        listening = .working
        do {
            lines[current].heard = try await recorder.stopListening()
        } catch {
            self.error = error.localizedDescription
        }
        listening = .idle
    }

    /// Discards a take so the line can be read again.
    func redo() {
        guard lines.indices.contains(current) else { return }
        lines[current].heard = nil
    }

    func next() {
        if isLastLine {
            finishReading()
        } else {
            current += 1
        }
    }

    func skip() {
        redo()
        next()
    }

    /// Stops reading and lets the brain learn from every line that was read.
    func finishReading() {
        Task { await recorder.cancelListening() }
        recorder.close()
        let attempts = lines.compactMap { line in
            line.heard.map { CalibrationAttempt(expected: line.text, heard: $0) }
        }
        do {
            try brain.learn(attempts)
            step = .learned(attempts.count)
        } catch {
            self.error = error.localizedDescription
        }
    }

    func leave() {
        Task { await recorder.cancelListening() }
        recorder.close()
    }
}
