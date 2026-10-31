import AuthenticationServices
#if os(iOS)
import UIKit
#else
import AppKit
#endif

/// Shows Spotify's sign-in pages in the system's in-app browser sheet, on iPhone
/// and Mac alike, so the default browser never matters. The loopback page hands
/// the code to the core, then redirects to network-aza-stream://auth-done, which
/// closes the sheet. Closing it by hand cancels the sign-in in the core.
@MainActor
final class WebAuth: NSObject, ASWebAuthenticationPresentationContextProviding {
    static let shared = WebAuth()
    private var session: ASWebAuthenticationSession?
    private var generation = 0

    func open(_ url: URL, onCancel: @escaping @MainActor () -> Void) {
        session?.cancel()
        generation += 1
        let mine = generation
        let session = ASWebAuthenticationSession(url: url, callback: .customScheme("network-aza-stream")) { @Sendable _, error in
            let cancelled = (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin
            Task { @MainActor in
                if cancelled, self.generation == mine { onCancel() }
            }
        }
        session.presentationContextProvider = self
        session.prefersEphemeralWebBrowserSession = false
        self.session = session
        // The Web API sheet may still be animating away when the playback one opens.
        Task {
            for _ in 0..<20 where self.generation == mine {
                if session.start() { return }
                try? await Task.sleep(for: .milliseconds(250))
            }
        }
    }

    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            #if os(iOS)
            let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
            return scenes.compactMap(\.keyWindow).first ?? scenes.first.map { UIWindow(windowScene: $0) } ?? ASPresentationAnchor()
            #else
            return NSApp.keyWindow ?? NSApp.windows.first { $0.isVisible } ?? ASPresentationAnchor()
            #endif
        }
    }
}
