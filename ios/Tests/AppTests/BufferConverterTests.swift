import AVFoundation
import XCTest
@testable import WhimprFlow

/// The mic delivers 48 kHz float audio; the speech model wants its own format
/// (16 kHz Int16 mono on current iPhones). A bad conversion means silence or
/// chipmunk audio reaching the recogniser, with no error anywhere.
final class BufferConverterTests: XCTestCase {

    private let micFormat = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
    private let analyzerFormat = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)!

    /// 100 ms of a 440 Hz tone at half volume.
    private func tone(frames: AVAudioFrameCount = 4_800) -> AVAudioPCMBuffer {
        let buffer = AVAudioPCMBuffer(pcmFormat: micFormat, frameCapacity: frames)!
        buffer.frameLength = frames
        let samples = buffer.floatChannelData![0]
        for i in 0..<Int(frames) {
            samples[i] = 0.5 * sin(2 * .pi * 440 * Float(i) / 48_000)
        }
        return buffer
    }

    func testResamplesToAnalyzerFormat() throws {
        let converter = try BufferConverter(from: micFormat, to: analyzerFormat)
        let output = try converter.convert(tone())

        XCTAssertEqual(output.format, analyzerFormat)
        // 4,800 frames at 48 kHz is 1,600 at 16 kHz. The resampler holds some
        // back at first (see testNothingIsLost), but never makes extra.
        XCTAssertGreaterThan(output.frameLength, 1_200)
        XCTAssertLessThanOrEqual(output.frameLength, 1_601)
    }

    func testKeepsTheSignal() throws {
        let converter = try BufferConverter(from: micFormat, to: analyzerFormat)
        let output = try converter.convert(tone())
        let samples = UnsafeBufferPointer(start: output.int16ChannelData![0], count: Int(output.frameLength))
        let peak = samples.map { abs(Int($0)) }.max() ?? 0

        // Half volume is about 16,384; allow for the resampler's filtering.
        XCTAssertGreaterThan(peak, 12_000)
        XCTAssertLessThan(peak, 20_000)
    }

    func testNothingIsLost() throws {
        let converter = try BufferConverter(from: micFormat, to: analyzerFormat)
        var total = try (0..<10).reduce(0) { sum, _ in sum + Int(try converter.convert(tone()).frameLength) }
        total += Int(try converter.flush()?.frameLength ?? 0)

        // One second in ten chunks comes out as one second once the tail
        // held in the resampler is flushed.
        XCTAssertEqual(Double(total), 16_000, accuracy: 16)
    }

    func testFlushResetsForTheNextDictation() throws {
        let converter = try BufferConverter(from: micFormat, to: analyzerFormat)
        _ = try converter.convert(tone())
        _ = try converter.flush()

        let next = try converter.convert(tone())
        XCTAssertGreaterThan(next.frameLength, 1_200)
    }

    func testPassesThroughMatchingFormat() throws {
        let converter = try BufferConverter(from: micFormat, to: micFormat)
        let input = tone()
        XCTAssertTrue(try converter.convert(input) === input)
    }
}
