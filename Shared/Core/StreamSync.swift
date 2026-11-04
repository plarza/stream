import AzaKit
import AzaUI
import Foundation
import StreamBindings

/// aza sync for Stream, and the Stream node at stream.aza.network.
///
/// Signed in with aza, Stream signs in to the node too (its certificate and a signed challenge, for a
/// bearer token), and moves aza sync items between the aza client and the core: `settings` (the
/// Spotify client ID, the Last.fm key and sign-in) and `songs/<sha256>`, one per music file on the
/// node. The core decides what each item means and what to write; this only carries them.
@MainActor
final class StreamSync {
    static let shared = StreamSync()
    static let appID = "stream.aza.network"
    static let node = URL(string: "https://\(appID)")!

    private var running = false
    private var again = false
    private var started = false
    /// The identity the core was connected to the node as, this launch.
    private var connected: String?
    private let keychain = Keychain(service: "network.aza.stream")

    private struct Session: Codable {
        var token: String
        var expires: Double
        var identity: String
    }

    /// Syncs now, then every five minutes and whenever the core has something to write.
    func start() {
        guard !started, !AppModel.isTesting else { return }
        started = true
        Task {
            while true {
                run()
                try? await Task.sleep(for: .seconds(300))
            }
        }
        watchAccount()
    }

    /// Signing in or out of aza syncs at once.
    private func watchAccount() {
        withObservationTracking { _ = Account.shared.client } onChange: {
            Task { @MainActor in
                self.run()
                self.watchAccount()
            }
        }
    }

    func run() {
        guard started else { return }
        if running {
            again = true
            return
        }
        running = true
        Task {
            repeat {
                again = false
                await once()
            } while again
            running = false
        }
    }

    private func once() async {
        let core = AppModel.shared.core
        guard let client = Account.shared.client, let identity = await client.identity() else {
            if connected != nil || keychain.get("node.session") != nil {
                keychain.set("node.session", nil)
                connected = nil
                core.nodeDisconnect()
            }
            return
        }
        do {
            try await connect(client, identity: identity)
            let items = try await client.readChanges()
            let incoming = items.map { StreamBindings.SyncItem(name: $0.name, version: $0.version, data: $0.data) }
            // Applying writes the library: keep it off the main thread.
            await Task.detached { core.syncApply(items: incoming) }.value
            for item in core.syncOutgoing() {
                do {
                    try await client.write(name: item.name, version: item.version, data: item.data)
                    core.syncWritten(item: item)
                } catch {
                    // Another device wrote first: read theirs, then try again.
                    print("sync: writing \(item.name): \(error)")
                    again = true
                    break
                }
            }
        } catch {
            print("sync: \(error)")
        }
    }

    /// Signs in to the node once per 90 days, and again whenever it stops taking the token.
    private func connect(_ client: AzaClient, identity: String) async throws {
        let core = AppModel.shared.core
        var session = keychain.get("node.session").flatMap { try? JSONDecoder().decode(Session.self, from: Data($0.utf8)) }
        let refused = connected != nil && !core.nodeStatus().connected
        if session?.identity != identity || (session?.expires ?? 0) < Date.now.timeIntervalSince1970 + 86400 || refused {
            session = try await signIn(client)
            keychain.set("node.session", String(decoding: try JSONEncoder().encode(session), as: UTF8.self))
            connected = nil
        }
        guard let session, connected != identity else { return }
        core.nodeConnect(url: Self.node.absoluteString, token: session.token, identity: identity)
        connected = identity
    }

    private func signIn(_ client: AzaClient) async throws -> Session {
        struct Challenge: Decodable { var challenge: String }
        let (data, _) = try await URLSession.shared.data(from: Self.node.appending(path: "v1/challenge"))
        let challenge = try JSONDecoder().decode(Challenge.self, from: data).challenge
        guard let bytes = Data(base64Encoded: challenge) else { throw URLError(.cannotParseResponse) }
        let proof = try await client.signIn(challenge: bytes)
        var request = URLRequest(url: Self.node.appending(path: "v1/session"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode([
            "certificate": proof.certificate.base64EncodedString(),
            "proof": proof.proof.base64EncodedString(),
            "challenge": challenge,
        ])
        let (body, response) = try await URLSession.shared.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else {
            struct Failure: Decodable { var error: String }
            throw NodeError(message: (try? JSONDecoder().decode(Failure.self, from: body).error) ?? "The Stream node didn't answer")
        }
        return try JSONDecoder().decode(Session.self, from: body)
    }

    struct NodeError: LocalizedError {
        var message: String
        var errorDescription: String? { message }
    }
}
