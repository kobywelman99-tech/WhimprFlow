import Foundation
import Observation
import os

private let log = Logger(subsystem: "com.whimpr.whimprflow", category: "history")

struct DictationRecord: Codable, Identifiable, Equatable {
    enum Source: String, Codable {
        case keyboard, app
    }

    var id = UUID()
    var date: Date
    /// What the recognizer heard.
    var raw: String
    /// What was inserted (the cleaned text, or `raw` when cleanup didn't run).
    var text: String
    var source: Source
    /// The cleanup level used; nil when cleanup was off.
    var level: DictationLevel?
    var cleaned: Bool
}

/// Every dictation, kept on the phone until the user deletes it: the record
/// of how Koby speaks that style learning grows from.
@MainActor
@Observable
final class DictationHistory {
    /// Newest first.
    private(set) var records: [DictationRecord] = []
    @ObservationIgnored private let fileURL: URL

    init(fileURL: URL = DictationHistory.defaultFileURL) {
        self.fileURL = fileURL
        load()
    }

    static var defaultFileURL: URL {
        URL.applicationSupportDirectory.appending(path: "history.json")
    }

    func add(_ record: DictationRecord) {
        guard !record.raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        records.insert(record, at: 0)
        save()
    }

    func delete(_ id: DictationRecord.ID) {
        records.removeAll { $0.id == id }
        save()
    }

    func clear() {
        records.removeAll()
        save()
    }

    private func load() {
        guard let data = try? Data(contentsOf: fileURL) else { return }
        do {
            records = try Self.decoder.decode([DictationRecord].self, from: data)
        } catch {
            // Keep the unreadable file rather than overwrite it on the next save.
            let aside = fileURL.deletingPathExtension()
                .appendingPathExtension("unreadable-\(Int(Date().timeIntervalSince1970)).json")
            try? FileManager.default.moveItem(at: fileURL, to: aside)
            log.error("History file unreadable, moved to \(aside.lastPathComponent, privacy: .public): \(error.localizedDescription, privacy: .public)")
        }
    }

    private func save() {
        do {
            try FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Self.encoder.encode(records).write(to: fileURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        } catch {
            log.error("Couldn't save history: \(error.localizedDescription, privacy: .public)")
        }
    }

    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted]
        return encoder
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
