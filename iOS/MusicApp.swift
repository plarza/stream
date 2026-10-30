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
                        .accountButton { showSettings = true }
                        .destinations()
                }
            }
            Tab("Library", systemImage: "square.stack.fill", value: .library) {
                NavigationStack(path: $libraryPath) {
                    LibraryView()
                        .accountButton { showSettings = true }
                        .destinations()
                }
            }
            Tab(value: .search, role: .search) {
                NavigationStack(path: $searchPath) {
                    SearchResultsView(search: search)
                        .accountButton { showSettings = true }
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
        .preferredColorScheme(model.colorScheme)
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
        .onChange(of: model.openRequested) {
            // Deep links and search results open in the tab you're on.
            if model.showNowPlaying { model.showNowPlaying = false } else { openPending() }
        }
        .onChange(of: model.resetRequested) {
            showSettings = false
            search.query = ""
            homePath = NavigationPath()
            libraryPath = NavigationPath()
            searchPath = NavigationPath()
            tab = .home
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
    /// Top-level tabs have no title, just the account button (opens Settings).
    func accountButton(_ open: @escaping () -> Void) -> some View {
        toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: open) {
                    Image(systemName: "person.crop.circle")
                        .font(.system(size: 22, weight: .semibold))
                }
                .accessibilityLabel("Account and Settings")
                .accessibilityIdentifier("account")
            }
        }
    }

    /// Album/playlist/artist pages and library sections, wherever we navigate from.
    func destinations() -> some View {
        navigationDestination(for: Item.self) { CollectionView(item: $0) }
            .navigationDestination(for: LibrarySection.self) { SectionView(section: $0) }
    }
}

/// The glass pill above the tab bar, laid out like Apple Music's: 30pt art
/// 16pt in, the title beside it, then play and next in 44pt cells.
struct MiniPlayer: View {
    @Environment(AppModel.self) private var model
    @Environment(\.tabViewBottomAccessoryPlacement) private var placement

    var body: some View {
        let t = model.player.track
        let inline = placement == .inline
        HStack(spacing: 0) {
            art(t, size: inline ? 24 : 30)
            VStack(alignment: .leading, spacing: 0) {
                Text(t?.title ?? "Not Playing")
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                    .accessibilityIdentifier("mini.title")
                if !inline, let a = t?.artist {
                    Text(a).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            .padding(.leading, 9)
            .id(t?.id)
            .transition(.blurReplace)
            Spacer(minLength: 4)
            Button { model.toggle() } label: {
                Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                    .font(.system(size: 18))
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(PressableStyle(scale: 0.8))
            .foregroundStyle(.primary)
            .accessibilityLabel(model.player.status == .playing ? "Pause" : "Play")
            .accessibilityIdentifier("mini.playPause")
            if !inline {
                Button { model.next() } label: {
                    Image(systemName: "forward.fill")
                        .font(.system(size: 18))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(PressableStyle(scale: 0.8))
                .foregroundStyle(t == nil ? .tertiary : .primary)
                .disabled(t == nil)
                .accessibilityLabel("Next")
                .accessibilityIdentifier("mini.next")
            }
        }
        .padding(.leading, inline ? 8 : 16)
        .padding(.trailing, 7)
        .contentShape(Rectangle())
        .animation(.smooth, value: t?.id)
    }

    @ViewBuilder
    private func art(_ t: Track?, size: CGFloat) -> some View {
        if let t {
            ArtworkView(url: t.art, size: size, cornerRadius: 6)
        } else {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(.primary.opacity(0.15))
                .frame(width: size, height: size)
                .overlay {
                    Image(systemName: "music.note")
                        .font(.system(size: size * 0.5, weight: .medium))
                        .foregroundStyle(.secondary)
                }
        }
    }
}
