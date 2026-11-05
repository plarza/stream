import StreamBindings
import SwiftUI
import WebKit

/// Settings → Services: one row per service in the order search and playback try them. Drag
/// a row to reorder it; the switch turns a service off without signing out; the row opens
/// its own page.
struct ServicesList: View {
    @Binding var settings: Preferences
    @Binding var soundCloudSignIn: Bool
    @Environment(AppModel.self) private var model

    private static let rowHeight: CGFloat = 58

    /// Every service, in order; one an older version didn't list goes last.
    private var order: [Service] {
        settings.services + [Service.files, .spotify, .soundCloud].filter { !settings.services.contains($0) }
    }

    var body: some View {
        List {
            ForEach(order) { s in
                NavigationLink(value: s) { row(s) }
                    .listRowBackground(Color.clear)
                    .listRowSeparator(s == order.last ? .hidden : .visible, edges: .bottom)
                    .listRowSeparator(s == order.first ? .hidden : .visible, edges: .top)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                    .accessibilityIdentifier("settings.service.\(s.title)")
            }
            .onMove { from, to in
                var o = order
                o.move(fromOffsets: from, toOffset: to)
                Haptics.tap()
                withAnimation(.snappy) { settings.services = o }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .scrollDisabled(true)
        .frame(height: Self.rowHeight * CGFloat(order.count))
        .environment(\.defaultMinListRowHeight, Self.rowHeight)
    }

    private func row(_ s: Service) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "line.3.horizontal")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.tertiary)
                .accessibilityHidden(true)
            ServiceIcon(service: s)
            VStack(alignment: .leading, spacing: 2) {
                Text(s.title)
                    .foregroundStyle(on(s) ? .primary : .secondary)
                status(s)
            }
            Spacer(minLength: 8)
            trailing(s)
        }
        .frame(height: Self.rowHeight)
        .contentShape(Rectangle())
    }

    private func on(_ s: Service) -> Bool { !settings.servicesOff.contains(s) }

    private func status(_ s: Service) -> some View {
        let a = model.account
        let (text, connected): (String, Bool) = switch s {
        case .files: ("Your files and Uploads", true)
        case .spotify:
            model.signingIn ? ("Signing in…", false)
                : a.spotifyWeb ? ([a.spotifyUser, a.spotifyPremium ? "Premium" : "Free"].compactMap { $0 }.joined(separator: " · "), a.spotifyPlayback)
                : ("Not connected", false)
        case .soundCloud: (a.soundcloudUser ?? "Not signed in", a.soundcloudUser != nil)
        }
        return HStack(spacing: 5) {
            if s != .files {
                Circle().fill(connected ? Color.green : Color.secondary.opacity(0.6)).frame(width: 6, height: 6)
            }
            Text(text).lineLimit(1)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }

    @ViewBuilder
    private func trailing(_ s: Service) -> some View {
        switch s {
        case .files:
            EmptyView()
        case .spotify where !model.account.spotifyWeb:
            // Not connected yet: the row's own page signs in.
            Text("Connect")
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .foregroundStyle(.white)
                .background(Color.accentColor, in: .capsule)
        default:
            // SoundCloud searches and plays without an account, so it can always be on.
            Toggle(s.title, isOn: Binding(get: { on(s) }, set: { v in
                withAnimation(.snappy) {
                    settings.servicesOff.removeAll { $0 == s }
                    if !v { settings.servicesOff.append(s) }
                }
            }))
            .labelsHidden()
            .accessibilityIdentifier("settings.toggle.\(s.title)")
        }
    }
}

/// A service's mark: its colour and a symbol, the same size as Settings' own icons.
struct ServiceIcon: View {
    let service: Service
    var size: CGFloat = 30

    var body: some View {
        let (symbol, colors): (String, [Color]) = switch service {
        case .files: ("music.note", [Color(red: 0.98, green: 0.35, blue: 0.42), Color(red: 0.98, green: 0.18, blue: 0.28)])
        case .spotify: ("waveform", [Color(red: 0.12, green: 0.84, blue: 0.38), Color(red: 0.08, green: 0.66, blue: 0.29)])
        case .soundCloud: ("cloud.fill", [Color(red: 1, green: 0.47, blue: 0), Color(red: 1, green: 0.2, blue: 0)])
        }
        Image(systemName: symbol)
            .font(.system(size: size * 0.46, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(LinearGradient(colors: colors, startPoint: .top, endPoint: .bottom), in: .rect(cornerRadius: size * 0.26))
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
