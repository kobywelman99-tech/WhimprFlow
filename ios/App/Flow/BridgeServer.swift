import Foundation
import Network
import os

private let log = Logger(subsystem: "com.whimpr.whimprflow", category: "bridge")

/// The app's end of the keyboard bridge (see Shared/BridgeProtocol.swift).
/// One keyboard at a time; a new connection replaces the old one.
@MainActor
final class BridgeServer {
    var onMessage: ((BridgeMessage) -> Void)?
    /// The authorised keyboard went away (keyboard hidden, app switched).
    var onDisconnect: (() -> Void)?
    /// A keyboard presented the right token (called after it's sent `ready`).
    var onAuthorized: (() -> Void)?
    /// Counts authorised connections, so a result can be matched to the
    /// keyboard that asked for it rather than whichever one is open now.
    private(set) var connectionID = 0
    var isConnected: Bool { client != nil && clientAuthorized }
    /// What an authorised keyboard is told first (the app adds its default level).
    var makeReady: () -> BridgeMessage = { BridgeMessage(.ready) }

    private var token: String
    private var listener: NWListener?
    private var client: NWConnection?
    private var clientAuthorized = false
    private var framer = LineFramer()

    init(token: String) {
        self.token = token
    }

    func update(token: String) {
        self.token = token
    }

    func start() throws {
        guard listener == nil else { return }
        let parameters = NWParameters.tcp
        parameters.allowLocalEndpointReuse = true
        // Loopback only: nothing on the Wi-Fi network can reach this.
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: NWEndpoint.Port(rawValue: Bridge.port)!)
        let listener = try NWListener(using: parameters)
        listener.newConnectionHandler = { [weak self] connection in
            MainActor.assumeIsolated { self?.accept(connection) }
        }
        listener.stateUpdateHandler = { state in
            log.info("Bridge listener: \(String(describing: state), privacy: .public)")
        }
        listener.start(queue: .main)
        self.listener = listener
    }

    func stop() {
        client?.cancel()
        client = nil
        clientAuthorized = false
        listener?.cancel()
        listener = nil
    }

    func send(_ message: BridgeMessage) {
        guard let client, clientAuthorized || message.kind == .denied else { return }
        client.send(content: LineFramer.encode(message), completion: .contentProcessed { _ in })
    }

    private func accept(_ connection: NWConnection) {
        if let client {
            client.cancel()
            dropped(client)
        }
        client = connection
        clientAuthorized = false
        framer = LineFramer()
        connection.stateUpdateHandler = { [weak self] state in
            MainActor.assumeIsolated {
                switch state {
                case .failed, .cancelled: self?.dropped(connection)
                default: break
                }
            }
        }
        connection.start(queue: .main)
        receive(on: connection)
    }

    private func receive(on connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, isComplete, error in
            MainActor.assumeIsolated {
                guard let self, connection === self.client else { return }
                if let data {
                    for message in self.framer.append(data) { self.handle(message, from: connection) }
                }
                if isComplete || error != nil {
                    connection.cancel()
                    self.dropped(connection)
                } else {
                    self.receive(on: connection)
                }
            }
        }
    }

    private func handle(_ message: BridgeMessage, from connection: NWConnection) {
        guard clientAuthorized else {
            if message.kind == .hello, message.text == token {
                clientAuthorized = true
                connectionID += 1
                log.info("Keyboard connected")
                send(makeReady())
                onAuthorized?()
            } else {
                log.error("Rejected a bridge client with the wrong token")
                send(BridgeMessage(.denied))
                connection.cancel()
            }
            return
        }
        onMessage?(message)
    }

    private func dropped(_ connection: NWConnection) {
        guard connection === client else { return }
        let wasAuthorized = clientAuthorized
        client = nil
        clientAuthorized = false
        if wasAuthorized {
            log.info("Keyboard disconnected")
            onDisconnect?()
        }
    }
}
