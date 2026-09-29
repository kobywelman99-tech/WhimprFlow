import Foundation

/// Remembers the last dictation the keyboard inserted, so the Undo chip can
/// take it back. Undo only works while that text is still right before the
/// cursor and for a few seconds; after that it could delete the wrong thing.
struct UndoTracker {
    static let window: TimeInterval = 6

    private var inserted: String?
    private var insertedAt: Date?

    mutating func recordInsert(_ text: String, at date: Date = Date()) {
        inserted = text.isEmpty ? nil : text
        insertedAt = date
    }

    mutating func clear() {
        inserted = nil
        insertedAt = nil
    }

    func canUndo(contextBefore: String?, now: Date = Date()) -> Bool {
        guard let inserted, let insertedAt, now.timeIntervalSince(insertedAt) <= Self.window else { return false }
        return Self.endsWith(contextBefore ?? "", inserted)
    }

    /// Deletes the inserted text one character at a time. Returns false (and
    /// deletes nothing) when undo no longer applies.
    mutating func undo(contextBefore: String?, now: Date = Date(), deleteBackward: () -> Void) -> Bool {
        guard canUndo(contextBefore: contextBefore, now: now), let inserted else { return false }
        for _ in 0..<inserted.count { deleteBackward() }
        clear()
        return true
    }

    /// iOS only shares a limited stretch of text before the cursor, so a long
    /// insert may be longer than the context: then the context must be its end.
    private static func endsWith(_ context: String, _ inserted: String) -> Bool {
        if context.count >= inserted.count { return context.hasSuffix(inserted) }
        return !context.isEmpty && inserted.hasSuffix(context)
    }
}
