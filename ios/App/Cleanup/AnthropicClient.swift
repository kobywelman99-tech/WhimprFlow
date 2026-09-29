import Foundation

/// Sends a prepared cleanup (from the Rust `prepareCleanup`) to the Anthropic
/// Messages API and returns the model's text. Same request as the desktop's
/// `AnthropicProvider` (crates/whimpr-cleanup). There's no official Swift SDK,
/// so this is plain HTTPS.
struct AnthropicClient: Sendable {
    enum ClientError: LocalizedError, Equatable {
        case unauthorized
        case http(status: Int, message: String)
        /// Hit max_tokens: the answer is cut off, so don't use it.
        case truncated
        case refused
        case emptyResponse
        case network(String)

        var errorDescription: String? {
            switch self {
            case .unauthorized: "Anthropic rejected the API key. Check it in Settings."
            case .http(let status, let message): "Anthropic error \(status): \(message)"
            case .truncated: "The cleaned text was cut off."
            case .refused: "Anthropic declined to clean up this text."
            case .emptyResponse: "Anthropic returned no text."
            case .network(let message): "Couldn't reach Anthropic: \(message)"
            }
        }

        /// Worth one quick retry: busy servers and dropped connections.
        var isRetryable: Bool {
            switch self {
            case .http(let status, _): status == 429 || status >= 500
            case .network: true
            default: false
            }
        }
    }

    static let endpoint = URL(string: "https://api.anthropic.com/v1/messages")!
    static let model = "claude-haiku-4-5"

    let apiKey: String
    var session: URLSession = .shared
    /// The keyboard is waiting on this, so give up well before the desktop's
    /// 15 s; the raw text is inserted instead.
    var timeout: TimeInterval = 8

    func clean(_ request: CleanupRequest) async throws -> String {
        let started = Date()
        do {
            return try await send(request)
        } catch let error as ClientError where error.isRetryable && Date().timeIntervalSince(started) < 3 {
            try await Task.sleep(for: .milliseconds(400))
            return try await send(request)
        }
    }

    private func send(_ request: CleanupRequest) async throws -> String {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: makeURLRequest(request))
        } catch {
            throw ClientError.network(error.localizedDescription)
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        return try Self.parse(data: data, status: status)
    }

    func makeURLRequest(_ request: CleanupRequest) throws -> URLRequest {
        var urlRequest = URLRequest(url: Self.endpoint, timeoutInterval: timeout)
        urlRequest.httpMethod = "POST"
        urlRequest.setValue("application/json", forHTTPHeaderField: "content-type")
        urlRequest.setValue(apiKey, forHTTPHeaderField: "x-api-key")
        urlRequest.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
        urlRequest.httpBody = try JSONEncoder().encode(Body(
            model: Self.model,
            // The desktop uses 512; a long phone dictation can exceed that, and
            // a cut-off answer is thrown away, so leave room.
            max_tokens: 1024,
            temperature: 0.2,
            system: request.system,
            messages: request.messages.map { Body.Message(role: $0.role, content: $0.content) }
        ))
        return urlRequest
    }

    static func parse(data: Data, status: Int) throws -> String {
        guard (200..<300).contains(status) else {
            if status == 401 { throw ClientError.unauthorized }
            let message = (try? JSONDecoder().decode(ErrorBody.self, from: data))?.error.message
                ?? HTTPURLResponse.localizedString(forStatusCode: status)
            throw ClientError.http(status: status, message: message)
        }
        let body: ResponseBody
        do {
            body = try JSONDecoder().decode(ResponseBody.self, from: data)
        } catch {
            throw ClientError.emptyResponse
        }
        switch body.stop_reason {
        case "max_tokens": throw ClientError.truncated
        case "refusal": throw ClientError.refused
        default: break
        }
        let text = body.content.filter { $0.type == "text" }.compactMap(\.text).joined()
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { throw ClientError.emptyResponse }
        return text
    }

    // MARK: Wire format

    struct Body: Encodable {
        struct Message: Encodable {
            let role: String
            let content: String
        }

        let model: String
        let max_tokens: Int
        let temperature: Double
        let system: String
        let messages: [Message]
    }

    private struct ResponseBody: Decodable {
        struct Block: Decodable {
            let type: String
            let text: String?
        }

        let content: [Block]
        let stop_reason: String?
    }

    private struct ErrorBody: Decodable {
        struct Detail: Decodable {
            let message: String
        }

        let error: Detail
    }
}
