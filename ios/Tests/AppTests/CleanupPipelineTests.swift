import Foundation
import XCTest
@testable import WhimprFlow

/// A stand-in for Claude that records what it was sent.
private final class FakeModel: CleanupModel, @unchecked Sendable {
    private let lock = NSLock()
    private var _requests: [CleanupRequest] = []
    private let answer: @Sendable (CleanupRequest) throws -> String

    init(answer: @escaping @Sendable (CleanupRequest) throws -> String) {
        self.answer = answer
    }

    var requests: [CleanupRequest] { lock.withLock { _requests } }

    func clean(_ request: CleanupRequest) async throws -> String {
        lock.withLock { _requests.append(request) }
        return try answer(request)
    }
}

@MainActor
final class CleanupPipelineTests: XCTestCase {
    private var keychain: KeychainStore!
    private var defaults: UserDefaults!

    override func setUp() async throws {
        keychain = KeychainStore(service: "com.whimpr.whimprflow.ios.tests.\(UUID().uuidString)")
        defaults = UserDefaults(suiteName: "cleanup-tests-\(UUID().uuidString)")
    }

    override func tearDown() async throws {
        try? keychain.delete(CleanupSettings.apiKeyAccount)
    }

    private func pipeline(key: String? = "sk-test", model: FakeModel) throws -> (CleanupPipeline, CleanupSettings) {
        let settings = CleanupSettings(defaults: defaults, keychain: keychain)
        if let key { try settings.saveAPIKey(key) }
        return (CleanupPipeline(settings: settings, makeModel: { _ in model }), settings)
    }

    func testCleansWithTheModel() async throws {
        let model = FakeModel { _ in "I think we should ship it on Friday." }
        let (pipeline, _) = try pipeline(model: model)

        let result = await pipeline.clean("um I think we should uh ship it on Friday")

        XCTAssertEqual(result, CleanupResult(text: "I think we should ship it on Friday.", cleaned: true))
        let sent = try XCTUnwrap(model.requests.first)
        XCTAssertFalse(sent.system.isEmpty)
        XCTAssertTrue(sent.messages.last?.content.contains("ship it on Friday") ?? false)
    }

    func testChattyAnswerFallsBackToYourWords() async throws {
        let model = FakeModel { _ in "Sure! Here's a cleaner version: The meeting is at 3pm." }
        let (pipeline, _) = try pipeline(model: model)

        let result = await pipeline.clean("what time is the meeting")

        XCTAssertEqual(result.text, "what time is the meeting")
        XCTAssertFalse(result.cleaned)
        XCTAssertNotNil(result.note)
    }

    func testModelFailureFallsBackToYourWords() async throws {
        let model = FakeModel { _ in throw AnthropicClient.ClientError.network("offline") }
        let (pipeline, _) = try pipeline(model: model)

        let result = await pipeline.clean("send the report new line thanks")

        XCTAssertEqual(result.text, "send the report\nthanks", "raw fallback still honours spoken line breaks")
        XCTAssertFalse(result.cleaned)
        XCTAssertEqual(result.note, "Couldn't reach Anthropic: offline")
    }

    func testNoKeyMeansNoCallAndATip() async throws {
        let model = FakeModel { _ in "unused" }
        let (pipeline, _) = try pipeline(key: nil, model: model)

        let result = await pipeline.clean("hello there")

        XCTAssertEqual(result.text, "hello there")
        XCTAssertFalse(result.cleaned)
        XCTAssertNotNil(result.note)
        XCTAssertTrue(model.requests.isEmpty)
    }

    func testTurnedOffMeansNoCall() async throws {
        let model = FakeModel { _ in "unused" }
        let (pipeline, settings) = try pipeline(model: model)
        settings.enabled = false

        let result = await pipeline.clean("um exactly this")

        XCTAssertEqual(result, CleanupResult(text: "um exactly this", cleaned: false))
        XCTAssertTrue(model.requests.isEmpty)
    }

    func testLevelOffMeansNoCall() async throws {
        let model = FakeModel { _ in "unused" }
        let (pipeline, settings) = try pipeline(model: model)
        settings.level = .off

        let result = await pipeline.clean("um exactly this")

        XCTAssertEqual(result.text, "um exactly this")
        XCTAssertTrue(model.requests.isEmpty)
    }

    func testSettingsPersist() throws {
        let settings = CleanupSettings(defaults: defaults, keychain: keychain)
        XCTAssertTrue(settings.enabled)
        XCTAssertEqual(settings.level, .light)
        XCTAssertFalse(settings.hasAPIKey)

        settings.enabled = false
        settings.level = .high
        try settings.saveAPIKey("  sk-ant-123\n")

        let reloaded = CleanupSettings(defaults: defaults, keychain: keychain)
        XCTAssertFalse(reloaded.enabled)
        XCTAssertEqual(reloaded.level, .high)
        XCTAssertTrue(reloaded.hasAPIKey)
        XCTAssertEqual(reloaded.apiKey, "sk-ant-123", "pasted whitespace is trimmed")

        try reloaded.removeAPIKey()
        XCTAssertFalse(CleanupSettings(defaults: defaults, keychain: keychain).hasAPIKey)
    }
}

final class KeychainStoreTests: XCTestCase {
    private let store = KeychainStore(service: "com.whimpr.whimprflow.ios.tests.keychain")

    override func tearDown() {
        try? store.delete("account")
    }

    func testRoundTripOverwriteDelete() throws {
        XCTAssertNil(store.read("account"))
        try store.save("first", for: "account")
        XCTAssertEqual(store.read("account"), "first")
        try store.save("second", for: "account")
        XCTAssertEqual(store.read("account"), "second")
        try store.delete("account")
        XCTAssertNil(store.read("account"))
        XCTAssertNoThrow(try store.delete("account"), "deleting twice is fine")
    }
}
