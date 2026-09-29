import Foundation
import XCTest
@testable import WhimprFlow

@MainActor
final class DictionaryModelTests: XCTestCase {
    private var fileURL: URL!

    override func setUp() async throws {
        fileURL = FileManager.default.temporaryDirectory
            .appending(path: "dict-\(UUID().uuidString)")
            .appending(path: "dictionary.json")
    }

    func testAddSplitsHeardAsAndPersists() throws {
        let model = DictionaryModel(fileURL: fileURL)
        XCTAssertTrue(model.entries.isEmpty)

        try model.add(word: "Manvi", heardAs: "monvi, manvee ,")

        XCTAssertEqual(model.entries, [DictEntry(correct: "Manvi", mishears: ["monvi", "manvee"], learned: false)])
        XCTAssertEqual(DictionaryModel(fileURL: fileURL).entries, model.entries, "saved to disk")
    }

    func testRemove() throws {
        let model = DictionaryModel(fileURL: fileURL)
        try model.add(word: "Manvi")
        try model.add(word: "ChargeBee")
        try model.remove("manvi")
        XCTAssertEqual(model.entries.map(\.correct), ["ChargeBee"])
    }

    func testEmptyWordIsRejected() {
        let model = DictionaryModel(fileURL: fileURL)
        XCTAssertThrowsError(try model.add(word: "  ")) { error in
            XCTAssertEqual(error.localizedDescription, "Enter a word to add.")
        }
    }

    func testSaveFailureIsReadable() throws {
        // A file where the folder should be makes saving fail.
        let blocker = FileManager.default.temporaryDirectory.appending(path: "blocker-\(UUID().uuidString)")
        try Data().write(to: blocker)
        let model = DictionaryModel(fileURL: blocker.appending(path: "dictionary.json"))
        XCTAssertThrowsError(try model.add(word: "Manvi")) { error in
            XCTAssertTrue(error.localizedDescription.hasPrefix("Couldn't save the dictionary:"), error.localizedDescription)
        }
    }

    func testRelevantAndWords() throws {
        let model = DictionaryModel(fileURL: fileURL)
        try model.add(word: "Manvi", heardAs: "monvi")
        try model.add(word: "ChargeBee")

        XCTAssertEqual(model.relevant(to: "send it to monvi").map(\.correct), ["Manvi"])
        XCTAssertEqual(model.words, ["Manvi", "ChargeBee"])
    }
}

/// The dictionary reaching the cleanup prompt.
@MainActor
final class DictionaryCleanupTests: XCTestCase {
    func testRelevantWordsReachThePrompt() async throws {
        let keychain = KeychainStore(service: "dict-tests.\(UUID().uuidString)")
        defer { try? keychain.delete(CleanupSettings.apiKeyAccount) }
        let settings = CleanupSettings(defaults: UserDefaults(suiteName: UUID().uuidString)!, keychain: keychain)
        try settings.saveAPIKey("sk-test")
        let dictionary = DictionaryModel(fileURL: FileManager.default.temporaryDirectory
            .appending(path: "dict-\(UUID().uuidString)/dictionary.json"))
        try dictionary.add(word: "Manvi", heardAs: "monvi")
        try dictionary.add(word: "ChargeBee")

        let sent = PromptRecorder()
        let pipeline = CleanupPipeline(
            settings: settings,
            vocabulary: { dictionary.relevant(to: $0) },
            makeModel: { _ in sent }
        )
        _ = await pipeline.clean("send the deck to monvi tomorrow")

        let prompt = try XCTUnwrap(sent.lastPrompt)
        XCTAssertTrue(prompt.contains("Manvi"), "relevant word is in the prompt")
        XCTAssertFalse(prompt.contains("ChargeBee"), "unrelated words are left out")
    }
}

private final class PromptRecorder: CleanupModel, @unchecked Sendable {
    private let lock = NSLock()
    private var prompt: String?
    var lastPrompt: String? { lock.withLock { prompt } }

    func clean(_ request: CleanupRequest) async throws -> String {
        lock.withLock { prompt = request.messages.last?.content }
        return "Send the deck to Manvi tomorrow."
    }
}
