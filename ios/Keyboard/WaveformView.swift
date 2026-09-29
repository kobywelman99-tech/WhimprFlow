import UIKit

/// Rounded bars of recent mic loudness, newest on the right. While the app is
/// working it shows a gentle travelling hump instead, so the keyboard never
/// looks frozen.
final class WaveformView: UIView {
    static let barCount = 32

    /// Loudness 0…1, oldest first; always `barCount` long.
    private(set) var levels = [Double](repeating: 0, count: WaveformView.barCount)
    private var sweepTimer: Timer?
    private var sweepPhase = 0.0

    var barColor: UIColor = .label {
        didSet { setNeedsDisplay() }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        contentMode = .redraw
        isAccessibilityElement = false
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// Adds the newest loudness reading.
    func push(_ level: Double) {
        stopSweep()
        levels.removeFirst()
        levels.append(min(1, max(0, level)))
        setNeedsDisplay()
    }

    func reset() {
        stopSweep()
        levels = [Double](repeating: 0, count: Self.barCount)
        setNeedsDisplay()
    }

    func startSweep() {
        guard sweepTimer == nil else { return }
        sweepTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 20, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.stepSweep() }
        }
    }

    func stopSweep() {
        sweepTimer?.invalidate()
        sweepTimer = nil
    }

    private func stepSweep() {
        sweepPhase += 0.12
        let center = (sin(sweepPhase) + 1) / 2 * Double(Self.barCount - 1)
        levels = (0..<Self.barCount).map { index in
            let distance = abs(Double(index) - center)
            return 0.15 + 0.35 * exp(-distance * distance / 18)
        }
        setNeedsDisplay()
    }

    override func draw(_ rect: CGRect) {
        let slot = bounds.width / CGFloat(Self.barCount)
        let barWidth = max(2, slot * 0.4)
        let minHeight: CGFloat = 3
        barColor.setFill()
        for (index, level) in levels.enumerated() {
            let height = max(minHeight, CGFloat(level) * bounds.height)
            let x = CGFloat(index) * slot + (slot - barWidth) / 2
            let bar = CGRect(x: x, y: (bounds.height - height) / 2, width: barWidth, height: height)
            UIBezierPath(roundedRect: bar, cornerRadius: barWidth / 2).fill()
        }
    }

    override func willMove(toWindow newWindow: UIWindow?) {
        super.willMove(toWindow: newWindow)
        if newWindow == nil { stopSweep() }
    }
}
