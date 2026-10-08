import Foundation

/// The keyboard can't use the microphone, and a free Apple account can't give
/// the keyboard and app a shared container. So while a Flow session is on, the
/// app listens on a loopback-only socket and the keyboard (which needs Full
/// Access for networking) connects to it. Messages are one JSON object per
/// line.
///
/// Security: the socket only binds 127.0.0.1, and the app answers only a
/// client that presents the token the keyboard passed when it opened the app
/// with `whimprflow://flow?token=…`.
enum Bridge {
    static let host = "127.0.0.1"
    static let port: UInt16 = 47_821

    static func flowURL(token: String) -> URL {
        var components = URLComponents()
        components.scheme = "whimprflow"
        components.host = "flow"
        components.queryItems = [URLQueryItem(name: "token", value: token)]
        return components.url!
    }

    /// The token from a `whimprflow://flow?token=…` URL, or nil for any other URL.
    static func token(fromFlowURL url: URL) -> String? {
        guard url.scheme == "whimprflow", url.host == "flow",
              let token = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                .queryItems?.first(where: { $0.name == "token" })?.value,
              !token.isEmpty else { return nil }
        return token
    }
}

/// Cleanup levels by name. The keyboard doesn't link the Rust core, so this is
/// how it names them; the app maps them to the Rust `Level`.
enum DictationLevel: String, Codable, CaseIterable, Sendable {
    case off, light, medium, high

    var title: String {
        switch self {
        case .off: "Raw"
        case .light: "Light"
        case .medium: "Medium"
        case .high: "High"
        }
    }

    /// The next level for the keyboard's style pill, wrapping around.
    var next: DictationLevel {
        let all = Self.allCases
        return all[(all.firstIndex(of: self)! + 1) % all.count]
    }
}

struct BridgeMessage: Codable, Equatable, Sendable {
    enum Kind: String, Codable, Sendable {
        // keyboard → app
        case hello, start, stop, cancel
        /// The keyboard inserted the pending dictation.
        case claim
        // app → keyboard
        case ready, denied, recording, audioLevel, partial, transcribing, cleaning, final, error
        /// A dictation finished after its keyboard closed (e.g. scrolling a
        /// chat hides the keyboard). Offered to insert, never inserted blindly:
        /// the user may be in a different text box by now.
        case pending
    }

    var kind: Kind
    /// The token for `hello`, the text for `partial`/`final`/`pending`, the message for `error`.
    var text: String?
    /// `ready`: the app's default level. `start`: the level for this dictation.
    var cleanupLevel: DictationLevel?
    /// `audioLevel`: how loud the mic is, 0…1.
    var value: Double?
    /// `start`: the text already in the box before the cursor, so the app can check
    /// the dictation makes sense in its conversation. Stays on this phone's loopback.
    var context: String?

    init(_ kind: Kind, text: String? = nil, cleanupLevel: DictationLevel? = nil, value: Double? = nil, context: String? = nil) {
        self.kind = kind
        self.text = text
        self.cleanupLevel = cleanupLevel
        self.value = value
        self.context = context
    }
}

/// Turns a TCP byte stream into messages and back.
struct LineFramer {
    private var pending = Data()

    /// Feeds received bytes; returns every complete message so far. Lines that
    /// aren't valid messages are skipped.
    mutating func append(_ data: Data) -> [BridgeMessage] {
        pending.append(data)
        var messages: [BridgeMessage] = []
        while let newline = pending.firstIndex(of: 0x0A) {
            let line = pending[pending.startIndex..<newline]
            pending = Data(pending[pending.index(after: newline)...])
            if let message = try? JSONDecoder().decode(BridgeMessage.self, from: line) {
                messages.append(message)
            }
        }
        return messages
    }

    static func encode(_ message: BridgeMessage) -> Data {
        // JSONEncoder escapes newlines inside strings, so the framing is safe.
        var data = (try? JSONEncoder().encode(message)) ?? Data()
        data.append(0x0A)
        return data
    }
}
