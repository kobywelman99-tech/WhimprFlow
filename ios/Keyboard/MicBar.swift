import UIKit

/// The bar above the keys: style pill on the left, status (or Undo) in the
/// middle, mic pill on the right. Fixed height so the keys never jump.
final class MicBar: UIView {
    enum MicState: Equatable {
        /// No Flow session yet; tapping opens the app to start one.
        case off
        case ready
        case recording
        /// Waiting for the text.
        case busy
    }

    static let height: CGFloat = 52

    var onMic: () -> Void = {}
    var onLevel: () -> Void = {}
    var onUndo: () -> Void = {}

    var level: DictationLevel = .light {
        didSet { updateLevel() }
    }
    private(set) var micState: MicState = .off

    let levelPill = UIButton(type: .system)
    let statusLabel = UILabel()
    let undoButton = UIButton(type: .system)
    let micButton = UIButton(type: .system)

    override init(frame: CGRect) {
        super.init(frame: frame)
        build()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    func setMicState(_ state: MicState) {
        micState = state
        var config = UIButton.Configuration.filled()
        config.cornerStyle = .capsule
        config.contentInsets = .zero
        switch state {
        case .off:
            config = .bordered()
            config.cornerStyle = .capsule
            config.image = UIImage(systemName: "mic")
            config.baseForegroundColor = .tintColor
            micButton.accessibilityLabel = "Start Flow"
            micButton.accessibilityHint = "Opens WhimprFlow once so the keyboard can dictate"
        case .ready:
            config.image = UIImage(systemName: "mic.fill")
            config.baseBackgroundColor = .tintColor
            config.baseForegroundColor = .white
            micButton.accessibilityLabel = "Dictate"
            micButton.accessibilityHint = nil
        case .recording:
            config.image = UIImage(systemName: "stop.fill")
            config.baseBackgroundColor = .systemRed
            config.baseForegroundColor = .white
            micButton.accessibilityLabel = "Stop dictating"
            micButton.accessibilityHint = nil
        case .busy:
            config.showsActivityIndicator = true
            config.baseBackgroundColor = .systemGray4
            config.baseForegroundColor = .label
            micButton.accessibilityLabel = "Working"
            micButton.accessibilityHint = nil
        }
        config.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 17, weight: .semibold)
        micButton.configuration = config
        micButton.isEnabled = state != .busy
    }

    func setStatus(_ text: String) {
        statusLabel.text = text
    }

    /// Shows the Undo chip in place of the status.
    func setUndoVisible(_ visible: Bool) {
        undoButton.isHidden = !visible
        statusLabel.isHidden = visible
    }

    private func build() {
        var pill = UIButton.Configuration.gray()
        pill.cornerStyle = .capsule
        pill.imagePlacement = .trailing
        pill.imagePadding = 4
        pill.image = UIImage(systemName: "chevron.up.chevron.down")
        pill.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 10, weight: .semibold)
        pill.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 12, bottom: 6, trailing: 10)
        levelPill.configuration = pill
        levelPill.accessibilityLabel = "Cleanup level"
        levelPill.accessibilityHint = "Double-tap to change for this dictation"
        levelPill.addAction(UIAction { [weak self] _ in self?.onLevel() }, for: .touchUpInside)

        statusLabel.font = .preferredFont(forTextStyle: .footnote)
        statusLabel.adjustsFontForContentSizeCategory = true
        statusLabel.textColor = .secondaryLabel
        statusLabel.textAlignment = .center
        statusLabel.lineBreakMode = .byTruncatingHead
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        var undo = UIButton.Configuration.gray()
        undo.cornerStyle = .capsule
        undo.title = "Undo"
        undo.image = UIImage(systemName: "arrow.uturn.backward")
        undo.imagePadding = 4
        undo.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold)
        undoButton.configuration = undo
        undoButton.accessibilityLabel = "Undo dictation"
        undoButton.isHidden = true
        undoButton.addAction(UIAction { [weak self] _ in self?.onUndo() }, for: .touchUpInside)

        micButton.addAction(UIAction { [weak self] _ in self?.onMic() }, for: .touchUpInside)

        let middle = UIView()
        for view in [statusLabel, undoButton] {
            view.translatesAutoresizingMaskIntoConstraints = false
            middle.addSubview(view)
        }

        let stack = UIStackView(arrangedSubviews: [levelPill, middle, micButton])
        stack.axis = .horizontal
        stack.alignment = .center
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        levelPill.setContentHuggingPriority(.required, for: .horizontal)
        levelPill.setContentCompressionResistancePriority(.required, for: .horizontal)

        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: Self.height),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            micButton.widthAnchor.constraint(equalToConstant: 56),
            micButton.heightAnchor.constraint(equalToConstant: 36),
            levelPill.heightAnchor.constraint(equalToConstant: 32),
            middle.heightAnchor.constraint(equalToConstant: 36),
            statusLabel.leadingAnchor.constraint(equalTo: middle.leadingAnchor),
            statusLabel.trailingAnchor.constraint(equalTo: middle.trailingAnchor),
            statusLabel.centerYAnchor.constraint(equalTo: middle.centerYAnchor),
            undoButton.centerXAnchor.constraint(equalTo: middle.centerXAnchor),
            undoButton.centerYAnchor.constraint(equalTo: middle.centerYAnchor),
            undoButton.heightAnchor.constraint(equalToConstant: 32),
        ])

        updateLevel()
        setMicState(.off)
    }

    private func updateLevel() {
        levelPill.configuration?.title = level.title
        levelPill.accessibilityValue = level.title
    }
}
