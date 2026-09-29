import UIKit
import XCTest

/// The mic bar, recording overlay and Undo chip, driven the way bridge events
/// drive them. Commands the keyboard sends are recorded instead of sent.
@MainActor
final class KeyboardUXTests: XCTestCase {
    private var sent: [BridgeMessage] = []

    private func makeKeyboard() -> KeyboardViewController {
        let keyboard = KeyboardViewController()
        keyboard.fullAccess = { true }
        keyboard.send = { [unowned self] in sent.append($0) }
        keyboard.loadViewIfNeeded()
        keyboard.view.frame = CGRect(x: 0, y: 0, width: 393, height: 300)
        keyboard.view.layoutIfNeeded()
        return keyboard
    }

    private func tap(_ button: UIButton) {
        button.sendActions(for: .touchUpInside)
    }

    func testPillStartsAtTheAppDefaultAndCycles() {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(.medium))
        XCTAssertEqual(keyboard.micBar.level, .medium)
        XCTAssertEqual(keyboard.micBar.levelPill.accessibilityValue, "Medium")

        tap(keyboard.micBar.levelPill)
        XCTAssertEqual(keyboard.pillLevel, .high)
        tap(keyboard.micBar.levelPill)
        XCTAssertEqual(keyboard.pillLevel, .off)
        XCTAssertEqual(keyboard.micBar.levelPill.configuration?.title, "Raw")
    }

    func testMicStartsWithThePillLevel() {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(.light))
        tap(keyboard.micBar.levelPill)
        tap(keyboard.micBar.micButton)
        XCTAssertEqual(sent, [BridgeMessage(.start, cleanupLevel: .medium)])
    }

    func testMicStatesAreLabelledForVoiceOver() {
        let keyboard = makeKeyboard()
        let mic = keyboard.micBar.micButton
        XCTAssertEqual(mic.accessibilityLabel, "Start Flow")
        keyboard.handleBridge(.ready(nil))
        XCTAssertEqual(mic.accessibilityLabel, "Dictate")
        keyboard.handleBridge(.recording)
        XCTAssertEqual(mic.accessibilityLabel, "Stop dictating")
        keyboard.handleBridge(.transcribing)
        XCTAssertEqual(mic.accessibilityLabel, "Working")
        XCTAssertFalse(mic.isEnabled, "no double taps while waiting")
    }

    func testOverlayFollowsARecording() {
        let keyboard = makeKeyboard()
        let overlay = keyboard.overlay
        XCTAssertTrue(overlay.isHidden)

        keyboard.handleBridge(.ready(.light))
        keyboard.handleBridge(.recording)
        XCTAssertFalse(overlay.isHidden)
        XCTAssertEqual(overlay.timerLabel.text, "0:00")

        keyboard.handleBridge(.audioLevel(0.8))
        XCTAssertEqual(overlay.waveform.levels.last, 0.8)
        keyboard.handleBridge(.partial("send the deck"))
        XCTAssertEqual(overlay.captionLabel.text, "send the deck")

        keyboard.handleBridge(.transcribing)
        XCTAssertEqual(overlay.captionLabel.text, "Finishing…")
        XCTAssertFalse(overlay.cancelButton.isEnabled, "too late to cancel")
        keyboard.handleBridge(.cleaning)
        XCTAssertEqual(overlay.captionLabel.text, "Polishing…")

        keyboard.handleBridge(.final("Send the deck."))
        XCTAssertEqual(overlay.alpha, 0, "overlay fades out")
        XCTAssertEqual(keyboard.flowState, .ready)
    }

    func testStopAndCancel() {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(nil))
        keyboard.handleBridge(.recording)
        tap(keyboard.micBar.micButton)
        XCTAssertEqual(sent, [BridgeMessage(.stop)])

        sent = []
        keyboard.handleBridge(.final("Hi."))
        keyboard.handleBridge(.recording)
        tap(keyboard.overlay.cancelButton)
        XCTAssertEqual(sent, [BridgeMessage(.cancel)])
        XCTAssertEqual(keyboard.overlay.alpha, 0)
        XCTAssertEqual(keyboard.flowState, .ready)
    }

    func testErrorsAndLostSessionsCloseTheOverlay() {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(nil))
        keyboard.handleBridge(.recording)
        keyboard.handleBridge(.error("Couldn't reach Anthropic"))
        XCTAssertEqual(keyboard.overlay.alpha, 0)
        XCTAssertEqual(keyboard.micBar.statusLabel.text, "Couldn't reach Anthropic")

        keyboard.handleBridge(.recording)
        keyboard.handleBridge(.noSession)
        XCTAssertEqual(keyboard.overlay.alpha, 0)
        XCTAssertEqual(keyboard.micBar.micState, .off)
    }

    func testUndoChipAppearsAfterInsertAndGoesAwayWhenTyping() throws {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(nil))
        keyboard.handleBridge(.recording)
        keyboard.handleBridge(.final("Hello there."))
        XCTAssertFalse(keyboard.micBar.undoButton.isHidden)
        XCTAssertTrue(keyboard.micBar.statusLabel.isHidden)

        let key = try XCTUnwrap(allButtons(in: keyboard.view).first { $0.title(for: .normal) == "a" })
        tap(key)
        XCTAssertTrue(keyboard.micBar.undoButton.isHidden)
    }

    // MARK: Koby's scrolling bug

    func testClosingTheKeyboardMidSentenceDoesNotCancel() {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(nil))
        keyboard.handleBridge(.recording)
        keyboard.viewWillDisappear(false)
        XCTAssertFalse(sent.contains(BridgeMessage(.cancel)), "scrolling a chat hides the keyboard; the words must survive")
    }

    func testAFinishedDictationIsOfferedNotInserted() throws {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(nil))
        keyboard.handleBridge(.pending("Sounds good, I'll call you after lunch."))

        XCTAssertEqual(keyboard.micBar.chip, .insert("Sounds good, I'll call you after lunch."))
        XCTAssertFalse(keyboard.micBar.insertButton.isHidden)
        XCTAssertEqual(keyboard.micBar.insertButton.configuration?.title, "Insert “Sounds good, I'll…”")

        tap(keyboard.micBar.insertButton)
        XCTAssertEqual(sent, [BridgeMessage(.claim)], "the app is told it was used")
        XCTAssertNil(keyboard.pendingText)
        XCTAssertEqual(keyboard.micBar.chip, .undo, "and it can be undone like any dictation")
    }

    func testTypingKeepsTheOfferButDictatingReplacesIt() throws {
        let keyboard = makeKeyboard()
        keyboard.handleBridge(.ready(nil))
        keyboard.handleBridge(.pending("Earlier words"))

        let key = try XCTUnwrap(allButtons(in: keyboard.view).first { $0.title(for: .normal) == "a" })
        tap(key)
        XCTAssertEqual(keyboard.micBar.chip, .insert("Earlier words"), "still there after typing")

        tap(keyboard.micBar.micButton)
        XCTAssertEqual(keyboard.micBar.chip, .none, "a new dictation replaces it (it's in History)")
        XCTAssertEqual(sent.last?.kind, .start)
    }

    func testPreviewIsShort() {
        XCTAssertEqual(MicBar.preview(of: "one two three four five"), "one two three…")
        XCTAssertEqual(MicBar.preview(of: "just two"), "just two")
    }

    func testBarFitsTheSmallestPhone() {
        let keyboard = makeKeyboard()
        keyboard.view.frame.size.width = 320
        keyboard.handleBridge(.ready(.medium))
        keyboard.view.layoutIfNeeded()
        let bar = keyboard.micBar
        for control in [bar.levelPill, bar.micButton] {
            let frame = control.convert(control.bounds, to: keyboard.view)
            XCTAssertTrue(keyboard.view.bounds.contains(frame), "\(control) is cut off at \(frame)")
        }
        XCTAssertGreaterThan(bar.statusLabel.bounds.width, 60, "room left for the status")
    }

    private func allButtons(in view: UIView) -> [UIButton] {
        view.subviews.flatMap { ($0 as? UIButton).map { [$0] } ?? allButtons(in: $0) }
    }
}

final class UndoTrackerTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_000)

    func testUndoDeletesExactlyTheInsert() {
        var tracker = UndoTracker()
        tracker.recordInsert(" Hi there 👋", at: start)
        var deletes = 0
        XCTAssertTrue(tracker.undo(contextBefore: "Okay Hi there 👋", now: start + 2) { deletes += 1 })
        XCTAssertEqual(deletes, 11, "one per character, emoji included")
        XCTAssertFalse(tracker.canUndo(contextBefore: "Okay", now: start + 2), "only once")
    }

    func testNoUndoAfterTheUserEdited() {
        var tracker = UndoTracker()
        tracker.recordInsert("Hi there", at: start)
        var deletes = 0
        XCTAssertFalse(tracker.undo(contextBefore: "Hi there, Sam", now: start + 1) { deletes += 1 })
        XCTAssertEqual(deletes, 0)
    }

    func testUndoExpires() {
        var tracker = UndoTracker()
        tracker.recordInsert("Hi", at: start)
        XCTAssertTrue(tracker.canUndo(contextBefore: "Hi", now: start + UndoTracker.window))
        XCTAssertFalse(tracker.canUndo(contextBefore: "Hi", now: start + UndoTracker.window + 1))
    }

    func testLongInsertWithShortContext() {
        var tracker = UndoTracker()
        let long = String(repeating: "word ", count: 200)
        tracker.recordInsert(long, at: start)
        XCTAssertTrue(tracker.canUndo(contextBefore: String(long.suffix(100)), now: start))
        XCTAssertFalse(tracker.canUndo(contextBefore: "different text", now: start))
        XCTAssertFalse(tracker.canUndo(contextBefore: "", now: start))
    }
}

@MainActor
final class WaveformTests: XCTestCase {
    func testKeepsAFixedNumberOfBarsAndClamps() {
        let waveform = WaveformView()
        for i in 0..<100 { waveform.push(Double(i) / 50) }
        XCTAssertEqual(waveform.levels.count, WaveformView.barCount)
        XCTAssertEqual(waveform.levels.last, 1, "clamped")
        waveform.push(-1)
        XCTAssertEqual(waveform.levels.last, 0)
        waveform.reset()
        XCTAssertEqual(Set(waveform.levels), [0])
    }

    func testTimerFormat() {
        let start = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(RecordingOverlay.elapsed(from: start, to: start + 7.9), "0:07")
        XCTAssertEqual(RecordingOverlay.elapsed(from: start, to: start + 75), "1:15")
    }
}
