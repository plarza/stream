import StreamBindings
import Foundation
import Network

/// Finds the Mac over Bonjour and mirrors its library onto this iPhone.
@MainActor
final class SyncClient {
    static let shared = SyncClient()
    private var browser: NWBrowser?
    private var probing: NWConnection?
    private var busy = false

    func syncNow() {
        // Under UI tests the music folder is the repo's fixture library; mirroring
        // a Mac into it would replace the fixtures with someone's real library.
        guard !busy, !AppModel.isTesting else { return }
        busy = true
        let browser = NWBrowser(for: .bonjour(type: "_streamsync._tcp", domain: nil), using: .tcp)
        self.browser = browser
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            guard let endpoint = results.first?.endpoint else { return }
            Task { @MainActor in self?.resolve(endpoint) }
        }
        browser.start(queue: .main)
        // Give up quietly if no Mac answers.
        Task {
            try? await Task.sleep(for: .seconds(8))
            if self.browser === browser, self.probing == nil { self.finish() }
        }
    }

    /// Connects once to learn the Mac's address, then hands the URL to the core.
    private func resolve(_ endpoint: NWEndpoint) {
        guard probing == nil else { return }
        let params = NWParameters.tcp
        params.requiredInterfaceType = .wifi
        if let ip = params.defaultProtocolStack.internetProtocol as? NWProtocolIP.Options { ip.version = .v4 }
        let conn = NWConnection(to: endpoint, using: params)
        probing = conn
        conn.stateUpdateHandler = { [weak self] state in
            guard case .ready = state else {
                if case .failed = state { Task { @MainActor in self?.finish() } }
                return
            }
            let remote = conn.currentPath?.remoteEndpoint
            conn.cancel()
            Task { @MainActor in
                guard let self, case let .hostPort(host, port)? = remote else { self?.finish(); return }
                var h = "\(host)"
                if let pct = h.firstIndex(of: "%") { h = String(h[..<pct]) }
                if h.contains(":") { h = "[\(h)]" }
                let url = "http://\(h):\(port.rawValue)"
                do {
                    try await AppModel.shared.core.syncFrom(baseUrl: url)
                } catch {
                    AppModel.shared.show("Sync failed: \(error.localizedDescription)")
                }
                self.finish()
            }
        }
        conn.start(queue: .main)
    }

    private func finish() {
        browser?.cancel()
        browser = nil
        probing = nil
        busy = false
    }
}
