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
                server.onAuthorized = { [weak self] in self?.keyboardAuthorized() }
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

    /// Where a finished dictation goes.
    enum Delivery: Equatable {
        /// Its keyboard is still open: insert it.
        case insert
        /// Its keyboard closed: keep it for the next keyboard to offer.
        case keepForLater
    }

    static func delivery(requestedBy: Int?, connected: Int?) -> Delivery {
        requestedBy != nil && requestedBy == connected ? .insert : .keepForLater
    }

    /// The connection that started the dictation in progress.
    @ObservationIgnored private var activeConnection: Int?
    @ObservationIgnored private var pending = PendingDictation()

    private var currentConnection: Int? {
        guard let server, server.isConnected else { return nil }
        return server.connectionID
    }

    /// Progress goes only to the keyboard that started the dictation.
    private func sendToActive(_ message: BridgeMessage) {
        guard activeConnection != nil, activeConnection == currentConnection else { return }
        server?.send(message)
    }

    private func handle(_ message: BridgeMessage) {
        keyboardConnected = true
        resetIdleTimer()
        switch message.kind {
        case .start:
            pending.clear()
            activeConnection = currentConnection
            Task {
                do {
                    try await dictation.beginUtterance(
                        level: message.cleanupLevel.map(Level.init),
                        onText: { [weak self] text in
                            self?.sendToActive(BridgeMessage(.partial, text: text))
                        },
                        onLevel: { [weak self] value in
                            Task { @MainActor in
                                self?.sendToActive(BridgeMessage(.audioLevel, value: value))
                            }
                        }
                    )
                    if activeConnection != nil, activeConnection == currentConnection {
                        sendToActive(BridgeMessage(.recording))
                    } else {
                        // The keyboard closed while the mic was starting.
                        finish()
                    }
                } catch {
                    sendToActive(BridgeMessage(.error, text: error.localizedDescription))
                }
            }
        case .stop:
            sendToActive(BridgeMessage(.transcribing))
            finish()
        case .cancel:
            activeConnection = nil
            Task { await dictation.cancelUtterance() }
        case .claim:
            pending.clear()
        case .hello, .ready, .denied, .recording, .audioLevel, .partial, .transcribing, .cleaning, .final, .error, .pending:
            break
        }
    }

    /// Finishes the dictation in progress and delivers it to its keyboard,
    /// or keeps it if that keyboard has closed in the meantime.
    private func finish() {
        let requester = activeConnection
        Task {
            do {
                let text = try await dictation.finishUtterance(source: .keyboard) { [weak self] in
                    self?.sendToActive(BridgeMessage(.cleaning))
                }
                activeConnection = nil
                switch Self.delivery(requestedBy: requester, connected: currentConnection) {
                case .insert:
                    server?.send(BridgeMessage(.final, text: text))
                case .keepForLater:
                    guard !text.isEmpty else { break }
                    pending.store(text)
                    log.info("Dictation finished after its keyboard closed; kept for later")
                    if server?.isConnected == true { server?.send(BridgeMessage(.pending, text: text)) }
                }
                SetupProgress().markKeyboardDictated()
            } catch {
                activeConnection = nil
                sendToActive(BridgeMessage(.error, text: error.localizedDescription))
            }
            resetIdleTimer()
        }
    }

    /// A keyboard connected: offer it anything that finished while no
    /// keyboard was open.
    private func keyboardAuthorized() {
        keyboardConnected = true
        // Proves the keyboard is added and has Full Access (Setup).
        SetupProgress().markKeyboardConnected()
        if let text = pending.text() {
            server?.send(BridgeMessage(.pending, text: text))
        }
    }

    private func keyboardDisconnected() {
        keyboardConnected = false
        // The keyboard closed mid-sentence (scrolling a chat hides it): finish
        // rather than throw the words away. The result is kept for later.
        if dictation.isRecording {
            finish()
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
