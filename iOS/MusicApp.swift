import AuthenticationServices
import SwiftUI

@main
struct MusicApp: App {
    @State private var model = AppModel.shared
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .onContinueUserActivity("network.aza.music.playing") { model.continueActivity($0) }
                .onOpenURL { model.handle(url: $0) }
                .onChange(of: phase) { _, p in
                    if p == .active {
                        SyncClient.shared.syncNow()
                        model.core.rescan()
                        Task { try? await model.core.refreshSpotify(force: false) }
                    }
                }
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model
    @State private var tab: TabID = .home
    @State private var search = SearchModel()
    @State private var homePath = NavigationPath()
    @State private var libraryPath = NavigationPath()
    @State private var searchPath = NavigationPath()
    @State private var showSettings = false
    @Namespace private var zoom

    enum TabID: Hashable { case home, library, search }

    var body: some View {
        @Bindable var model = model
        TabView(selection: $tab) {
            Tab("Home", systemImage: "house.fill", value: .home) {
                NavigationStack(path: $homePath) {
                    HomeView()
                        .toolbar {
                            ToolbarItem(placement: .topBarTrailing) {
                                Button { showSettings = true } label: { Image(systemName: "gearshape") }
                                    .accessibilityLabel("Settings")
                                    .accessibilityIdentifier("home.settings")
                            }
                        }
                        .destinations()
                }
            }
            Tab("Library", systemImage: "square.stack.fill", value: .library) {
                NavigationStack(path: $libraryPath) {
                    LibraryView().destinations()
                }
            }
            Tab(value: .search, role: .search) {
                NavigationStack(path: $searchPath) {
                    SearchResultsView(search: search)
                        .navigationTitle("Search")
                        .destinations()
                }
                .searchable(text: $search.query, prompt: "Search your library")
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        .tabViewBottomAccessory {
            MiniPlayer()
                .matchedTransitionSource(id: "nowplaying", in: zoom)
                .onTapGesture { model.showNowPlaying = true }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("miniPlayer")
        }
        .tint(model.tint)
        .fullScreenCover(isPresented: $model.showNowPlaying, onDismiss: openPending) {
            NowPlayingView()
                .navigationTransition(.zoom(sourceID: "nowplaying", in: zoom))
        }
        .sheet(isPresented: $showSettings) {
            NavigationStack {
                SettingsView()
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) { Button("Done") { showSettings = false } }
                    }
            }
        }
        .overlay(alignment: .top) {
            if let t = model.toast { Toast(text: t).padding(.top, 8) }
        }
        .background { WebAuthPresenter(url: $model.signInURL) }
        .onChange(of: model.openRequested) {
            // Deep links and search results open in the tab you're on.
            if model.showNowPlaying { model.showNowPlaying = false } else { openPending() }
        }
    }

    private func openPending() {
        guard let item = model.pendingNavigation else { return }
        model.pendingNavigation = nil
        switch tab {
        case .home: homePath.append(item)
        case .library: libraryPath.append(item)
        case .search: searchPath.append(item)
        }
    }
}

extension View {
    /// Album/playlist/artist pages and library sections, wherever we navigate from.
    func destinations() -> some View {
        navigationDestination(for: Item.self) { CollectionView(item: $0) }
            .navigationDestination(for: LibrarySection.self) { SectionView(section: $0) }
    }
}

/// The glass bar above the tab bar.
struct MiniPlayer: View {
    @Environment(AppModel.self) private var model
    @Environment(\.tabViewBottomAccessoryPlacement) private var placement

    var body: some View {
        let t = model.player.track
        HStack(spacing: 10) {
            ArtworkView(url: t?.art, size: placement == .inline ? 28 : 36)
            VStack(alignment: .leading, spacing: 0) {
                Text(t?.title ?? "Not Playing").font(.subheadline.weight(.semibold)).lineLimit(1)
                    .accessibilityIdentifier("mini.title")
                if placement != .inline, let a = t?.artist {
                    Text(a).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            .id(t?.id)
            .transition(.blurReplace)
            Spacer(minLength: 4)
            Button { model.toggle() } label: {
                Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                    .font(.title3)
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 36, height: 36)
            }
            .buttonStyle(PressableStyle(scale: 0.8))
            .accessibilityLabel(model.player.status == .playing ? "Pause" : "Play")
            .accessibilityIdentifier("mini.playPause")
            if placement != .inline {
                Button { model.next() } label: {
                    Image(systemName: "forward.fill").font(.body).frame(width: 32, height: 36)
                }
                .buttonStyle(PressableStyle(scale: 0.8))
                .accessibilityLabel("Next")
                .accessibilityIdentifier("mini.next")
            }
        }
        .padding(.horizontal, 10)
        .contentShape(Rectangle())
        .animation(.smooth, value: t?.id)
        .disabled(t == nil)
    }
}

/// Runs Spotify's sign-in pages in an in-app browser sheet. The loopback page
/// redirects to network-aza-music://auth-done, which closes it.
struct WebAuthPresenter: UIViewControllerRepresentable {
    @Binding var url: URL?

    func makeUIViewController(context: Context) -> UIViewController { UIViewController() }

    func updateUIViewController(_ vc: UIViewController, context: Context) {
        guard let url, context.coordinator.current != url else { return }
        context.coordinator.current = url
        let session = ASWebAuthenticationSession(url: url, callback: .customScheme("network-aza-music")) { _, _ in
            Task { @MainActor in
                context.coordinator.current = nil
                self.url = nil
            }
        }
        session.presentationContextProvider = context.coordinator
        session.prefersEphemeralWebBrowserSession = false
        context.coordinator.session = session
        session.start()
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, ASWebAuthenticationPresentationContextProviding {
        var current: URL?
        var session: ASWebAuthenticationSession?
        func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
            MainActor.assumeIsolated {
                UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first ?? ASPresentationAnchor()
            }
        }
    }
}
