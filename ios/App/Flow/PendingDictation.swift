import Foundation

/// A dictation that finished after its keyboard closed, waiting for the
/// next keyboard to offer it. Offered for 10 minutes; it's in History either way.
struct PendingDictation {
    static let lifetime: TimeInterval = 10 * 60

    private var stored: (text: String, at: Date)?

    mutating func store(_ text: String, at date: Date = Date()) {
        stored = (text, date)
    }

    /// The text, while it's still fresh.
    func text(now: Date = Date()) -> String? {
        guard let stored, now.timeIntervalSince(stored.at) <= Self.lifetime else { return nil }
        return stored.text
    }

    mutating func clear() {
        stored = nil
    }
}
