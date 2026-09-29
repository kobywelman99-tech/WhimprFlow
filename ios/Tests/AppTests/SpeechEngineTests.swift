import AVFoundation
import Speech
import XCTest
@testable import WhimprFlow

/// Runs a recorded sentence (macOS `say`, in Fixtures/) through the real
/// on-device recogniser, the same way the mic path feeds it.
@MainActor
final class SpeechEngineTests: XCTestCase {

    private func transcribe(_ engine: SpeechEngine, file url: URL, vocabulary: [String] = []) async throws -> String {
        let format = try XCTUnwrap(engine.audioFormat)
        let file = try AVAudioFile(forReading: url)
        let converter = try BufferConverter(from: file.processingFormat, to: format)
        let (stream, continuation) = AsyncStream.makeStream(of: AnalyzerInput.self)

        let utterance = try await engine.begin(input: stream, vocabulary: vocabulary) { _ in }
        while file.framePosition < file.length {
            let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 4_096))
            try file.read(into: buffer)
            continuation.yield(AnalyzerInput(buffer: try converter.convert(buffer)))
        }
        if let tail = try converter.flush() { continuation.yield(AnalyzerInput(buffer: tail)) }
        continuation.finish()
        return try await engine.finish(utterance)
    }

    /// The simulator has no speech models; these run on a phone
    /// (`xcodebuild test -destination id=<udid> -only-testing:WhimprFlowTests/SpeechEngineTests`).
    private func requireRecognizer() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("Speech recognition runs on a real iPhone only")
        #endif
    }

    func testTranscribesASpokenSentenceTwice() async throws {
        try await requireRecognizer()
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "dictation-sample", withExtension: "wav"))
        let engine = SpeechEngine()
        try await engine.prepare(onDownload: {})

        // Twice: each dictation needs a fresh analyzer, so the second run
        // checks the engine can be reused.
        for run in 1...2 {
            let text = try await transcribe(engine, file: url).lowercased()
            for word in ["report", "sarah", "meeting", "tomorrow", "morning"] {
                XCTAssertTrue(text.contains(word), "run \(run): missing \"\(word)\" in \"\(text)\"")
            }
        }
    }

    /// Dictionary words passed as vocabulary should come out spelled the
    /// user's way. Both transcripts are logged, to see what the hints changed.
    func testVocabularyFixesUnusualNames() async throws {
        try await requireRecognizer()
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "vocabulary-sample", withExtension: "wav"))
        let engine = SpeechEngine()
        try await engine.prepare(onDownload: {})

        let without = try await transcribe(engine, file: url)
        let with = try await transcribe(engine, file: url, vocabulary: ["Koby", "Welman", "Zyphrex"])
        print("VOCAB without: \(without)")
        print("VOCAB with:    \(with)")

        for word in ["Koby", "Welman", "Zyphrex"] {
            XCTAssertTrue(with.contains(word), "missing \"\(word)\" in \"\(with)\"")
        }
    }
}
