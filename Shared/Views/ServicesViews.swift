import StreamBindings
import SwiftUI
import WebKit

/// Settings → Services: one row per service in the order search and playback try them, as a
/// native inset list. Hold and drag a row to reorder it; the row opens its own page, where it
/// can be turned off.
struct ServicesList: View {
    @Binding var settings: Preferences
    @Environment(AppModel.self) private var model

    private static let rowHeight: CGFloat = 52

    /// Every service, in order; one an older version didn't list goes last.
    private var order: [Service] {
        settings.services + [Service.files, .spotify, .soundCloud].filter { !settings.services.contains($0) }
    }

    var body: some View {
        List {
            Section {
                ForEach(order) { s in
                    NavigationLink(value: s) {
                        LabeledContent {
                            Text(status(s))
                        } label: {
                            Label { Text(s.title) } icon: { ServiceIcon(service: s) }
                        }
                    }
                    .listRowBackground(Color.clear)
                    .listRowSeparator(s == order.first ? .hidden : .visible, edges: .top)
                    .listRowSeparator(s == order.last ? .hidden : .visible, edges: .bottom)
                    .accessibilityIdentifier("settings.service.\(s.title)")
                }
                .onMove { from, to in
                    var o = order
                    o.move(fromOffsets: from, toOffset: to)
                    Haptics.tap()
                    withAnimation(.snappy) { settings.services = o }
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .scrollDisabled(true)
        .frame(height: Self.rowHeight * CGFloat(order.count))
        .environment(\.defaultMinListRowHeight, Self.rowHeight)
        .padding(.vertical, 6)
        .glassEffect(.regular, in: .rect(cornerRadius: 24))
    }

    /// The row's value: who it's signed in as, or that it's off.
    private func status(_ s: Service) -> String {
        let a = model.account
        if settings.servicesOff.contains(s) { return "Off" }
        return switch s {
        case .files: ""
        case .spotify:
            model.signingIn ? "Signing in…"
                : a.spotifyWeb ? (a.spotifyUser ?? (a.spotifyPremium ? "Premium" : "Free"))
                : "Not connected"
        case .soundCloud: a.soundcloudUser ?? ""
        }
    }
}

/// A service's mark, the size of Settings' own icons: Stream's own icon for your music, and
/// Spotify's and SoundCloud's real logos on their app icons' colours.
struct ServiceIcon: View {
    let service: Service
    var size: CGFloat = 30

    var body: some View {
        Group {
            switch service {
            case .files:
                Image("Icon").resizable().scaledToFill()
                    .background(.white)
            case .spotify:
                Image("Spotify").resizable().scaledToFit()
                    .foregroundStyle(Color(red: 0.12, green: 0.84, blue: 0.38))
                    .padding(size * 0.17)
                    .background(.black)
            case .soundCloud:
                Image("SoundCloud").resizable().scaledToFit()
                    .foregroundStyle(.white)
                    .padding(size * 0.15)
                    .background(LinearGradient(colors: [Color(red: 1, green: 0.47, blue: 0), Color(red: 1, green: 0.2, blue: 0)],
                                               startPoint: .top, endPoint: .bottom))
            }
        }
        .frame(width: size, height: size)
        .clipShape(.rect(cornerRadius: size * 0.225, style: .continuous))
        .overlay {
            // Stream's white icon keeps its edge on a white page, as Settings does.
            if service == .files {
                RoundedRectangle(cornerRadius: size * 0.225, style: .continuous)
                    .strokeBorder(.separator, lineWidth: 0.5)
            }
        }
        .accessibilityHidden(true)
    }
}

/// One service's own page: its mark and name, then its settings.
struct ServicePage<Content: View>: View {
    let service: Service
    @ViewBuilder let content: Content
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                VStack(spacing: 10) {
                    ServiceIcon(service: service, size: 64)
                    Text(service.title).font(.title2.weight(.bold))
                }
                .frame(maxWidth: .infinity)
                .padding(.top, 12)
                content
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 30)
        }
        .navigationTitle(service.title)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button("Done", systemImage: "checkmark") { model.showSettings = false }
            }
        }
        #endif
    }
}

/// Signs in on soundcloud.com itself, in a private browser of its own: once SoundCloud's
/// web player has its token, that's handed to the core and the sheet closes.
struct SoundCloudSignInSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var checking = false

    var body: some View {
        NavigationStack {
            SoundCloudWeb { token in
                guard !checking else { return }
                checking = true
                Task {
                    do {
                        try await model.core.soundcloudSignIn(token: token)
                        Haptics.success()
                        model.show("Signed in to SoundCloud")
                        dismiss()
                    } catch {
                        checking = false
                        model.show(AppModel.message(error))
                    }
                }
            }
            .overlay {
                if checking { ProgressView().controlSize(.large) }
            }
            .navigationTitle("SoundCloud")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", role: .cancel) { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(width: 480, height: 640)
        #endif
    }
}

#if os(iOS)
private typealias Representable = UIViewRepresentable
#else
private typealias Representable = NSViewRepresentable
#endif

/// soundcloud.com's sign-in page, watching its cookies for the web player's `oauth_token`.
private struct SoundCloudWeb: Representable {
    let signedIn: (String) -> Void

    func makeCoordinator() -> Watcher { Watcher(signedIn: signedIn) }

    private func make(_ context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        // Its own cookies, gone with the sheet: the token is what's kept, in the Keychain.
        config.websiteDataStore = .nonPersistent()
        let web = WKWebView(frame: .zero, configuration: config)
        config.websiteDataStore.httpCookieStore.add(context.coordinator)
        context.coordinator.store = config.websiteDataStore.httpCookieStore
        web.load(URLRequest(url: URL(string: "https://soundcloud.com/signin")!))
        return web
    }

    #if os(iOS)
    func makeUIView(context: Context) -> WKWebView { make(context) }
    func updateUIView(_ view: WKWebView, context: Context) {}
    #else
    func makeNSView(context: Context) -> WKWebView { make(context) }
    func updateNSView(_ view: WKWebView, context: Context) {}
    #endif

    final class Watcher: NSObject, WKHTTPCookieStoreObserver {
        let signedIn: (String) -> Void
        weak var store: WKHTTPCookieStore?

        init(signedIn: @escaping (String) -> Void) { self.signedIn = signedIn }

        func cookiesDidChange(in cookieStore: WKHTTPCookieStore) {
            cookieStore.getAllCookies { cookies in
                guard let token = cookies.first(where: { $0.name == "oauth_token" && $0.domain.hasSuffix("soundcloud.com") })?.value,
                      !token.isEmpty else { return }
                Task { @MainActor in self.signedIn(token) }
            }
        }
    }
}
