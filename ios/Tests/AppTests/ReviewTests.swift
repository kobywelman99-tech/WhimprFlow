import Foundation
import XCTest
@testable import WhimprFlow

/// Answers cleanup and the sense check differently, and records both.
private final class FakeClaude: CleanupModel, @unchecked Sendable {
    private let lock = NSLock()
    private var _requests: [CleanupRequest] = []
    private let answer: @Sendable (CleanupRequest) throws -> String

    init(_ answer: @escaping @Sendable (CleanupRequest) throws -> String) { self.answer = answer }

    var requests: [CleanupRequest] { lock.withLock { _requests } }

    func clean(_ request: CleanupRequest) async throws -> String {
        lock.withLock { _requests.append(request) }
        return try answer(request)
    }
}

private func isReview(_ request: CleanupRequest) -> Bool { request.system.contains("last check") }

@MainActor
final class ReviewTests: XCTestCase {
    private var keychain: KeychainStore!
    private var settings: CleanupSettings!

    override func setUp() async throws {
        keychain = KeychainStore(service: "com.whimpr.whimprflow.ios.tests.\(UUID().uuidString)")
        settings = CleanupSettings(defaults: UserDefaults(suiteName: "review-tests-\(UUID().uuidString)")!, keychain: keychain)
        try settings.saveAPIKey("sk-test")
    }

    override func tearDown() async throws {
        try? keychain.delete(CleanupSettings.apiKeyAccount)
    }

    private let heard = "yes I'll send you the rapport in an hour"
    private let cleaned = "Yes, I'll send you the rapport in an hour."

    /// Cleanup returns `cleaned`; the sense check returns `review`.
    private func pipeline(review: @escaping @Sendable () throws -> String, withCheck: Bool = true) -> (CleanupPipeline, FakeClaude, FakeClaude) {
        let cleanup = FakeClaude { [cleaned] _ in cleaned }
        let check = FakeClaude { _ in try review() }
        let pipeline = CleanupPipeline(
            settings: settings,
            makeModel: { _ in cleanup },
            makeReviewModel: withCheck ? { _ in check } : nil
        )
        return (pipeline, cleanup, check)
    }

    func testAWordThatDoesNotFitIsRepaired() async {
        let (pipeline, _, _) = pipeline { "<fixed>Yes, I'll send you the report in an hour.</fixed>" }
        let result = await pipeline.clean(heard, context: "Can you send me the report before the meeting?")
        XCTAssertEqual(result.text, "Yes, I'll send you the report in an hour.")
        XCTAssertEqual(result.fixes, [ReviewChange(from: "rapport", to: "report")])
    }

    func testTheCheckSeesTheConversationAndWhatWasHeard() async throws {
        let (pipeline, cleanup, check) = pipeline { "<ok/>" }
        _ = await pipeline.clean(heard, context: "Can you send me the report before the meeting?")
        let request = try XCTUnwrap(check.requests.first)
        XCTAssertTrue(isReview(request))
        let prompt = try XCTUnwrap(request.messages.last?.content)
        XCTAssertTrue(prompt.contains("Can you send me the report before the meeting?"), "the conversation")
        XCTAssertTrue(prompt.contains(heard), "what the recogniser produced")
        XCTAssertTrue(prompt.contains(cleaned), "the message about to be inserted")
        XCTAssertEqual(cleanup.requests.count, 1)
        XCTAssertFalse(cleanup.requests.contains(where: isReview))
    }

    func testAGarbledSentenceIsRebuiltFromWhatWasHeardAndItsNeighboursAreKept() async {
        let heard = "I booked the room already the meeting moved to thursday because the client is out of town"
        let garbled = "I booked the room already. Meeting client town out because the moved Thursday of is."
        let rebuilt = "I booked the room already. The meeting moved to Thursday because the client is out of town."
        let cleanup = FakeClaude { _ in garbled }
        let check = FakeClaude { _ in "<fixed>\(rebuilt)</fixed>" }
        let pipeline = CleanupPipeline(settings: settings, makeModel: { _ in cleanup }, makeReviewModel: { _ in check })

        let result = await pipeline.clean(heard, context: "When can we move the meeting?")

        XCTAssertEqual(result.text, rebuilt)
        XCTAssertEqual(result.fixes.count, 1, "reported as one rebuilt sentence")
        XCTAssertEqual(result.fixes.first?.from, "Meeting client town out because the moved Thursday of is.")
    }

    func testAMessageThatMakesSenseIsLeftAlone() async {
        let (pipeline, _, _) = pipeline { "<ok/>" }
        let result = await pipeline.clean(heard)
        XCTAssertEqual(result.text, cleaned)
        XCTAssertTrue(result.fixes.isEmpty)
    }

    func testAnInventedReplacementIsRefused() async {
        let (pipeline, _, _) = pipeline { "<fixed>Yes, I'll send you the president in an hour.</fixed>" }
        let result = await pipeline.clean(heard)
        XCTAssertEqual(result.text, cleaned)
        XCTAssertTrue(result.fixes.isEmpty)
    }

    func testAFailedCheckNeverBlocksTheMessage() async {
        struct Down: Error {}
        let (pipeline, _, _) = pipeline { throw Down() }
        let result = await pipeline.clean(heard)
        XCTAssertEqual(result.text, cleaned)
        XCTAssertNil(result.note, "a failed bonus check isn't worth telling the user")
    }

    func testTheCheckCanBeTurnedOff() async {
        settings.reviewEnabled = false
        let (pipeline, _, check) = pipeline { "<fixed>Yes, I'll send you the report in an hour.</fixed>" }
        let result = await pipeline.clean(heard)
        XCTAssertEqual(result.text, cleaned)
        XCTAssertTrue(check.requests.isEmpty)
    }

    func testRawLevelIsNotSecondGuessed() async {
        let (pipeline, _, check) = pipeline { "<fixed>Yes, I'll send you the report in an hour.</fixed>" }
        let result = await pipeline.clean(heard, level: .off)
        XCTAssertEqual(result.text, heard)
        XCTAssertTrue(check.requests.isEmpty)
    }

    func testNoCheckWithoutAReviewModel() async {
        let (pipeline, _, check) = pipeline(review: { "<ok/>" }, withCheck: false)
        _ = await pipeline.clean(heard)
        XCTAssertTrue(check.requests.isEmpty)
    }

    func testVeryShortMessagesSkipTheCheck() async {
        let cleanup = FakeClaude { _ in "Okay thanks." }
        let check = FakeClaude { _ in "<ok/>" }
        let pipeline = CleanupPipeline(settings: settings, makeModel: { _ in cleanup }, makeReviewModel: { _ in check })
        _ = await pipeline.clean("okay thanks")
        XCTAssertTrue(check.requests.isEmpty)
    }

    func testTheSettingIsRemembered() {
        let defaults = UserDefaults(suiteName: "review-setting-\(UUID().uuidString)")!
        let first = CleanupSettings(defaults: defaults, keychain: keychain)
        XCTAssertTrue(first.reviewEnabled, "on by default")
        first.reviewEnabled = false
        XCTAssertFalse(CleanupSettings(defaults: defaults, keychain: keychain).reviewEnabled)
    }

    func testTheKeyboardsContextTravelsWithStartAndOldMessagesStillDecode() {
        var framer = LineFramer()
        let sent = BridgeMessage(.start, cleanupLevel: .light, context: "Can you send me the report?\nThanks")
        XCTAssertEqual(framer.append(LineFramer.encode(sent)), [sent])

        // A message from an older keyboard, with no context field.
        let old = Data(#"{"kind":"start","cleanupLevel":"light"}"#.utf8 + [0x0A])
        XCTAssertEqual(framer.append(old).first?.context, nil)
        XCTAssertEqual(framer.append(old).first?.kind, .start)
    }
}
