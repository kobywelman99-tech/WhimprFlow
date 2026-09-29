import UIKit

/// WhimprFlow's keyboard. It stays deliberately small: iOS gives keyboard
/// extensions a tight memory budget and no microphone, so dictation runs in the
/// host app. The mic button opens the app to start a Flow session the first
/// time, then drives dictation over the loopback bridge (BridgeClient).
final class KeyboardViewController: UIInputViewController {

    private enum Page { case letters, numbers, symbols }

    private enum Key {
        case character(String)
        case shift, delete, space, returnKey, globe
        case page(Page)
    }

    /// One key on screen. `width` is a fraction of the row width; nil keys in a
    /// row share whatever space is left.
    private struct KeySpec {
        let key: Key
        let width: CGFloat?
        init(_ key: Key, width: CGFloat? = 0.1) {
            self.key = key
            self.width = width
        }
    }

    private final class KeyButton: UIButton {
        var key: Key = .space
    }

    private static let keyHeight: CGFloat = 42
    private static let keySpacing: CGFloat = 6
    private static let characterColor = UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: 0.42, alpha: 1) : .white }
    private static let functionColor = UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: 0.27, alpha: 1) : UIColor(red: 0.67, green: 0.69, blue: 0.73, alpha: 1) }

    private var page: Page = .letters
    private var shifted = false
    private var showsGlobe = true

    private let rowsStack = UIStackView()
    let micBar = MicBar()
    let overlay = RecordingOverlay()
    private var keyButtons: [KeyButton] = []
    private var deleteTimer: Timer?

    enum FlowState { case off, connecting, ready, recording, transcribing }
    private(set) var flowState: FlowState = .off
    private let bridge = BridgeClient()
    /// Where commands go; tests swap in a recorder.
    lazy var send: (BridgeMessage) -> Void = { [bridge] in bridge.send($0) }
    /// Whether the user allowed Full Access (needed for the bridge); tests override.
    lazy var fullAccess: () -> Bool = { [unowned self] in self.hasFullAccess }
    private var undo = UndoTracker()
    /// A dictation that finished after this keyboard closed, offered by the app.
    private(set) var pendingText: String?
    private var undoTimer: Timer?
    /// The mic was tapped before we knew whether Flow is on: start dictating
    /// if it is, open the app if it isn't.
    private var micPending = false
    /// The style pill: this keyboard session's cleanup level. Starts at the
    /// app's default each time the keyboard connects.
    private(set) var pillLevel: DictationLevel = .light {
        didSet { micBar.level = pillLevel }
    }

    /// Handed to the app in the URL that starts Flow; the app only talks to a
    /// keyboard that presents it. Lives in the keyboard's own defaults.
    private var bridgeToken: String {
        let key = "bridgeToken"
        if let token = UserDefaults.standard.string(forKey: key) { return token }
        let token = UUID().uuidString + UUID().uuidString
        UserDefaults.standard.set(token, forKey: key)
        return token
    }

    // MARK: Lifecycle

    override func viewDidLoad() {
        super.viewDidLoad()
        buildChrome()
        showsGlobe = needsInputModeSwitchKey
        rebuildKeys()
        bridge.onEvent = { [weak self] event in self?.handleBridge(event) }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        if showsGlobe != needsInputModeSwitchKey {
            showsGlobe = needsInputModeSwitchKey
            rebuildKeys()
        }
        refreshAutoShift()
        // Find out now whether Flow is on, so the mic responds instantly.
        if fullAccess(), flowState == .off {
            flowState = .connecting
            bridge.connect(token: bridgeToken)
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // No cancel here: this also happens when scrolling a chat hides the
        // keyboard mid-sentence. The app finishes the dictation and offers it
        // the next time the keyboard opens. (✕ still cancels.)
        bridge.disconnect()
        flowState = .off
        micPending = false
        overlay.hide()
        hideUndo()
        pendingText = nil
        updateChip()
        updateMicButton()
    }

    override func textDidChange(_ textInput: UITextInput?) {
        super.textDidChange(textInput)
        refreshAutoShift()
    }

    // MARK: Layout

    private func buildChrome() {
        let height = view.heightAnchor.constraint(equalToConstant: 4 * Self.keyHeight + 3 * 10 + MicBar.height + 4)
        height.priority = UILayoutPriority(999)
        height.isActive = true

        micBar.setStatus("WhimprFlow")
        micBar.level = pillLevel
        micBar.onMic = { [weak self] in self?.micTapped() }
        micBar.onLevel = { [weak self] in self?.cycleLevel() }
        micBar.onUndo = { [weak self] in self?.undoDictation() }
        micBar.onInsertPending = { [weak self] in self?.insertPending() }

        rowsStack.axis = .vertical
        rowsStack.spacing = 10
        rowsStack.distribution = .fillEqually

        let root = UIStackView(arrangedSubviews: [micBar, rowsStack])
        root.axis = .vertical
        root.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(root)

        // Over the keys, not the bar, so the mic stays reachable.
        overlay.onCancel = { [weak self] in self?.cancelDictation() }
        overlay.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(overlay)

        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 3),
            root.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -3),
            root.topAnchor.constraint(equalTo: view.topAnchor),
            root.bottomAnchor.constraint(equalTo: view.bottomAnchor, constant: -4),
            overlay.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            overlay.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            overlay.topAnchor.constraint(equalTo: rowsStack.topAnchor),
            overlay.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
    }

    private func rows(for page: Page) -> [[KeySpec]] {
        let chars: (String) -> [KeySpec] = { $0.split(separator: " ").map { KeySpec(.character(String($0))) } }
        var bottom: [KeySpec] = [KeySpec(.page(page == .letters ? .numbers : .letters), width: 0.13)]
        if showsGlobe { bottom.append(KeySpec(.globe, width: 0.11)) }
        bottom += [KeySpec(.space, width: nil), KeySpec(.returnKey, width: 0.24)]

        switch page {
        case .letters:
            return [
                chars("q w e r t y u i o p"),
                chars("a s d f g h j k l"),
                [KeySpec(.shift, width: nil)] + chars("z x c v b n m") + [KeySpec(.delete, width: nil)],
                bottom,
            ]
        case .numbers, .symbols:
            let top = page == .numbers ? "1 2 3 4 5 6 7 8 9 0" : "[ ] { } # % ^ * + ="
            let middle = page == .numbers ? "- / : ; ( ) $ & @ \"" : "_ \\ | ~ < > € £ ¥ •"
            let toggle = KeySpec(.page(page == .numbers ? .symbols : .numbers), width: nil)
            let punctuation = ". , ? ! '".split(separator: " ").map { KeySpec(.character(String($0)), width: 0.13) }
            return [chars(top), chars(middle), [toggle] + punctuation + [KeySpec(.delete, width: nil)], bottom]
        }
    }

    private func rebuildKeys() {
        rowsStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        keyButtons.removeAll()

        for specs in rows(for: page) {
            let row = UIStackView()
            row.axis = .horizontal
            row.spacing = Self.keySpacing
            // Add the row first: key widths are pinned to rowsStack, and
            // activating a constraint between views in different hierarchies
            // throws (this crashed the keyboard on launch).
            rowsStack.addArrangedSubview(row)
            let flexible = specs.contains { $0.width == nil }
            var firstFlexible: UIView?

            // Rows without a stretchy key get equal spacers so they stay centred.
            let leading = UIView(), trailing = UIView()
            if !flexible { row.addArrangedSubview(leading) }

            for spec in specs {
                let button = makeButton(for: spec.key)
                row.addArrangedSubview(button)
                keyButtons.append(button)
                if let width = spec.width {
                    button.widthAnchor.constraint(
                        equalTo: rowsStack.widthAnchor,
                        multiplier: width,
                        constant: -Self.keySpacing * 9 * width
                    ).isActive = true
                } else if let first = firstFlexible {
                    button.widthAnchor.constraint(equalTo: first.widthAnchor).isActive = true
                } else {
                    firstFlexible = button
                }
            }

            if !flexible {
                // The spacers must not add gaps of their own, or a full row
                // of ten keys overflows the keyboard.
                row.setCustomSpacing(0, after: leading)
                if let last = row.arrangedSubviews.last { row.setCustomSpacing(0, after: last) }
                row.addArrangedSubview(trailing)
                trailing.widthAnchor.constraint(equalTo: leading.widthAnchor).isActive = true
            }
        }
        updateKeyLabels()
    }

    private func makeButton(for key: Key) -> KeyButton {
        let button = KeyButton(type: .custom)
        button.key = key
        button.layer.cornerRadius = 5
        button.layer.shadowColor = UIColor.black.cgColor
        button.layer.shadowOpacity = 0.3
        button.layer.shadowOffset = CGSize(width: 0, height: 1)
        button.layer.shadowRadius = 0
        button.setTitleColor(.label, for: .normal)
        button.tintColor = .label

        switch key {
        case .character:
            button.backgroundColor = Self.characterColor
            button.titleLabel?.font = .systemFont(ofSize: 22)
        case .space:
            button.backgroundColor = Self.characterColor
            button.titleLabel?.font = .systemFont(ofSize: 16)
        default:
            button.backgroundColor = Self.functionColor
            button.titleLabel?.font = .systemFont(ofSize: 16)
        }

        switch key {
        case .globe:
            button.setImage(UIImage(systemName: "globe"), for: .normal)
            button.accessibilityLabel = "Next keyboard"
            button.addTarget(self, action: #selector(handleInputModeList(from:with:)), for: .allTouchEvents)
        case .delete:
            button.setImage(UIImage(systemName: "delete.left"), for: .normal)
            button.accessibilityLabel = "Delete"
            button.addTarget(self, action: #selector(deleteDown), for: .touchDown)
            button.addTarget(self, action: #selector(deleteUp), for: [.touchUpInside, .touchUpOutside, .touchCancel])
        default:
            button.addTarget(self, action: #selector(keyTapped(_:)), for: .touchUpInside)
        }
        return button
    }

    private func updateKeyLabels() {
        for button in keyButtons {
            switch button.key {
            case .character(let c):
                button.setTitle(page == .letters && shifted ? c.uppercased() : c, for: .normal)
            case .shift:
                button.setImage(UIImage(systemName: shifted ? "shift.fill" : "shift"), for: .normal)
                button.accessibilityLabel = "Shift"
            case .space:
                button.setTitle("space", for: .normal)
            case .returnKey:
                button.setTitle("return", for: .normal)
            case .page(let target):
                button.setTitle(target == .letters ? "ABC" : target == .numbers ? "123" : "#+=", for: .normal)
            case .delete, .globe:
                break
            }
        }
    }

    // MARK: Input

    @objc private func keyTapped(_ sender: KeyButton) {
        hideUndo()
        let proxy = textDocumentProxy
        switch sender.key {
        case .character(let c):
            proxy.insertText(page == .letters && shifted ? c.uppercased() : c)
            if shifted { setShifted(false) }
        case .space:
            insertSpace()
        case .returnKey:
            proxy.insertText("\n")
        case .shift:
            setShifted(!shifted)
        case .page(let target):
            page = target
            rebuildKeys()
            refreshAutoShift()
        case .delete, .globe:
            break
        }
    }

    /// Double space becomes ". " like the system keyboard.
    private func insertSpace() {
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        if before.hasSuffix(" "),
           let previous = before.dropLast().last,
           previous.isLetter || previous.isNumber {
            textDocumentProxy.deleteBackward()
            textDocumentProxy.insertText(". ")
        } else {
            textDocumentProxy.insertText(" ")
        }
        refreshAutoShift()
    }

    @objc private func deleteDown() {
        hideUndo()
        textDocumentProxy.deleteBackward()
        deleteTimer?.invalidate()
        deleteTimer = Timer.scheduledTimer(withTimeInterval: 0.45, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.startDeleteRepeat() }
        }
    }

    private func startDeleteRepeat() {
        deleteTimer = Timer.scheduledTimer(withTimeInterval: 0.08, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.textDocumentProxy.deleteBackward() }
        }
    }

    @objc private func deleteUp() {
        deleteTimer?.invalidate()
        deleteTimer = nil
        refreshAutoShift()
    }

    // MARK: Dictation

    private let impact = UIImpactFeedbackGenerator(style: .medium)
    private let notice = UINotificationFeedbackGenerator()
    private let selection = UISelectionFeedbackGenerator()

    func micTapped() {
        guard fullAccess() else {
            micBar.setStatus("Turn on Allow Full Access in Settings ▸ General ▸ Keyboard ▸ Keyboards ▸ WhimprFlow")
            notice.notificationOccurred(.warning)
            return
        }
        hideUndo()
        switch flowState {
        case .ready:
            impact.impactOccurred()
            // A new dictation replaces the one waiting (it's in History).
            pendingText = nil
            updateChip()
            send(BridgeMessage(.start, cleanupLevel: pillLevel))
            micBar.setStatus("Starting…")
        case .recording:
            impact.impactOccurred()
            send(BridgeMessage(.stop))
            flowState = .transcribing
            overlay.showWorking("Finishing…")
        case .connecting:
            micPending = true
        case .off:
            micPending = true
            flowState = .connecting
            bridge.connect(token: bridgeToken)
        case .transcribing:
            break
        }
        updateMicButton()
    }

    private func cancelDictation() {
        guard flowState == .recording else { return }
        send(BridgeMessage(.cancel))
        flowState = .ready
        overlay.hide()
        micBar.setStatus("Cancelled")
        updateMicButton()
    }

    private func cycleLevel() {
        pillLevel = pillLevel.next
        selection.selectionChanged()
    }

    func handleBridge(_ event: BridgeClient.Event) {
        switch event {
        case .ready(let defaultLevel):
            flowState = .ready
            if let defaultLevel { pillLevel = defaultLevel }
            if micPending {
                micPending = false
                impact.impactOccurred()
                send(BridgeMessage(.start, cleanupLevel: pillLevel))
                micBar.setStatus("Starting…")
            } else {
                micBar.setStatus("Flow is on")
            }
        case .noSession:
            let wantedMic = micPending
            micPending = false
            flowState = .off
            overlay.hide()
            if wantedMic {
                openAppForFlow()
            } else {
                micBar.setStatus("Tap the mic to start Flow")
            }
        case .recording:
            flowState = .recording
            micBar.setStatus("")
            overlay.showRecording()
        case .audioLevel(let value):
            if flowState == .recording { overlay.push(level: value) }
        case .partial(let text):
            if flowState == .recording { overlay.showHeard(text) }
        case .transcribing:
            flowState = .transcribing
            overlay.showWorking("Finishing…")
        case .cleaning:
            flowState = .transcribing
            overlay.showWorking("Polishing…")
        case .final(let text):
            flowState = .ready
            overlay.hide()
            if text.isEmpty {
                micBar.setStatus("Didn't catch that. Try again.")
                notice.notificationOccurred(.warning)
            } else {
                let inserted = Self.joining(text, after: textDocumentProxy.documentContextBeforeInput)
                textDocumentProxy.insertText(inserted)
                notice.notificationOccurred(.success)
                showUndo(for: inserted)
            }
        case .error(let message):
            flowState = .ready
            overlay.hide()
            micBar.setStatus(message)
            notice.notificationOccurred(.error)
        case .pending(let text):
            pendingText = text
            updateChip()
        }
        updateMicButton()
    }

    private func openAppForFlow() {
        if AppOpener.open(Bridge.flowURL(token: bridgeToken), from: self) {
            micBar.setStatus("Tap ◀ at the top left to come back")
        } else {
            micBar.setStatus("Open the WhimprFlow app to start Flow")
        }
    }

    private func updateMicButton() {
        let state: MicBar.MicState = switch flowState {
        case .off: .off
        case .connecting: micPending ? .busy : .off
        case .ready: .ready
        case .recording: .recording
        case .transcribing: .busy
        }
        micBar.setMicState(state)
    }

    // MARK: Undo

    private func showUndo(for inserted: String) {
        undo.recordInsert(inserted)
        micBar.setChip(.undo)
        undoTimer?.invalidate()
        undoTimer = Timer.scheduledTimer(withTimeInterval: UndoTracker.window, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.hideUndo() }
        }
    }

    private func hideUndo() {
        undoTimer?.invalidate()
        undoTimer = nil
        undo.clear()
        updateChip()
        if flowState == .ready { micBar.setStatus("Flow is on") }
    }

    /// Undo while it applies, otherwise a pending dictation to insert.
    private func updateChip() {
        if undoTimer != nil {
            micBar.setChip(.undo)
        } else if let pendingText {
            micBar.setChip(.insert(pendingText))
        } else {
            micBar.setChip(.none)
        }
    }

    private func insertPending() {
        guard let text = pendingText else { return }
        pendingText = nil
        let inserted = Self.joining(text, after: textDocumentProxy.documentContextBeforeInput)
        textDocumentProxy.insertText(inserted)
        send(BridgeMessage(.claim))
        notice.notificationOccurred(.success)
        showUndo(for: inserted)
    }

    private func undoDictation() {
        let proxy = textDocumentProxy
        let undone = undo.undo(contextBefore: proxy.documentContextBeforeInput) { proxy.deleteBackward() }
        hideUndo()
        micBar.setStatus(undone ? "Undone" : "Can't undo after editing")
    }

    /// Dictated text joined onto what's already typed: a space before it
    /// unless it starts a line, follows a space, or starts with punctuation.
    static func joining(_ text: String, after before: String?) -> String {
        guard let last = before?.last, !last.isWhitespace else { return text }
        if let first = text.first, ".,!?;:'".contains(first) { return text }
        return " " + text
    }

    /// The end of a long partial transcript, so the newest words stay visible.
    private static func tail(of text: String, length: Int = 42) -> String {
        text.count > length ? "…" + text.suffix(length) : text
    }

    // MARK: Shift

    private func setShifted(_ value: Bool) {
        guard shifted != value else { return }
        shifted = value
        updateKeyLabels()
    }

    /// Capitalise the first letter of the field and of each sentence.
    private func refreshAutoShift() {
        guard page == .letters, textDocumentProxy.autocapitalizationType != UITextAutocapitalizationType.none else {
            setShifted(false)
            return
        }
        let before = textDocumentProxy.documentContextBeforeInput ?? ""
        let sentenceStart = before.isEmpty
            || before.hasSuffix("\n")
            || [". ", "! ", "? "].contains { before.hasSuffix($0) }
        setShifted(sentenceStart)
    }
}
