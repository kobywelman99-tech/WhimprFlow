import Foundation

/// Turns pasted or imported writing (emails, notes, documents) into style
/// samples: only the user's own words, one sample per piece.
enum StyleImport {
    /// Separates pieces in an import file, e.g. one email per block.
    static let separator = "-----"

    /// Splits `text` into samples: at separator lines, or at blank lines when
    /// there are none. Each piece is cleaned; pieces under 3 words are dropped.
    static func samples(from text: String) -> [String] {
        let normalized = text.replacingOccurrences(of: "\r\n", with: "\n")
        let pieces: [String]
        if normalized.components(separatedBy: "\n").contains(where: { $0.trimmingCharacters(in: .whitespaces) == separator }) {
            pieces = normalized.components(separatedBy: "\n")
                .split { $0.trimmingCharacters(in: .whitespaces) == separator }
                .map { $0.joined(separator: "\n") }
        } else {
            pieces = normalized.components(separatedBy: "\n\n")
        }
        return pieces.map(ownWords).filter { $0.split(whereSeparator: \.isWhitespace).count >= 3 }
    }

    /// Keeps what the author wrote in one email or note: drops quoted
    /// replies, forwarded text, signatures, a greeting line and sign-off
    /// lines, so "Best regards" never becomes a "go-to phrase".
    static func ownWords(_ piece: String) -> String {
        var lines: [String] = []
        for line in piece.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            // Everything after these belongs to someone else or is boilerplate.
            if trimmed == "--" || trimmed == "-- " || isReplyHeader(trimmed) || isForwardHeader(trimmed) { break }
            if trimmed.hasPrefix(">") { continue }
            lines.append(trimmed)
        }
        while let first = lines.first, first.isEmpty || isGreeting(first) { lines.removeFirst() }
        while let last = lines.last, last.isEmpty || isSignOff(last) || isShortNameLine(last, afterSignOff: lines) {
            lines.removeLast()
        }
        let joined = lines.filter { !$0.isEmpty }.joined(separator: " ")
        return withoutPhoneNumbers(withoutInlineSignature(joined))
    }

    /// A signature that shares a line with the text ("…Many thanks, Jane Doe
    /// Phone Number: …", common in email previews) is cut from where it starts.
    static func withoutInlineSignature(_ text: String) -> String {
        let patterns = [
            #"\s(many thanks|thanks|thank you|best regards|kind regards|regards|best|cheers|sincerely)[,!.]?\s+[A-Z][\w()'.-]*(\s+[A-Z(][\w()'.-]*){0,3}\s+(phone|mobile|cell|tel|linkedin|www\.|https?://).*$"#,
            #"\s(phone( number)?|mobile|cell|tel|linkedin)\s*:.*$"#,
        ]
        var result = text
        for pattern in patterns {
            if let range = result.range(of: pattern, options: [.regularExpression, .caseInsensitive]) {
                result.removeSubrange(range)
            }
        }
        return result.trimmingCharacters(in: .whitespaces)
    }

    /// Phone numbers say nothing about style and shouldn't be stored.
    static func withoutPhoneNumbers(_ text: String) -> String {
        text.replacingOccurrences(of: #"\+?\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}"#, with: "", options: .regularExpression)
            .replacingOccurrences(of: #"\s{2,}"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
    }

    private static func isReplyHeader(_ line: String) -> Bool {
        let lower = line.lowercased()
        return (lower.hasPrefix("on ") && lower.hasSuffix("wrote:"))
            || lower.hasPrefix("from: ")
            || lower.hasPrefix("-----original message")
    }

    private static func isForwardHeader(_ line: String) -> Bool {
        line.lowercased().contains("forwarded message")
    }

    private static let greetings = ["hi", "hey", "hello", "dear", "good morning", "good afternoon", "good evening", "hi all", "hey all", "hi team"]

    /// "Hi Sam," / "Hello," / "Dear team," as a line of its own.
    private static func isGreeting(_ line: String) -> Bool {
        let lower = line.lowercased()
        let words = lower.split(whereSeparator: \.isWhitespace)
        guard words.count <= 4 else { return false }
        return greetings.contains { lower == $0 || lower.hasPrefix($0 + " ") || lower.hasPrefix($0 + ",") || lower.hasPrefix($0 + "!") }
    }

    private static let signOffs = [
        "best", "best regards", "kind regards", "regards", "warm regards", "thanks", "thank you", "thanks!",
        "thanks again", "many thanks", "cheers", "sincerely", "all the best", "talk soon", "take care",
    ]

    private static func isSignOff(_ line: String) -> Bool {
        let lower = line.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: ",.! "))
        return signOffs.contains(lower) || lower.hasPrefix("sent from my ") || lower.hasPrefix("get outlook for")
    }

    /// A name under a sign-off ("Best,\nKoby") is part of the sign-off.
    private static func isShortNameLine(_ line: String, afterSignOff lines: [String]) -> Bool {
        guard lines.count >= 2, isSignOff(lines[lines.count - 2]) else { return false }
        return line.split(whereSeparator: \.isWhitespace).count <= 3
    }

    /// Learns from every `.txt` in `folder` (Documents/Import, where writing
    /// copied onto the phone over the cable lands), then deletes them: only
    /// the counts are kept. Returns how many samples were learned.
    @MainActor
    static func learnFromImportFolder(_ folder: URL = importFolder, into style: StyleModel) throws -> Int {
        let files = (try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? []
        var learned = 0
        for file in files where file.pathExtension.lowercased() == "txt" {
            let text = try String(contentsOf: file, encoding: .utf8)
            learned += try style.learn(from: samples(from: text))
            try FileManager.default.removeItem(at: file)
        }
        return learned
    }

    static var importFolder: URL {
        URL.documentsDirectory.appending(path: "Import")
    }
}
