import Foundation
import XCTest
@testable import WhimprFlow

/// AnthropicClient against a stubbed network. Never calls the real API.
final class AnthropicClientTests: XCTestCase {

    private let request = CleanupRequest(
        skipModel: false,
        system: "SYSTEM PROMPT",
        messages: [
            ChatMessage(role: "user", content: "<USER_MESSAGE>\nexample\n</USER_MESSAGE>"),
            ChatMessage(role: "assistant", content: "Example."),
            ChatMessage(role: "user", content: "<USER_MESSAGE>\num hello there\n</USER_MESSAGE>"),
        ],
        fallback: "um hello there"
    )

    override func setUp() {
        StubNetwork.reset()
    }

    private func client() -> AnthropicClient {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubNetwork.self]
        return AnthropicClient(apiKey: "sk-test", session: URLSession(configuration: config))
    }

    private func reply(_ text: String, stop: String = "end_turn") -> Data {
        Data(#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":\#(String(reflecting: text))}],"stop_reason":"\#(stop)"}"#.utf8)
    }

    // MARK: Request

    func testRequestMatchesTheDesktopProvider() throws {
        let urlRequest = try client().makeURLRequest(request)

        XCTAssertEqual(urlRequest.url, URL(string: "https://api.anthropic.com/v1/messages"))
        XCTAssertEqual(urlRequest.httpMethod, "POST")
        XCTAssertEqual(urlRequest.value(forHTTPHeaderField: "x-api-key"), "sk-test")
        XCTAssertEqual(urlRequest.value(forHTTPHeaderField: "anthropic-version"), "2023-06-01")
        XCTAssertEqual(urlRequest.value(forHTTPHeaderField: "content-type"), "application/json")

        let body = try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(urlRequest.httpBody)) as? [String: Any])
        XCTAssertEqual(body["model"] as? String, "claude-haiku-4-5")
        XCTAssertEqual(body["temperature"] as? Double, 0.2)
        XCTAssertEqual(body["system"] as? String, "SYSTEM PROMPT")
        let messages = try XCTUnwrap(body["messages"] as? [[String: String]])
        XCTAssertEqual(messages.map { $0["role"] }, ["user", "assistant", "user"])
        XCTAssertEqual(messages.last?["content"], "<USER_MESSAGE>\num hello there\n</USER_MESSAGE>")
    }

    // MARK: Responses

    func testReturnsTheText() async throws {
        StubNetwork.responses = [(200, reply("Hello there."))]
        let text = try await client().clean(request)
        XCTAssertEqual(text, "Hello there.")
    }

    func testJoinsMultipleTextBlocks() throws {
        let data = Data(#"{"content":[{"type":"text","text":"Hello "},{"type":"text","text":"there."}],"stop_reason":"end_turn"}"#.utf8)
        XCTAssertEqual(try AnthropicClient.parse(data: data, status: 200), "Hello there.")
    }

    func testBadKeyIsReportedAndNotRetried() async {
        StubNetwork.responses = [(401, Data(#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.utf8))]
        await assertThrows(.unauthorized)
        XCTAssertEqual(StubNetwork.requestCount, 1)
    }

    func testAPIErrorMessageIsKept() async {
        StubNetwork.responses = [(400, Data(#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: too long"}}"#.utf8))]
        await assertThrows(.http(status: 400, message: "messages: too long"))
    }

    func testRetriesOnceWhenOverloaded() async throws {
        StubNetwork.responses = [
            (529, Data(#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#.utf8)),
            (200, reply("Worked the second time.")),
        ]
        let text = try await client().clean(request)
        XCTAssertEqual(text, "Worked the second time.")
        XCTAssertEqual(StubNetwork.requestCount, 2)
    }

    func testGivesUpAfterOneRetry() async {
        let overloaded = Data(#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#.utf8)
        StubNetwork.responses = [(529, overloaded), (529, overloaded), (529, overloaded)]
        await assertThrows(.http(status: 529, message: "Overloaded"))
        XCTAssertEqual(StubNetwork.requestCount, 2)
    }

    func testCutOffAnswerIsRejected() async {
        StubNetwork.responses = [(200, reply("Half a sent", stop: "max_tokens"))]
        await assertThrows(.truncated)
    }

    func testRefusalIsRejected() async {
        StubNetwork.responses = [(200, reply("", stop: "refusal"))]
        await assertThrows(.refused)
    }

    func testOfflineIsANetworkError() async {
        StubNetwork.failure = URLError(.notConnectedToInternet)
        do {
            _ = try await client().clean(request)
            XCTFail("expected a network error")
        } catch let error as AnthropicClient.ClientError {
            guard case .network = error else { return XCTFail("got \(error)") }
        } catch {
            XCTFail("unexpected \(error)")
        }
    }

    private func assertThrows(_ expected: AnthropicClient.ClientError, file: StaticString = #filePath, line: UInt = #line) async {
        do {
            _ = try await client().clean(request)
            XCTFail("expected \(expected)", file: file, line: line)
        } catch {
            XCTAssertEqual(error as? AnthropicClient.ClientError, expected, file: file, line: line)
        }
    }
}

/// Canned HTTP responses, served in order.
final class StubNetwork: URLProtocol {
    nonisolated(unsafe) static var responses: [(Int, Data)] = []
    nonisolated(unsafe) static var failure: Error?
    nonisolated(unsafe) static var requestCount = 0
    private static let lock = NSLock()

    static func reset() {
        lock.withLock {
            responses = []
            failure = nil
            requestCount = 0
        }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        let (failure, next): (Error?, (Int, Data)?) = Self.lock.withLock {
            Self.requestCount += 1
            return (Self.failure, Self.responses.isEmpty ? nil : Self.responses.removeFirst())
        }
        if let failure {
            client?.urlProtocol(self, didFailWithError: failure)
            return
        }
        let (status, data) = next ?? (500, Data())
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
}
