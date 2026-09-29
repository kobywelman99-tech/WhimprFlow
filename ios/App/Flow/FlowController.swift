import Foundation
import Observation
import os

private let log = Logger(subsystem: "com.whimpr.whimprflow", category: "flow")

/// A Flow session: the mic stays on in the background and the keyboard drives
/// dictation over the bridge. Started by the keyboard opening
/// `whimprflow://flow?token=…`; ends after `idleTimeout` without dictation, on
/// an audio interruption, or from the app's End button.
@MainActor
@Observable
final class FlowController {
    static let idleTimeout: Duration = .seconds(15 * 60)

    private(set) var isOn = false
    private(set) var keyboardConnected = false
    private(set) var lastError: String?

    private let dictation: DictationService
    /// For the default level the keyboard's pill starts at.
    private let settings: CleanupSettings
    @ObservationIgnored private var server: BridgeServer?
    @ObservationIgnored private var idleTimer: Task<Void, Never>?

    init(dictation: DictationService, settings: CleanupSettings) {
        self.dictation = dictation
        self.settings = settings
        dictation.onSessionInterrupted = { [weak self] in self?.end() }
    }

    /// Handles a URL the app was opened with. Returns false if it isn't ours.
    @discardableResult
    func handle(url: URL) async -> Bool {
        guard let token = Bridge.token(fromFlowURL: url) else { return false }
        await start(token: token)
        return true
    }

    func start(token: String) async {
        lastError = nil
        do {
            try await dictation.startSession()
            dictation.keepSessionOpen()
            if let server {
                server.update(token: token)
            } else {
                let server = BridgeServer(token: token)
                server.makeReady = { [weak self] in
                    BridgeMessage(.ready, cleanupLevel: self.map { DictationLevel($0.settings.level) })
                }
                server.onMessage = { [weak self] message in self?.handle(message) }
                server.onAuthorized = { [weak self] in
                    self?.keyboardConnected = true
                    // Proves the keyboard is added and has Full Access (Setup).
                    SetupProgress().markKeyboardConnected()
                }
                server.onDisconnect = { [weak self] in self?.keyboardDisconnected() }
                try server.start()
                self.server = server
            }
            isOn = true
            resetIdleTimer()
            log.info("Flow session on")
        } catch {
            log.error("Flow session failed to start: \(error.localizedDescription, privacy: .public)")
            lastError = error.localizedDescription
            end()
        }
    }

    func end() {
        idleTimer?.cancel()
        idleTimer = nil
        server?.stop()
        server = nil
        keyboardConnected = false
        if dictation.sessionActive { dictation.endSession() }
        if isOn { log.info("Flow session off") }
        isOn = false
    }

    private func handle(_ message: BridgeMessage) {
        keyboardConnected = true
        resetIdleTimer()
        switch message.kind {
        case .start:
            Task {
                do {
                    try await dictation.beginUtterance(
                        level: message.cleanupLevel.map(Level.init),
                        onText: { [weak self] text in
                            self?.server?.send(BridgeMessage(.partial, text: text))
                        },
                        onLevel: { [weak self] value in
                            Task { @MainActor in
                                self?.server?.send(BridgeMessage(.audioLevel, value: value))
                            }
                        }
                    )
                    server?.send(BridgeMessage(.recording))
                } catch {
                    server?.send(BridgeMessage(.error, text: error.localizedDescription))
                }
            }
        case .stop:
            server?.send(BridgeMessage(.transcribing))
            Task {
                do {
                    let text = try await dictation.finishUtterance(source: .keyboard) { [weak self] in
                        self?.server?.send(BridgeMessage(.cleaning))
                    }
                    server?.send(BridgeMessage(.final, text: text))
                    SetupProgress().markKeyboardDictated()
                } catch {
                    server?.send(BridgeMessage(.error, text: error.localizedDescription))
                }
                resetIdleTimer()
            }
        case .cancel:
            Task { await dictation.cancelUtterance() }
        case .hello, .ready, .denied, .recording, .audioLevel, .partial, .transcribing, .cleaning, .final, .error:
            break
        }
    }

    private func keyboardDisconnected() {
        keyboardConnected = false
        // The keyboard went away mid-sentence: don't keep listening.
        if dictation.isRecording {
            Task { await dictation.cancelUtterance() }
        }
    }

    private func resetIdleTimer() {
        idleTimer?.cancel()
        idleTimer = Task { [weak self] in
            try? await Task.sleep(for: Self.idleTimeout)
            guard !Task.isCancelled else { return }
            log.info("Flow session idle for \(Self.idleTimeout, privacy: .public)")
            self?.end()
        }
    }
}
