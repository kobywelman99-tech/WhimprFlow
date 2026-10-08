import AVFoundation
import Speech
import os

private let log = Logger(subsystem: "com.whimpr.whimprflow", category: "asr")

/// On-device speech-to-text with Apple's DictationTranscriber (iOS 26), the
/// model behind the system keyboard's dictation. No Apple Intelligence needed.
///
/// Not SpeechTranscriber: on the iPhone it ignored vocabulary hints entirely
/// ("Kobe Wellman" with or without "Koby", "Welman" as hints), while
/// DictationTranscriber honours them. See SpeechEngineTests.
///
/// An analyzer can't be restarted once it finishes, so each utterance gets its
/// own. `.processLifetime` keeps the model loaded between them.
@MainActor
final class SpeechEngine {
    enum EngineError: LocalizedError {
        case unavailable
        case unsupportedLocale
        case notPrepared

        var errorDescription: String? {
            switch self {
            case .unavailable: "On-device speech recognition isn't available on this iPhone."
            case .unsupportedLocale: "Your language isn't supported by on-device speech recognition yet."
            case .notPrepared: "The speech model isn't ready yet."
            }
        }
    }

    /// One dictation in progress.
    final class Utterance {
        fileprivate let analyzer: SpeechAnalyzer
        fileprivate let results: Task<String, Error>

        fileprivate init(analyzer: SpeechAnalyzer, results: Task<String, Error>) {
            self.analyzer = analyzer
            self.results = results
        }
    }

    private(set) var audioFormat: AVAudioFormat?
    private var locale: Locale?

    var isPrepared: Bool { audioFormat != nil }

    /// Picks the locale and installs its model if needed (a one-time download).
    func prepare(onDownload: () -> Void) async throws {
        var match = await DictationTranscriber.supportedLocale(equivalentTo: Locale.current)
        if match == nil {
            match = await DictationTranscriber.supportedLocale(equivalentTo: Locale(identifier: "en-US"))
        }
        guard let locale = match else { throw EngineError.unsupportedLocale }

        let probe = makeTranscriber(locale: locale)
        if await AssetInventory.status(forModules: [probe]) < .installed,
           let request = try await AssetInventory.assetInstallationRequest(supporting: [probe]) {
            onDownload()
            log.info("Downloading speech model for \(locale.identifier, privacy: .public)")
            try await request.downloadAndInstall()
        }

        audioFormat = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [probe])
        self.locale = locale
        log.info("Speech engine ready: \(locale.identifier, privacy: .public), \(String(describing: self.audioFormat), privacy: .public)")
    }

    /// Starts transcribing `input`. `vocabulary` (the user's dictionary) nudges
    /// the recogniser toward those spellings. `choose` picks among the
    /// recogniser's candidate transcriptions for each finished stretch (best
    /// first); without it the first is used. `onText` gets the running
    /// transcript, including words that may still change.
    func begin(
        input: AsyncStream<AnalyzerInput>,
        vocabulary: [String] = [],
        choose: (@MainActor ([String]) -> String)? = nil,
        onText: @escaping @MainActor (String) -> Void
    ) async throws -> Utterance {
        guard let locale, let audioFormat else { throw EngineError.notPrepared }
        let transcriber = makeTranscriber(locale: locale)
        let analyzer = SpeechAnalyzer(
            modules: [transcriber],
            options: .init(priority: .userInitiated, modelRetention: .processLifetime)
        )

        let results = Task { @MainActor in
            var finalized = ""
            for try await result in transcriber.results {
                let text = String(result.text.characters)
                if result.isFinal {
                    finalized += choose.map { Self.pick(text, result.alternatives, with: $0) } ?? text
                    onText(finalized)
                } else {
                    onText(finalized + text)
                }
            }
            return finalized.trimmingCharacters(in: .whitespacesAndNewlines)
        }

        if !vocabulary.isEmpty {
            let context = AnalysisContext()
            context.contextualStrings[.general] = vocabulary
            try await analyzer.setContext(context)
        }
        try await analyzer.prepareToAnalyze(in: audioFormat)
        try await analyzer.start(inputSequence: input)
        return Utterance(analyzer: analyzer, results: results)
    }

    /// Waits for the last words once the input stream has ended.
    func finish(_ utterance: Utterance) async throws -> String {
        try await utterance.analyzer.finalizeAndFinishThroughEndOfInput()
        return try await utterance.results.value
    }

    /// Throws away an utterance without waiting for its text.
    func cancel(_ utterance: Utterance) async {
        await utterance.analyzer.cancelAndFinishNow()
        utterance.results.cancel()
    }

    /// The recogniser's first choice and its alternatives, without repeats.
    private static func pick(_ text: String, _ alternatives: [AttributedString], with choose: @MainActor ([String]) -> String) -> String {
        var candidates = [text]
        for alternative in alternatives.map({ String($0.characters) }) where !candidates.contains(alternative) {
            candidates.append(alternative)
        }
        return choose(candidates)
    }

    private func makeTranscriber(locale: Locale) -> DictationTranscriber {
        DictationTranscriber(
            locale: locale,
            contentHints: [],
            transcriptionOptions: [.punctuation],
            reportingOptions: [.volatileResults, .alternativeTranscriptions],
            attributeOptions: []
        )
    }
}
