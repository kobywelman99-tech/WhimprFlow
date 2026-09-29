import AVFoundation
import Speech
import XCTest
@testable import WhimprFlow

final class MeteringTests: XCTestCase {
    private func sine(amplitude: Float) -> AVAudioPCMBuffer {
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4_800)!
        buffer.frameLength = 4_800
        for i in 0..<4_800 {
            buffer.floatChannelData![0][i] = amplitude * sin(2 * .pi * 440 * Float(i) / 48_000)
        }
        return buffer
    }

    func testRMSOfASine() {
        // A sine's RMS is its amplitude / √2.
        XCTAssertEqual(Recorder.rms(of: sine(amplitude: 0.5)), 0.5 / Float(2).squareRoot(), accuracy: 0.001)
        XCTAssertEqual(Recorder.rms(of: sine(amplitude: 0)), 0)
    }

    func testMeterScale() {
        XCTAssertEqual(Recorder.meterLevel(rms: 0), 0)
        XCTAssertEqual(Recorder.meterLevel(rms: 0.001), 0, "-60 dB is below the floor")
        XCTAssertEqual(Recorder.meterLevel(rms: pow(10, -30.0 / 20)), 0.5, accuracy: 0.001, "-30 dB is halfway")
        XCTAssertEqual(Recorder.meterLevel(rms: 1), 1, "clipped at the top")
    }

    func testLouderIsHigher() {
        let quiet = Recorder.meterLevel(rms: Recorder.rms(of: sine(amplitude: 0.01)))
        let loud = Recorder.meterLevel(rms: Recorder.rms(of: sine(amplitude: 0.2)))
        XCTAssertGreaterThan(loud, quiet)
    }
}

@MainActor
final class LevelOverrideTests: XCTestCase {
    private final class Recorder: CleanupModel, @unchecked Sendable {
        private let lock = NSLock()
        private var systems: [String] = []
        var sentSystems: [String] { lock.withLock { systems } }
        func clean(_ request: CleanupRequest) async throws -> String {
            lock.withLock { systems.append(request.system) }
            return "Hello there."
        }
    }

    private func pipeline() throws -> (CleanupPipeline, Recorder) {
        let keychain = KeychainStore(service: "level-tests.\(UUID().uuidString)")
        addTeardownBlock { try? keychain.delete(CleanupSettings.apiKeyAccount) }
        let settings = CleanupSettings(defaults: UserDefaults(suiteName: UUID().uuidString)!, keychain: keychain)
        try settings.saveAPIKey("sk-test")
        let model = Recorder()
        return (CleanupPipeline(settings: settings, makeModel: { _ in model }), model)
    }

    func testOverrideChangesThePrompt() async throws {
        let (pipeline, model) = try pipeline()
        _ = await pipeline.clean("um hello there")
        _ = await pipeline.clean("um hello there", level: .high)

        let systems = model.sentSystems
        XCTAssertEqual(systems.count, 2)
        XCTAssertNotEqual(systems[0], systems[1], "High asks for a different edit than the default Light")
        XCTAssertEqual(systems[0], prepareCleanup(raw: "x", level: .light, vocab: [], appBundleId: nil, styleSummary: nil).system)
        XCTAssertEqual(systems[1], prepareCleanup(raw: "x", level: .high, vocab: [], appBundleId: nil, styleSummary: nil).system)
    }

    func testRawOverrideSkipsClaude() async throws {
        let (pipeline, model) = try pipeline()
        let result = await pipeline.clean("um hello there", level: .off)
        XCTAssertEqual(result.text, "um hello there")
        XCTAssertTrue(model.sentSystems.isEmpty)
    }

    func testLevelUsedForHistory() throws {
        let keychain = KeychainStore(service: "level-used.\(UUID().uuidString)")
        let settings = CleanupSettings(defaults: UserDefaults(suiteName: UUID().uuidString)!, keychain: keychain)
        let pipeline = CleanupPipeline(settings: settings)
        XCTAssertEqual(pipeline.level(for: nil), .light, "the default")
        XCTAssertEqual(pipeline.level(for: .high), .high, "the pill's choice")
        settings.enabled = false
        XCTAssertNil(pipeline.level(for: .high), "cleanup off")
    }

    func testLevelNamesMapBothWays() {
        for name in DictationLevel.allCases {
            XCTAssertEqual(DictationLevel(Level(name)), name)
        }
    }
}

/// Koby's first-day bug: music left his Bluetooth headphones while Flow was on.
final class AudioSessionTests: XCTestCase {
    func testMusicStaysOnBluetoothHeadphones() {
        let options = Recorder.sessionOptions
        XCTAssertTrue(options.contains(.allowBluetoothA2DP), "without it, playback moves to the phone speaker")
        XCTAssertTrue(options.contains(.mixWithOthers), "music keeps playing")
        XCTAssertFalse(options.contains(.allowBluetoothHFP), "HFP turns music into call-quality audio")
    }

    func testIOSAcceptsTheSessionSetup() throws {
        // setCategory throws on combinations iOS rejects.
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .default, options: Recorder.sessionOptions)
        XCTAssertTrue(session.categoryOptions.contains(.allowBluetoothA2DP))
    }
}

/// When headphones come or go mid-dictation, the mic's format can change.
final class RouteChangeTests: XCTestCase {
    private func tone(rate: Double, seconds: Double = 0.1) -> AVAudioPCMBuffer {
        let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1)!
        let frames = AVAudioFrameCount(rate * seconds)
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)!
        buffer.frameLength = frames
        for i in 0..<Int(frames) {
            buffer.floatChannelData![0][i] = 0.5 * sin(2 * .pi * 440 * Float(i) / Float(rate))
        }
        return buffer
    }

    func testCaptureSurvivesAFormatChange() async throws {
        let analyzer = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)!
        let (stream, continuation) = AsyncStream.makeStream(of: AnalyzerInput.self)
        var capture: Recorder.Capture? = Recorder.Capture(
            converter: try BufferConverter(from: tone(rate: 48_000).format, to: analyzer),
            continuation: continuation,
            onLevel: { _ in }
        )

        // 0.5 s on the built-in mic at 48 kHz, then 0.5 s after a switch to 44.1 kHz.
        for _ in 0..<5 { Recorder.process(tone(rate: 48_000), into: &capture) }
        for _ in 0..<5 { Recorder.process(tone(rate: 44_100), into: &capture) }
        XCTAssertEqual(capture?.converter.inputFormat.sampleRate, 44_100, "a new converter took over")
        if let tail = try capture?.converter.flush() { continuation.yield(AnalyzerInput(buffer: tail)) }
        continuation.finish()

        var frames = 0
        for await input in stream {
            XCTAssertEqual(input.buffer.format, analyzer, "everything reaches the analyzer in its format")
            frames += Int(input.buffer.frameLength)
        }
        XCTAssertEqual(Double(frames), 16_000, accuracy: 32, "one second in, one second out: nothing lost at the switch")
    }
}
