import Foundation
import Observation

/// The user's dictionary (Rust `UserDictionary`) for SwiftUI. Words reach
/// dictation twice: as vocabulary hints to the speech recogniser, and as the
/// relevant entries in the cleanup prompt.
@MainActor
@Observable
final class DictionaryModel {
    private(set) var entries: [DictEntry]
    @ObservationIgnored private let store: UserDictionary

    init(fileURL: URL = DictionaryModel.defaultFileURL) {
        store = UserDictionary.open(path: fileURL.path)
        entries = store.entries()
    }

    /// Application Support/dictionary.json, the desktop's file name and format.
    static var defaultFileURL: URL {
        URL.applicationSupportDirectory.appending(path: "dictionary.json")
    }

    /// Readable versions of the Rust errors. The generated `DictionaryError`
    /// describes itself as its Swift case name ("DictionaryError.EmptyWord").
    struct EditError: LocalizedError, Equatable {
        let errorDescription: String?
    }

    /// Adds `word`. `heardAs` is optional, comma-separated, as on desktop.
    func add(word: String, heardAs: String = "") throws {
        let mishears = heardAs.split(separator: ",").map { String($0) }
        try readable { try store.add(correct: word, mishears: mishears) }
        entries = store.entries()
    }

    func remove(_ word: String) throws {
        try readable { _ = try store.remove(correct: word) }
        entries = store.entries()
    }

    private func readable(_ body: () throws -> Void) throws {
        do {
            try body()
        } catch let error as DictionaryError {
            switch error {
            case .EmptyWord: throw EditError(errorDescription: "Enter a word to add.")
            case .Io(let message): throw EditError(errorDescription: "Couldn't save the dictionary: \(message)")
            }
        }
    }

    /// The entries that sound close to something in `utterance`.
    func relevant(to utterance: String) -> [Vocab] {
        store.relevant(utterance: utterance)
    }

    var words: [String] { store.words() }

    /// Known mishears rewritten to their right spelling (whole words only).
    func applyReplacements(to text: String) -> String {
        store.applyReplacements(text: text)
    }

}
