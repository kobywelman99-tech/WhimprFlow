import Foundation
import XCTest
@testable import WhimprFlow

@MainActor
final class DictationHistoryTests: XCTestCase {
    private var fileURL: URL!

    override func setUp() async throws {
        fileURL = FileManager.default.temporaryDirectory
            .appending(path: "history-\(UUID().uuidString)")
            .appending(path: "history.json")
    }

    private func record(_ raw: String, _ text: String, source: DictationRecord.Source = .keyboard) -> DictationRecord {
        DictationRecord(date: Date(timeIntervalSince1970: 1_790_000_000), raw: raw, text: text, source: source, level: .light, cleaned: raw != text)
    }

    func testKeepsNewestFirstAcrossRelaunches() {
        let history = DictationHistory(fileURL: fileURL)
        history.add(record("um first", "First."))
        history.add(record("second one", "Second one.", source: .app))

        let reloaded = DictationHistory(fileURL: fileURL)
        XCTAssertEqual(reloaded.records.map(\.text), ["Second one.", "First."])
        XCTAssertEqual(reloaded.records.last?.raw, "um first", "what was said is kept too")
        XCTAssertEqual(reloaded.records.first?.source, .app)
        XCTAssertEqual(reloaded.records, history.records)
    }

    func testDeleteAndClear() {
        let history = DictationHistory(fileURL: fileURL)
        history.add(record("a", "A."))
        history.add(record("b", "B."))
        history.delete(history.records[0].id)
        XCTAssertEqual(DictationHistory(fileURL: fileURL).records.map(\.text), ["A."])
        history.clear()
        XCTAssertTrue(DictationHistory(fileURL: fileURL).records.isEmpty)
    }

    func testEmptyDictationsAreNotKept() {
        let history = DictationHistory(fileURL: fileURL)
        history.add(record("  ", ""))
        XCTAssertTrue(history.records.isEmpty)
    }

    func testAnUnreadableFileIsSetAsideNotLost() throws {
        try FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("{ not json".utf8).write(to: fileURL)

        let history = DictationHistory(fileURL: fileURL)
        XCTAssertTrue(history.records.isEmpty)
        history.add(record("new", "New."))

        let folder = try FileManager.default.contentsOfDirectory(atPath: fileURL.deletingLastPathComponent().path)
        let setAside = try XCTUnwrap(folder.first { $0.contains("unreadable") })
        let kept = try String(contentsOf: fileURL.deletingLastPathComponent().appending(path: setAside), encoding: .utf8)
        XCTAssertEqual(kept, "{ not json", "the old file's contents are preserved")
    }
}
