import UIKit

/// Covers the keys while dictating: ✕ to cancel, a timer, a live waveform and
/// what's been heard so far. Finishing is the bar's mic button (now a stop
/// button), which stays under the thumb that started the dictation.
final class RecordingOverlay: UIView {
    var onCancel: () -> Void = {}

    let cancelButton = UIButton(type: .system)
    let waveform = WaveformView()
    let timerLabel = UILabel()
    let captionLabel = UILabel()

    private var startedAt: Date?
    private var clock: Timer?

    override init(frame: CGRect) {
        super.init(frame: frame)
        build()
        isHidden = true
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// Recording has started.
    func showRecording(now: Date = Date()) {
        waveform.reset()
        waveform.barColor = .tintColor
        startedAt = now
        updateTimer(now: now)
        captionLabel.text = "Listening…"
        setButtonsEnabled(true)
        clock?.invalidate()
        clock = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.updateTimer(now: Date()) }
        }
        reveal()
    }

    /// The words so far, newest visible.
    func showHeard(_ text: String) {
        captionLabel.text = text.isEmpty ? "Listening…" : text
    }

    func push(level: Double) {
        waveform.push(level)
    }

    /// Recording is over; waiting for the text ("Finishing…", "Polishing…").
    func showWorking(_ caption: String) {
        clock?.invalidate()
        clock = nil
        captionLabel.text = caption
        setButtonsEnabled(false)
        waveform.barColor = .tertiaryLabel
        waveform.startSweep()
        reveal()
    }

    func hide() {
        clock?.invalidate()
        clock = nil
        startedAt = nil
        waveform.stopSweep()
        guard !isHidden else { return }
        UIView.animate(withDuration: 0.15, animations: { self.alpha = 0 }, completion: { _ in
            if self.alpha == 0 { self.isHidden = true }
        })
    }

    /// "0:07" since recording started.
    static func elapsed(from start: Date, to now: Date) -> String {
        let seconds = max(0, Int(now.timeIntervalSince(start)))
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }

    private func updateTimer(now: Date) {
        guard let startedAt else { return }
        let text = Self.elapsed(from: startedAt, to: now)
        timerLabel.text = text
        timerLabel.accessibilityLabel = "Recording, \(text)"
    }

    private func reveal() {
        guard isHidden || alpha < 1 else { return }
        isHidden = false
        alpha = 0
        UIView.animate(withDuration: 0.15) { self.alpha = 1 }
    }

    private func setButtonsEnabled(_ enabled: Bool) {
        cancelButton.isEnabled = enabled
        cancelButton.alpha = enabled ? 1 : 0.35
    }

    private func build() {
        backgroundColor = UIColor { $0.userInterfaceStyle == .dark ? UIColor(white: 0.17, alpha: 1) : UIColor(red: 0.82, green: 0.83, blue: 0.85, alpha: 1) }

        var cancel = UIButton.Configuration.gray()
        cancel.cornerStyle = .capsule
        cancel.image = UIImage(systemName: "xmark")
        cancel.baseForegroundColor = .label
        cancelButton.configuration = cancel
        cancelButton.accessibilityLabel = "Cancel dictation"
        cancelButton.addAction(UIAction { [weak self] _ in self?.onCancel() }, for: .touchUpInside)

        cancelButton.configuration?.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 15, weight: .semibold)

        timerLabel.font = .monospacedDigitSystemFont(ofSize: 15, weight: .semibold)
        timerLabel.textAlignment = .center
        captionLabel.font = .preferredFont(forTextStyle: .footnote)
        captionLabel.textColor = .secondaryLabel
        captionLabel.textAlignment = .center
        captionLabel.lineBreakMode = .byTruncatingHead
        captionLabel.accessibilityTraits = .updatesFrequently

        // A spacer the width of ✕ keeps the timer centred.
        let balance = UIView()
        let top = UIStackView(arrangedSubviews: [cancelButton, timerLabel, balance])
        top.axis = .horizontal
        top.alignment = .center
        top.distribution = .equalCentering

        let stack = UIStackView(arrangedSubviews: [top, waveform, captionLabel])
        stack.axis = .vertical
        // Spare height goes into equal gaps rather than stretching one row.
        stack.distribution = .equalSpacing
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)

        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 6),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
            cancelButton.widthAnchor.constraint(equalToConstant: 56),
            cancelButton.heightAnchor.constraint(equalToConstant: 36),
            balance.widthAnchor.constraint(equalToConstant: 56),
            top.heightAnchor.constraint(equalToConstant: 36),
            waveform.heightAnchor.constraint(equalToConstant: 72),
        ])
        waveform.setContentHuggingPriority(.defaultLow, for: .vertical)
    }
}
