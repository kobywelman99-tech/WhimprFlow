import AVFoundation
import Foundation
import Observation
import os

private let log = Logger(subsystem: "com.whimpr.whimprflow", category: "dictation")

/// Mic session plus speech engine. Used by the in-app mic button and, through
/// FlowController, by the keyboard.
@MainActor
@Observable
final class DictationService {
    enum Phase: Equatable {
        case idle
        case preparing(String)
        case recording
        case transcribing
        case cleaning
        case failed(String)
    }

    enum DictationError: LocalizedError {
        case microphoneDenied
        case notReady
        case notRecording

        var errorDescription: String? {
            switch self {
            case .microphoneDenied: "Microphone access is off. Turn it on in Settings ▸ Apps ▸ WhimprFlow."
            case .notReady: "Dictation isn't ready yet."
            case .notRecording: "Nothing is being recorded."
            }
        }
    }

    private(set) var phase: Phase = .idle
    private(set) var transcript = ""
    /// Time from stop to having the final text.
    private(set) var lastLatency: Duration?
    /// How the last dictation's cleanup went (nil while none has finished).
    private(set) var lastCleanup: CleanupResult?
    private(set) var sessionActive = false

    /// Called when iOS takes the mic away (a phone call, Siri, a route change).
    var onSessionInterrupted: (() -> Void)?

    private let engine = SpeechEngine()
    private let cleanup: CleanupPipeline
    /// The dictionary's words, as hints for the speech recogniser.
    private let vocabulary: () -> [String]
    private let recorder = Recorder()
    private var utterance: SpeechEngine.Utterance?
    /// The keyboard pill's level for the dictation in progress.
    private var levelOverride: Level?
    private var observers: [NSObjectProtocol] = []
    /// The in-app button opened the session just for one dictation.
    private var endSessionAfterUtterance = false

    init(cleanup: CleanupPipeline, vocabulary: @escaping () -> [String] = { [] }) {
        self.cleanup = cleanup
        self.vocabulary = vocabulary
    }

    // MARK: Session

    func startSession() async throws {
        guard !sessionActive else { return }
        guard await Recorder.requestPermission() else { throw DictationError.microphoneDenied }
        if !engine.isPrepared {
            phase = .preparing("Getting speech recognition ready…")
            do {
                try await engine.prepare { phase = .preparing("Downloading the speech model (one time)…") }
            } catch {
                phase = .failed(error.localizedDescription)
                throw error
            }
        }
        try recorder.startSession()
        observeInterruptions()
        sessionActive = true
        phase = .idle
        log.info("Mic session started (\(Recorder.routeDescription, privacy: .public))")
    }

    func endSession() {
        if let utterance {
            self.utterance = nil
            Task { await engine.cancel(utterance) }
        }
        recorder.stopSession()
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        sessionActive = false
        endSessionAfterUtterance = false
        if phase == .recording || phase == .transcribing || phase == .cleaning { phase = .idle }
        log.info("Mic session ended")
    }

    private func observeInterruptions() {
        let center = NotificationCenter.default
        // A phone call, Siri, an alarm: the mic is taken away, so Flow ends.
        // Only an interruption *beginning* matters; by the end the session is
        // already gone.
        let interrupted: @Sendable (Notification) -> Void = { [weak self] note in
            let ended = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt)
                .flatMap(AVAudioSession.InterruptionType.init(rawValue:)) == .ended
            MainActor.assumeIsolated {
                guard let self, self.sessionActive, !ended else { return }
                log.info("Mic session interrupted")
                self.endSession()
                self.onSessionInterrupted?()
            }
        }
        // Headphones connected or disconnected: iOS stops the engine while it
        // reroutes. Start it again rather than ending Flow.
        let reconfigured: @Sendable (Notification) -> Void = { [weak self] _ in
            MainActor.assumeIsolated {
                self?.restartAfterConfigurationChange()
            }
        }
        observers = [
            center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main, using: interrupted),
            center.addObserver(forName: .AVAudioEngineConfigurationChange, object: nil, queue: .main, using: reconfigured),
        ]
    }

    private func restartAfterConfigurationChange() {
        guard sessionActive else { return }
        do {
            try recorder.restartAfterConfigurationChange()
            log.info("Audio route changed, mic restarted (\(Recorder.routeDescription, privacy: .public))")
        } catch {
            log.error("Mic couldn't restart after a route change: \(error.localizedDescription, privacy: .public)")
            endSession()
            onSessionInterrupted?()
        }
    }

    // MARK: Utterances

    var isRecording: Bool { utterance != nil }

    func beginUtterance(
        level: Level? = nil,
        onText: @escaping @MainActor (String) -> Void = { _ in },
        onLevel: @escaping @Sendable (Double) -> Void = { _ in }
    ) async throws {
        guard sessionActive, let format = engine.audioFormat else { throw DictationError.notReady }
        guard utterance == nil else { return }
        transcript = ""
        lastLatency = nil
        lastCleanup = nil
        levelOverride = level
        let input = try recorder.beginCapture(convertingTo: format, onLevel: onLevel)
        do {
            utterance = try await engine.begin(input: input, vocabulary: vocabulary()) { [weak self] text in
                self?.transcript = text
                onText(text)
            }
        } catch {
            recorder.endCapture()
            throw error
        }
        phase = .recording
    }

    /// Stops listening and returns the final text, cleaned up if cleanup is
    /// on. `onCleaning` fires once the words are in and cleanup starts.
    func finishUtterance(onCleaning: () -> Void = {}) async throws -> String {
        guard let utterance else { throw DictationError.notRecording }
        self.utterance = nil
        let stoppedAt = ContinuousClock.now
        phase = .transcribing
        // The tap delivers audio in ~85 ms chunks; wait for the one holding
        // the last syllable before closing the stream.
        try? await Task.sleep(for: .milliseconds(150))
        recorder.endCapture()
        do {
            let raw = try await engine.finish(utterance)
            transcript = raw
            let heard = ContinuousClock.now - stoppedAt
            phase = .cleaning
            onCleaning()
            let result = await cleanup.clean(raw, level: levelOverride)
            transcript = result.text
            lastCleanup = result
            let latency = ContinuousClock.now - stoppedAt
            lastLatency = latency
            log.info("\(raw.split(separator: " ").count) words; speech \(heard, privacy: .public), total \(latency, privacy: .public), cleaned: \(result.cleaned)")
            phase = .idle
            return result.text
        } catch {
            log.error("Transcription failed: \(error.localizedDescription, privacy: .public)")
            phase = .failed(error.localizedDescription)
            throw error
        }
    }

    func cancelUtterance() async {
        guard let utterance else { return }
        self.utterance = nil
        recorder.endCapture()
        await engine.cancel(utterance)
        phase = .idle
    }

    // MARK: In-app button

    func toggleInApp() async {
        switch phase {
        case .recording:
            _ = try? await finishUtterance()
            if endSessionAfterUtterance { endSession() }
        case .idle, .failed:
            do {
                if !sessionActive {
                    try await startSession()
                    endSessionAfterUtterance = true
                }
                try await beginUtterance()
            } catch {
                if endSessionAfterUtterance { endSession() }
                phase = .failed(error.localizedDescription)
            }
        case .preparing, .transcribing, .cleaning:
            break
        }
    }

    /// A Flow session started while the in-app button had the mic: keep it.
    func keepSessionOpen() {
        endSessionAfterUtterance = false
    }
}
