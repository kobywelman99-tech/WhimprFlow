import Foundation
import XCTest
@testable import WhimprFlow

final class StyleImportTests: XCTestCase {
    func testKeepsOnlyTheAuthorsWords() {
        let email = """
        Hi Sam,

        I don't think we should ship on Friday. Let's give it another week.

        Best,
        Koby

        On Tue, Sep 23, 2026 at 4:12 PM Sam Lee <sam@example.com> wrote:
        > Can we ship Friday?
        """
        XCTAssertEqual(StyleImport.ownWords(email), "I don't think we should ship on Friday. Let's give it another week.")
    }

    func testDropsSignaturesQuotesAndPhoneFooters() {
        XCTAssertEqual(StyleImport.ownWords("Sounds good to me.\n> earlier text\nSent from my iPhone"), "Sounds good to me.")
        XCTAssertEqual(StyleImport.ownWords("Will do tonight.\n--\nKoby Welman\nFounder"), "Will do tonight.")
        XCTAssertEqual(StyleImport.ownWords("Thanks for this.\n---------- Forwarded message ---------\nFrom: someone"), "Thanks for this.")
    }

    func testCutsASignatureThatSharesTheLine() {
        // Email previews collapse lines, so the signature follows the text.
        let preview = "I'd be happy to meet whenever works for you. Many thanks, Jane (JD) Doe Phone Number: 555-123-4567 LinkedIn: www.linkedin.com/in/janedoe"
        XCTAssertEqual(StyleImport.ownWords(preview), "I'd be happy to meet whenever works for you.")
    }

    func testNeverKeepsPhoneNumbers() {
        XCTAssertEqual(StyleImport.ownWords("Call me at 555-123-4567 or (555) 987 6543 tonight."), "Call me at or tonight.")
    }

    func testKeepsSentencesThatMerelyStartLikeAGreeting() {
        // Only a short line of its own is a greeting.
        let text = "Hey so I was thinking we could move the meeting to Thursday afternoon instead"
        XCTAssertEqual(StyleImport.ownWords(text), text)
    }

    func testSplitsAtSeparatorLines() {
        let corpus = """
        Hi Dana,
        I'll send the deck over tonight, it's basically done.
        Thanks,
        Koby
        -----
        Hey,
        Honestly I don't mind either way, it's up to you.
        -----
        ok
        """
        XCTAssertEqual(StyleImport.samples(from: corpus), [
            "I'll send the deck over tonight, it's basically done.",
            "Honestly I don't mind either way, it's up to you.",
        ], "the one-word piece is too short to count")
    }

    func testSplitsPastedTextAtBlankLines() {
        let pasted = "First thought about the launch plan here.\n\nSecond thought, a bit longer than the first one.\n\n\n"
        XCTAssertEqual(StyleImport.samples(from: pasted).count, 2)
    }

    @MainActor
    func testImportFolderIsLearnedThenEmptied() throws {
        let folder = FileManager.default.temporaryDirectory.appending(path: "import-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try "I'll check tonight, it's fine.\n-----\nI don't mind, honestly it's up to you.".write(to: folder.appending(path: "emails.txt"), atomically: true, encoding: .utf8)
        try "not text".write(to: folder.appending(path: "photo.jpg"), atomically: true, encoding: .utf8)
        let style = StyleModel(fileURL: folder.appending(path: "style.json"), defaults: UserDefaults(suiteName: UUID().uuidString)!)

        XCTAssertEqual(try StyleImport.learnFromImportFolder(folder, into: style), 2)
        XCTAssertEqual(style.overview.samples, 2)
        let left = try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted()
        XCTAssertEqual(left, ["photo.jpg", "style.json"], "the email text is deleted once learned; other files are left alone")
        XCTAssertEqual(try StyleImport.learnFromImportFolder(folder, into: style), 0, "nothing learned twice")
    }
}

@MainActor
final class StyleModelTests: XCTestCase {
    private let casual = [
        "so I don't really think we should ship it on Friday, you know",
        "I'm gonna check with Sam, I don't think it's ready yet",
        "honestly it's fine, we don't need another meeting about it",
        "I'll send it over tonight, it's basically done",
        "yeah I don't mind, it's up to you honestly",
    ]

    private func makeModel() -> StyleModel {
        StyleModel(
            fileURL: FileManager.default.temporaryDirectory.appending(path: "style-\(UUID().uuidString)/style.json"),
            defaults: UserDefaults(suiteName: UUID().uuidString)!
        )
    }

    func testLearnsFromDictationsUntilItHasASummary() {
        let model = makeModel()
        for line in casual.dropLast() { model.observe(line) }
        XCTAssertFalse(model.overview.ready)
        XCTAssertNil(model.promptSummary)

        model.observe(casual.last!)
        XCTAssertTrue(model.overview.ready)
        XCTAssertNotNil(model.promptSummary)
    }

    func testTurningLearningOffStopsBothLearningAndUse() {
        let model = makeModel()
        try? model.learn(from: casual)
        XCTAssertNotNil(model.promptSummary)

        model.enabled = false
        XCTAssertNil(model.promptSummary, "cleanup gets no style while off")
        model.observe("I don't think so")
        XCTAssertEqual(Int(model.overview.samples), casual.count, "dictations aren't learned while off")
    }

    func testReset() throws {
        let model = makeModel()
        try model.learn(from: casual)
        try model.reset()
        XCTAssertEqual(model.overview.samples, 0)
        XCTAssertNil(model.promptSummary)
    }

    func testTheSummaryReachesTheCleanupPrompt() async throws {
        let model = makeModel()
        try model.learn(from: casual)
        let summary = try XCTUnwrap(model.promptSummary)

        let keychain = KeychainStore(service: "style-tests.\(UUID().uuidString)")
        defer { try? keychain.delete(CleanupSettings.apiKeyAccount) }
        let settings = CleanupSettings(defaults: UserDefaults(suiteName: UUID().uuidString)!, keychain: keychain)
        try settings.saveAPIKey("sk-test")
        let recorder = RequestRecorder()
        let pipeline = CleanupPipeline(settings: settings, styleSummary: { model.promptSummary }, makeModel: { _ in recorder })

        _ = await pipeline.clean("um so I'll send it tonight")
        let request = try XCTUnwrap(recorder.last)
        let everything = ([request.system] + request.messages.map(\.content)).joined(separator: "\n")
        XCTAssertTrue(everything.contains(summary))
    }
}

private final class RequestRecorder: CleanupModel, @unchecked Sendable {
    private let lock = NSLock()
    private var request: CleanupRequest?
    var last: CleanupRequest? { lock.withLock { request } }

    func clean(_ request: CleanupRequest) async throws -> String {
        lock.withLock { self.request = request }
        return "I'll send it tonight."
    }
}
