import Foundation
import XCTest
@testable import WhimprFlow

final class PendingDictationTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_000)

    func testKeptForTenMinutes() {
        var pending = PendingDictation()
        XCTAssertNil(pending.text(now: start))
        pending.store("Kept", at: start)
        XCTAssertEqual(pending.text(now: start + PendingDictation.lifetime), "Kept")
        XCTAssertNil(pending.text(now: start + PendingDictation.lifetime + 1), "stale after that (it's in History)")
    }

    func testClear() {
        var pending = PendingDictation()
        pending.store("Kept", at: start)
        pending.clear()
        XCTAssertNil(pending.text(now: start))
    }

    @MainActor
    func testResultsGoOnlyToTheKeyboardThatAsked() {
        XCTAssertEqual(FlowController.delivery(requestedBy: 3, connected: 3), .insert)
        XCTAssertEqual(FlowController.delivery(requestedBy: 3, connected: nil), .keepForLater, "keyboard closed (scrolled away)")
        XCTAssertEqual(FlowController.delivery(requestedBy: 3, connected: 4), .keepForLater, "keyboard reopened, maybe in another chat")
        XCTAssertEqual(FlowController.delivery(requestedBy: nil, connected: 4), .keepForLater)
    }
}
