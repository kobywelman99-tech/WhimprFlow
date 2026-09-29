import Foundation
import XCTest

/// The keyboard's BridgeClient against the app's BridgeServer over a real
/// loopback socket (this bundle compiles both, see project.yml).
@MainActor
final class BridgeTests: XCTestCase {
    private var server: BridgeServer?
    private var client: BridgeClient?

    override func tearDown() async throws {
        client?.disconnect()
        server?.stop()
        // Let the listener release the port before the next test binds it.
        try await Task.sleep(for: .milliseconds(100))
    }

    private func startServer(token: String) throws -> BridgeServer {
        let server = BridgeServer(token: token)
        try server.start()
        self.server = server
        return server
    }

    /// Connects a client and waits for its first event.
    private func connectClient(token: String) async throws -> (BridgeClient, BridgeClient.Event) {
        let client = BridgeClient()
        self.client = client
        let first = expectation(description: "first event")
        var event: BridgeClient.Event?
        client.onEvent = { received in
            if event == nil {
                event = received
                first.fulfill()
            }
        }
        client.connect(token: token)
        await fulfillment(of: [first], timeout: 3)
        return (client, try XCTUnwrap(event))
    }

    func testNoServerMeansNoSession() async throws {
        let (_, event) = try await connectClient(token: "anything")
        XCTAssertEqual(event, .noSession)
    }

    func testRightTokenConnects() async throws {
        _ = try startServer(token: "secret")
        let (client, event) = try await connectClient(token: "secret")
        XCTAssertEqual(event, .ready(nil))
        XCTAssertTrue(client.isConnected)
    }

    func testWrongTokenIsRejected() async throws {
        let server = try startServer(token: "secret")
        var serverGotMessage = false
        server.onMessage = { _ in serverGotMessage = true }

        let (client, event) = try await connectClient(token: "guess")
        XCTAssertEqual(event, .noSession)
        XCTAssertFalse(client.isConnected)
        client.send(BridgeMessage(.start))
        try await Task.sleep(for: .milliseconds(200))
        XCTAssertFalse(serverGotMessage, "a client with the wrong token must not reach the app")
    }

    func testCommandsAndResultsFlowBothWays() async throws {
        let server = try startServer(token: "secret")
        let gotStart = expectation(description: "server got start")
        server.onMessage = { message in
            if message.kind == .start { gotStart.fulfill() }
        }
        let (client, _) = try await connectClient(token: "secret")

        var events: [BridgeClient.Event] = []
        let gotFinal = expectation(description: "client got final")
        client.onEvent = { event in
            events.append(event)
            if case .final = event { gotFinal.fulfill() }
        }

        client.send(BridgeMessage(.start))
        await fulfillment(of: [gotStart], timeout: 2)
        server.send(BridgeMessage(.recording))
        server.send(BridgeMessage(.partial, text: "hello"))
        server.send(BridgeMessage(.transcribing))
        server.send(BridgeMessage(.cleaning))
        server.send(BridgeMessage(.final, text: "Hello there.\nSecond line"))
        await fulfillment(of: [gotFinal], timeout: 2)

        XCTAssertEqual(events, [.recording, .partial("hello"), .transcribing, .cleaning, .final("Hello there.\nSecond line")])
    }

    func testLevelsTravelBothWays() async throws {
        let server = try startServer(token: "secret")
        server.makeReady = { BridgeMessage(.ready, cleanupLevel: .medium) }
        let gotStart = expectation(description: "start")
        var startLevel: DictationLevel?
        server.onMessage = { message in
            if message.kind == .start {
                startLevel = message.cleanupLevel
                gotStart.fulfill()
            }
        }
        let (client, event) = try await connectClient(token: "secret")
        XCTAssertEqual(event, .ready(.medium), "the keyboard learns the app's default")

        var levels: [Double] = []
        let gotLevels = expectation(description: "levels")
        client.onEvent = { event in
            if case .audioLevel(let value) = event {
                levels.append(value)
                if levels.count == 3 { gotLevels.fulfill() }
            }
        }
        client.send(BridgeMessage(.start, cleanupLevel: .high))
        await fulfillment(of: [gotStart], timeout: 2)
        XCTAssertEqual(startLevel, .high, "the pill's level reaches the app")

        for value in [0.1, 0.6, 0.3] { server.send(BridgeMessage(.audioLevel, value: value)) }
        await fulfillment(of: [gotLevels], timeout: 2)
        XCTAssertEqual(levels, [0.1, 0.6, 0.3])
    }

    func testAuthorizedFiresOnlyForTheRightToken() async throws {
        let server = try startServer(token: "secret")
        var authorized = 0
        server.onAuthorized = { authorized += 1 }

        _ = try await connectClient(token: "guess")
        XCTAssertEqual(authorized, 0)
        _ = try await connectClient(token: "secret")
        XCTAssertEqual(authorized, 1)
    }

    func testServerNoticesKeyboardLeaving() async throws {
        let server = try startServer(token: "secret")
        let left = expectation(description: "disconnect")
        server.onDisconnect = { left.fulfill() }
        let (client, _) = try await connectClient(token: "secret")

        client.disconnect()
        await fulfillment(of: [left], timeout: 2)
    }

    func testSessionEndingTellsTheKeyboard() async throws {
        let server = try startServer(token: "secret")
        let (client, _) = try await connectClient(token: "secret")
        let ended = expectation(description: "noSession")
        client.onEvent = { if $0 == .noSession { ended.fulfill() } }

        server.stop()
        await fulfillment(of: [ended], timeout: 2)
    }
}

final class BridgeProtocolTests: XCTestCase {
    func testFramerHandlesSplitAndBatchedLines() {
        var framer = LineFramer()
        let a = LineFramer.encode(BridgeMessage(.partial, text: "one"))
        let b = LineFramer.encode(BridgeMessage(.final, text: "two"))
        let both = a + b

        XCTAssertEqual(framer.append(both.prefix(5)), [])
        XCTAssertEqual(framer.append(both.dropFirst(5)), [BridgeMessage(.partial, text: "one"), BridgeMessage(.final, text: "two")])
    }

    func testFramerSkipsGarbage() {
        var framer = LineFramer()
        XCTAssertEqual(framer.append(Data("not json\n".utf8) + LineFramer.encode(BridgeMessage(.stop))), [BridgeMessage(.stop)])
    }

    func testNewFieldsRoundTripAndAreOptional() throws {
        var framer = LineFramer()
        let message = BridgeMessage(.start, cleanupLevel: .high)
        XCTAssertEqual(framer.append(LineFramer.encode(message)), [message])

        // A message without the new fields (as sent before they existed).
        XCTAssertEqual(framer.append(Data(#"{"kind":"ready"}"# .utf8) + Data([0x0A])), [BridgeMessage(.ready)])
        // Unset fields aren't sent at all.
        let encoded = String(decoding: LineFramer.encode(BridgeMessage(.stop)), as: UTF8.self)
        XCTAssertFalse(encoded.contains("cleanupLevel"))
        XCTAssertFalse(encoded.contains("value"))
    }

    func testPillCyclesThroughLevels() {
        var level = DictationLevel.light
        var seen: [DictationLevel] = []
        for _ in 0..<4 {
            level = level.next
            seen.append(level)
        }
        XCTAssertEqual(seen, [.medium, .high, .off, .light])
        XCTAssertEqual(DictationLevel.off.title, "Raw")
    }

    func testFlowURLRoundTripsToken() throws {
        let token = "A1B2-C3D4"
        XCTAssertEqual(Bridge.token(fromFlowURL: Bridge.flowURL(token: token)), token)
        XCTAssertNil(Bridge.token(fromFlowURL: try XCTUnwrap(URL(string: "whimprflow://other?token=x"))))
        XCTAssertNil(Bridge.token(fromFlowURL: try XCTUnwrap(URL(string: "whimprflow://flow"))))
    }
}

@MainActor
final class DictationInsertTests: XCTestCase {
    func testJoinsDictationOntoExistingText() {
        XCTAssertEqual(KeyboardViewController.joining("Hi there.", after: nil), "Hi there.")
        XCTAssertEqual(KeyboardViewController.joining("Hi there.", after: ""), "Hi there.")
        XCTAssertEqual(KeyboardViewController.joining("see you", after: "Okay"), " see you")
        XCTAssertEqual(KeyboardViewController.joining("see you", after: "Okay "), "see you")
        XCTAssertEqual(KeyboardViewController.joining("see you", after: "Okay\n"), "see you")
        XCTAssertEqual(KeyboardViewController.joining(", right?", after: "Okay"), ", right?")
    }
}
