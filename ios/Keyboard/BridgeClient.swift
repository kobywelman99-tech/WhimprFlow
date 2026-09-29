import Foundation
import Network

/// The keyboard's end of the bridge (see Shared/BridgeProtocol.swift).
@MainActor
final class BridgeClient {
    enum Event: Equatable {
        /// Connected to a live Flow session; carries the app's default level.
        case ready(DictationLevel?)
        /// No Flow session (nothing listening, or it ended); open the app.
        case noSession
        case recording
        /// Mic loudness 0…1, about ten times a second while recording.
        case audioLevel(Double)
        case partial(String)
        case transcribing
        /// The words are in; Claude is tidying them up.
        case cleaning
        case final(String)
        /// A dictation that finished after this keyboard closed, to offer.
        case pending(String)
        case error(String)
    }

    var onEvent: ((Event) -> Void)?
    private(set) var isConnected = false

    private var connection: NWConnection?
    private var framer = LineFramer()

    func connect(token: String) {
        disconnect()
        let connection = NWConnection(
            host: NWEndpoint.Host(Bridge.host),
            port: NWEndpoint.Port(rawValue: Bridge.port)!,
            using: .tcp
        )
        self.connection = connection
        framer = LineFramer()
        connection.stateUpdateHandler = { [weak self] state in
            MainActor.assumeIsolated {
                guard let self, connection === self.connection else { return }
                switch state {
                case .ready:
                    self.send(BridgeMessage(.hello, text: token))
                    self.receive(on: connection)
                case .waiting, .failed:
                    // Refused: the app isn't running a Flow session.
                    self.lost(connection)
                default:
                    break
                }
            }
        }
        connection.start(queue: .main)

        // A live session answers within milliseconds; don't hang the keyboard.
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(1.5))
            guard let self, connection === self.connection, !self.isConnected else { return }
            self.lost(connection)
        }
    }

    func disconnect() {
        connection?.cancel()
        connection = nil
        isConnected = false
    }

    func send(_ message: BridgeMessage) {
        connection?.send(content: LineFramer.encode(message), completion: .contentProcessed { _ in })
    }

    private func receive(on connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, isComplete, error in
            MainActor.assumeIsolated {
                guard let self, connection === self.connection else { return }
                if let data {
                    for message in self.framer.append(data) { self.handle(message, from: connection) }
                }
                if isComplete || error != nil {
                    self.lost(connection)
                } else {
                    self.receive(on: connection)
                }
            }
        }
    }

    private func handle(_ message: BridgeMessage, from connection: NWConnection) {
        switch message.kind {
        case .ready:
            isConnected = true
            onEvent?(.ready(message.cleanupLevel))
        case .denied:
            lost(connection)
        case .recording:
            onEvent?(.recording)
        case .audioLevel:
            onEvent?(.audioLevel(message.value ?? 0))
        case .partial:
            onEvent?(.partial(message.text ?? ""))
        case .transcribing:
            onEvent?(.transcribing)
        case .cleaning:
            onEvent?(.cleaning)
        case .final:
            onEvent?(.final(message.text ?? ""))
        case .error:
            onEvent?(.error(message.text ?? "Something went wrong."))
        case .pending:
            if let text = message.text, !text.isEmpty { onEvent?(.pending(text)) }
        case .hello, .start, .stop, .cancel, .claim:
            break
        }
    }

    private func lost(_ connection: NWConnection) {
        guard connection === self.connection else { return }
        connection.cancel()
        self.connection = nil
        isConnected = false
        onEvent?(.noSession)
    }
}
