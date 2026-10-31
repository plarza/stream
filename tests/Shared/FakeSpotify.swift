import Foundation
import Network

/// A stand-in for Spotify's accounts service and Web API, served by the test runner on
/// a local port, so a sign-in in the Simulator runs all the way through: the app opens
/// /authorize in its sign-in sheet, this hands a code straight back to the app's
/// loopback listener, the app trades it at /api/token and asks /v1/me who signed in.
/// (Spotify's streaming servers are the real ones; they turn this stand-in's tokens away.)
final class FakeSpotify: @unchecked Sendable {
    static let shared = FakeSpotify()
    static let listener = "Test Listener"

    private let server: NWListener
    private let queue = DispatchQueue(label: "fake-spotify")
    private let lock = NSLock()
    private var approved = 0
    let port: UInt16

    var accountsURL: String { "http://127.0.0.1:\(port)" }
    var apiURL: String { "http://127.0.0.1:\(port)/v1" }
    /// How many approvals the sign-in sheet has handed back.
    var approvals: Int { lock.withLock { approved } }

    private init() {
        // The listener needs its connection handler before it starts; it hands off to
        // `accept` once this object exists.
        let server = try! NWListener(using: .tcp, on: .any)
        let handoff = Handoff()
        server.newConnectionHandler = { c in handoff.accept?(c) }
        let ready = DispatchSemaphore(value: 0)
        server.stateUpdateHandler = { state in
            if case .ready = state { ready.signal() }
            if case .failed = state { ready.signal() }
        }
        server.start(queue: queue)
        _ = ready.wait(timeout: .now() + 5)
        self.server = server
        port = server.port?.rawValue ?? 0
        handoff.accept = { [weak self] c in self?.accept(c) }
    }

    private final class Handoff: @unchecked Sendable {
        var accept: ((NWConnection) -> Void)?
    }

    private func accept(_ c: NWConnection) {
        c.start(queue: queue)
        read(c, Data())
    }

    /// Reads one request (head, then as much body as Content-Length says) and answers it.
    private func read(_ c: NWConnection, _ buffer: Data) {
        c.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { [weak self] data, _, done, error in
            guard let self else { return }
            var buffer = buffer
            if let data { buffer.append(data) }
            if let end = buffer.range(of: Data("\r\n\r\n".utf8)) {
                let head = String(decoding: buffer[..<end.lowerBound], as: UTF8.self)
                let length = head.split(separator: "\r\n")
                    .first { $0.lowercased().hasPrefix("content-length:") }
                    .flatMap { Int($0.split(separator: ":")[1].trimmingCharacters(in: .whitespaces)) } ?? 0
                if buffer.count - end.upperBound >= length {
                    self.answer(c, requestLine: head.components(separatedBy: "\r\n").first ?? "")
                    return
                }
            }
            if done || error != nil { c.cancel(); return }
            self.read(c, buffer)
        }
    }

    private func answer(_ c: NWConnection, requestLine: String) {
        let target = requestLine.split(separator: " ").dropFirst().first.map(String.init) ?? "/"
        let url = URLComponents(string: "http://x" + target)
        let query = { (name: String) in url?.queryItems?.first { $0.name == name }?.value ?? "" }
        switch url?.path ?? "/" {
        case "/authorize":
            // Approve at once and hand the code back where the app asked.
            lock.withLock { approved += 1 }
            var back = URLComponents(string: query("redirect_uri"))!
            back.queryItems = [URLQueryItem(name: "code", value: "test-code"), URLQueryItem(name: "state", value: query("state"))]
            send(c, status: "302 Found", headers: ["Location": back.url!.absoluteString], body: "")
        case "/api/token":
            send(c, json: #"{"access_token":"test-token","token_type":"Bearer","expires_in":3600,"refresh_token":"test-refresh"}"#)
        case "/v1/me":
            send(c, json: #"{"id":"tester","display_name":"\#(Self.listener)","product":"premium"}"#)
        default:
            // The library is empty: every paged list has no items.
            send(c, json: #"{"items":[],"next":null,"artists":{"items":[],"next":null}}"#)
        }
    }

    private func send(_ c: NWConnection, json: String) {
        send(c, status: "200 OK", headers: ["Content-Type": "application/json"], body: json)
    }

    private func send(_ c: NWConnection, status: String, headers: [String: String], body: String) {
        var head = "HTTP/1.1 \(status)\r\nContent-Length: \(body.utf8.count)\r\nConnection: close\r\n"
        for (k, v) in headers { head += "\(k): \(v)\r\n" }
        c.send(content: Data((head + "\r\n" + body).utf8), completion: .contentProcessed { _ in c.cancel() })
    }
}
