import AVFoundation
import Speech
import os

/// Owns the microphone.
///
/// A session keeps the mic running, which is also what keeps the app alive in
/// the background (UIBackgroundModes `audio`) so the keyboard can use it. iOS
/// won't let a backgrounded app *start* the mic, so the engine runs for the
/// whole session and audio is only forwarded while a capture is open.
final class Recorder: @unchecked Sendable {
    enum RecorderError: LocalizedError {
        case noInput
        case notRunning

        var errorDescription: String? {
            switch self {
            case .noInput: "No microphone is available."
            case .notRunning: "The microphone isn't on."
            }
        }
    }

    struct Capture: Sendable {
        /// Replaced when the mic's format changes mid-capture (headphones).
        var converter: BufferConverter
        let continuation: AsyncStream<AnalyzerInput>.Continuation
        /// Mic loudness 0…1 per buffer (~10 a second), for the waveform.
        let onLevel: @Sendable (Double) -> Void
    }

    /// How the app shares audio while Flow is on:
    /// - `mixWithOthers`: the user's music keeps playing.
    /// - `allowBluetoothA2DP`: that music keeps going to Bluetooth headphones.
    ///   Without it, `.playAndRecord` can't use A2DP and iOS moves playback to
    ///   the phone speaker (Koby's first-day bug). A2DP has no mic, so
    ///   dictation uses the iPhone's.
    /// - `defaultToSpeaker`: with no headphones, sound comes from the speaker,
    ///   not the earpiece.
    /// Not `allowBluetoothHFP`: that would pull headphones into call-quality
    /// mode and make music sound like a phone call.
    static let sessionOptions: AVAudioSession.CategoryOptions = [.mixWithOthers, .allowBluetoothA2DP, .defaultToSpeaker]

    private let engine = AVAudioEngine()
    /// Touched by the audio thread (the tap) and by callers, hence the lock.
    private let capture = OSAllocatedUnfairLock<Capture?>(initialState: nil)

    var isRunning: Bool { engine.isRunning }

    static func requestPermission() async -> Bool {
        await AVAudioApplication.requestRecordPermission()
    }

    func startSession() throws {
        guard !engine.isRunning else { return }
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .default, options: Self.sessionOptions)
        try session.setActive(true)
        try installTapAndStart()
    }

    /// After iOS reconfigures audio (headphones in or out) the engine has
    /// stopped. Start it again on the new route; an open capture carries on.
    func restartAfterConfigurationChange() throws {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        try installTapAndStart()
    }

    /// "in: MicrophoneBuiltIn, out: BluetoothA2DPOutput", for logs.
    static var routeDescription: String {
        let route = AVAudioSession.sharedInstance().currentRoute
        let inputs = route.inputs.map(\.portType.rawValue).joined(separator: "+")
        let outputs = route.outputs.map(\.portType.rawValue).joined(separator: "+")
        return "in: \(inputs), out: \(outputs)"
    }

    private func installTapAndStart() throws {
        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0 else { throw RecorderError.noInput }

        input.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { [capture] buffer, _ in
            capture.withLock { current in
                Recorder.process(buffer, into: &current)
            }
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw error
        }
    }

    /// One mic buffer for the open capture, if any: meter it, convert it and
    /// hand it to the analyzer. If the mic's format changed (the route
    /// changed), the old converter's tail is flushed and a new one takes over,
    /// so no audio is lost or garbled across the switch.
    static func process(_ buffer: AVAudioPCMBuffer, into capture: inout Capture?) {
        guard var current = capture else { return }
        current.onLevel(meterLevel(rms: rms(of: buffer)))
        if buffer.format != current.converter.inputFormat {
            if let tail = try? current.converter.flush() {
                current.continuation.yield(AnalyzerInput(buffer: tail))
            }
            guard let replacement = try? BufferConverter(from: buffer.format, to: current.converter.outputFormat) else { return }
            current.converter = replacement
            capture = current
        }
        guard let converted = try? current.converter.convert(buffer) else { return }
        current.continuation.yield(AnalyzerInput(buffer: converted))
    }

    func stopSession() {
        endCapture()
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Starts forwarding mic audio, converted to `format`, until `endCapture()`.
    /// `onLevel` is called on the audio thread.
    func beginCapture(
        convertingTo format: AVAudioFormat,
        onLevel: @escaping @Sendable (Double) -> Void = { _ in }
    ) throws -> AsyncStream<AnalyzerInput> {
        guard engine.isRunning else { throw RecorderError.notRunning }
        let converter = try BufferConverter(from: engine.inputNode.outputFormat(forBus: 0), to: format)
        let (stream, continuation) = AsyncStream.makeStream(of: AnalyzerInput.self)
        let previous = capture.withLock { current in
            defer { current = Capture(converter: converter, continuation: continuation, onLevel: onLevel) }
            return current
        }
        previous?.continuation.finish()
        return stream
    }

    /// Stops forwarding and ends the stream, which lets the analyzer finish.
    func endCapture() {
        let ended = capture.withLock { current in
            defer { current = nil }
            return current
        }
        guard let ended else { return }
        // The tap no longer sees this converter, so draining it here is safe.
        if let tail = try? ended.converter.flush() {
            ended.continuation.yield(AnalyzerInput(buffer: tail))
        }
        ended.continuation.finish()
    }
}

// MARK: Metering

extension Recorder {
    /// Root-mean-square of the first channel of a float buffer (0 for others).
    static func rms(of buffer: AVAudioPCMBuffer) -> Float {
        guard let samples = buffer.floatChannelData?[0], buffer.frameLength > 0 else { return 0 }
        var sum: Float = 0
        for i in 0..<Int(buffer.frameLength) {
            sum += samples[i] * samples[i]
        }
        return (sum / Float(buffer.frameLength)).squareRoot()
    }

    /// Maps RMS to 0…1 on a dB scale: -50 dB (a quiet room) is 0, -10 dB
    /// (speaking close to the phone) is 1.
    static func meterLevel(rms: Float) -> Double {
        guard rms > 0 else { return 0 }
        let decibels = 20 * log10(Double(rms))
        return min(1, max(0, (decibels + 50) / 40))
    }
}

/// Converts mic buffers (usually 48 kHz float) to the analyzer's format
/// (usually 16 kHz Int16 mono).
final class BufferConverter: @unchecked Sendable {
    enum ConversionError: LocalizedError {
        case unsupported(from: AVAudioFormat, to: AVAudioFormat)
        case failed(NSError?)

        var errorDescription: String? {
            switch self {
            case .unsupported(let from, let to): "Can't convert audio from \(from) to \(to)."
            case .failed(let error): "Audio conversion failed: \(error?.localizedDescription ?? "unknown error")"
            }
        }
    }

    let inputFormat: AVAudioFormat
    let outputFormat: AVAudioFormat
    private let converter: AVAudioConverter

    init(from input: AVAudioFormat, to output: AVAudioFormat) throws {
        guard let converter = AVAudioConverter(from: input, to: output) else {
            throw ConversionError.unsupported(from: input, to: output)
        }
        // Don't let the resampler swallow the first few milliseconds of speech.
        converter.primeMethod = .none
        self.converter = converter
        self.inputFormat = input
        self.outputFormat = output
    }

    func convert(_ buffer: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
        if buffer.format == outputFormat { return buffer }
        let ratio = outputFormat.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up)) + 1
        guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: capacity) else {
            throw ConversionError.failed(nil)
        }

        var supplied = false
        var error: NSError?
        let status = converter.convert(to: output, error: &error) { _, inputStatus in
            if supplied {
                inputStatus.pointee = .noDataNow
                return nil
            }
            supplied = true
            inputStatus.pointee = .haveData
            return buffer
        }
        if status == .error { throw ConversionError.failed(error) }
        return output
    }

    /// Returns the audio still held inside the resampler (about 15 ms), then
    /// resets it. Without this the end of the last word is cut off.
    func flush() throws -> AVAudioPCMBuffer? {
        guard converter.inputFormat != outputFormat,
              let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: 4096) else { return nil }
        var error: NSError?
        let status = converter.convert(to: output, error: &error) { _, inputStatus in
            inputStatus.pointee = .endOfStream
            return nil
        }
        converter.reset()
        if status == .error { throw ConversionError.failed(error) }
        return output.frameLength > 0 ? output : nil
    }
}
