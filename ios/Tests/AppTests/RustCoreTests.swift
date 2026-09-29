import XCTest
@testable import WhimprFlow

/// The Rust cleanup pipeline (crates/whimpr-ffi) called from Swift, as the
/// app calls it. Its own behaviour is covered by `cargo test -p whimpr-ffi`;
/// this checks the bridge: linking, argument passing, strings both ways.
final class RustCoreTests: XCTestCase {

    func testPreparesTheDesktopPrompt() {
        let request = prepareCleanup(
            raw: "um so send it to monvi new line thanks",
            level: .light,
            vocab: [Vocab(correct: "Manvi", mishears: ["monvi"])],
            appBundleId: nil,
            styleSummary: nil
        )

        XCTAssertFalse(request.skipModel)
        XCTAssertFalse(request.system.isEmpty)
        XCTAssertEqual(request.messages.last?.role, "user")
        let transcript = request.messages.last?.content ?? ""
        XCTAssertTrue(transcript.contains("Manvi"), "dictionary word reaches the prompt")
        XCTAssertTrue(transcript.contains("[[NL]]"), "spoken line break becomes a marker")
        XCTAssertTrue(request.fallback.contains("\n"), "fallback keeps the line break")
        // Few-shot turns alternate user/assistant before the real transcript.
        XCTAssertEqual(request.messages.count % 2, 1)
    }

    func testOffSkipsTheModel() {
        let request = prepareCleanup(raw: "exactly as said", level: .off, vocab: [], appBundleId: nil, styleSummary: nil)
        XCTAssertTrue(request.skipModel)
        XCTAssertEqual(request.fallback, "exactly as said")
    }

    func testFinishAcceptsAndRejects() {
        let good = finishCleanup(modelOutput: "Send it to Manvi.", fallback: "um send it to Manvi", level: .light)
        XCTAssertTrue(good.accepted)
        XCTAssertEqual(good.text, "Send it to Manvi.")

        let chatty = finishCleanup(modelOutput: "Sure, here's your message: Send it.", fallback: "send it", level: .light)
        XCTAssertFalse(chatty.accepted)
        XCTAssertEqual(chatty.text, "send it")
        XCTAssertNotNil(chatty.rejection)
    }

    func testUnicodeSurvivesTheBridge() {
        let raw = "café at 3 — naïve résumé 👍"
        let request = prepareCleanup(raw: raw, level: .off, vocab: [], appBundleId: nil, styleSummary: nil)
        XCTAssertEqual(request.fallback, raw)
    }

    func testDefaultLevelIsLight() {
        XCTAssertEqual(defaultLevel(), .light)
    }
}
