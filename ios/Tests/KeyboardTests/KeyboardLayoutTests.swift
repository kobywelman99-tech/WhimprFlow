import UIKit
import XCTest

/// Loads the keyboard the way iOS does and walks every page, so a layout bug
/// fails here instead of crashing the keyboard on the phone.
@MainActor
final class KeyboardLayoutTests: XCTestCase {

    private func makeKeyboard(width: CGFloat = 393) -> KeyboardViewController {
        let keyboard = KeyboardViewController()
        keyboard.loadViewIfNeeded()
        keyboard.view.frame = CGRect(x: 0, y: 0, width: width, height: 300)
        keyboard.view.layoutIfNeeded()
        return keyboard
    }

    private func buttons(in view: UIView) -> [UIButton] {
        view.subviews.flatMap { ($0 as? UIButton).map { [$0] } ?? buttons(in: $0) }
    }

    private func button(titled title: String, in view: UIView) -> UIButton? {
        buttons(in: view).first { $0.title(for: .normal) == title }
    }

    private func assertKeysHaveSize(_ view: UIView, file: StaticString = #filePath, line: UInt = #line) {
        for key in buttons(in: view) {
            let name = key.title(for: .normal) ?? key.accessibilityLabel ?? "?"
            XCTAssertGreaterThan(key.bounds.width, 0, "key \(name) has no width", file: file, line: line)
            XCTAssertGreaterThan(key.bounds.height, 0, file: file, line: line)
            let frame = key.convert(key.bounds, to: view)
            XCTAssertTrue(view.bounds.insetBy(dx: -0.5, dy: -0.5).contains(frame), "key \(name) is cut off at \(frame)", file: file, line: line)
        }
    }

    func testLettersPageLoads() {
        let keyboard = makeKeyboard()
        let letters = buttons(in: keyboard.view).compactMap { $0.title(for: .normal) }.filter { $0.count == 1 && $0.first!.isLetter }
        XCTAssertEqual(letters.count, 26)
        assertKeysHaveSize(keyboard.view)
    }

    func testEveryPageLoads() throws {
        let keyboard = makeKeyboard()
        for title in ["123", "#+=", "123", "ABC"] {
            let key = try XCTUnwrap(button(titled: title, in: keyboard.view), "no \(title) key")
            key.sendActions(for: .touchUpInside)
            keyboard.view.layoutIfNeeded()
            assertKeysHaveSize(keyboard.view)
        }
    }

    func testFitsSmallestPhoneWidth() {
        assertKeysHaveSize(makeKeyboard(width: 320).view)
    }
}
