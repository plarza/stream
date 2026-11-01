import SwiftUI

@main
struct StreamApp: App {
    @State private var model = AppModel.shared
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .onContinueUserActivity("network.aza.stream.playing") { model.continueActivity($0) }
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
    @Namespace private var zoom

    enum TabID: Hashable { case home, library, search }

    var body: some View {
        @Bindable var model = model
        TabView(selection: $tab) {
            Tab("Home", systemImage: "house.fill", value: .home) {
                NavigationStack(path: $homePath) {
                    HomeView()
                        .navigationTitle("Home")
                        .accountButton { model.showSettings = true }
                        .destinations()
                }
            }
            Tab("Library", systemImage: "square.stack.fill", value: .library) {
                NavigationStack(path: $libraryPath) {
                    LibraryView()
                        .navigationTitle("Library")
                        .accountButton { model.showSettings = true }
                        .destinations()
                }
            }
            Tab(value: .search, role: .search) {
                NavigationStack(path: $searchPath) {
                    SearchResultsView(search: search)
                        .navigationTitle("Search")
                        .accountButton { model.showSettings = true }
                        .destinations()
                }
                .searchable(text: $search.query, prompt: "Search your library")
            }
        }
        // The playing bar stays full size while lists scroll.
        .tabBarMinimizeBehavior(.never)
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
        .sheet(isPresented: $model.showSettings) {
            NavigationStack {
                SettingsView()
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) { Button("Done") { model.showSettings = false } }
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
            model.showSettings = false
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
        // Already there: going to the same page again adds nothing to Back.
        guard item.id != model.currentPage else { return }
        switch tab {
        case .home: homePath.append(item)
        case .library: libraryPath.append(item)
        case .search: searchPath.append(item)
        }
    }
}

extension View {
    /// The account button level with each tab's large title (opens Settings).
    func accountButton(_ open: @escaping () -> Void) -> some View {
        toolbarTitleDisplayMode(.inlineLarge)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: open) { AccountAvatar() }
                        .accessibilityLabel("Account and Settings")
                        .accessibilityIdentifier("account")
                }
                .sharedBackgroundVisibility(.hidden)
            }
    }

    /// Album/playlist/artist pages and library sections, wherever we navigate from.
    func destinations() -> some View {
        navigationDestination(for: Item.self) { CollectionView(item: $0) }
            .navigationDestination(for: LibrarySection.self) { SectionView(section: $0) }
    }
}

/// Your Spotify initials on a tinted circle, or a person when signed out.
struct AccountAvatar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let initials = Self.initials(model.account.spotifyUser)
        ZStack {
            Circle().fill(LinearGradient(colors: [model.tint.opacity(0.75), model.tint], startPoint: .top, endPoint: .bottom))
            if initials.isEmpty {
                Image(systemName: "person.fill").font(.system(size: 17, weight: .semibold))
            } else {
                Text(initials).font(.system(size: 15, weight: .semibold, design: .rounded))
            }
        }
        .foregroundStyle(.white)
        .frame(width: 36, height: 36)
        // A ring of glass around the avatar, like the system's own account buttons.
        .padding(3)
        .glassEffect(.regular.interactive(), in: .circle)
        .contentTransition(.opacity)
        .animation(.smooth, value: initials)
    }

    static func initials(_ name: String?) -> String {
        let words = (name ?? "").split(whereSeparator: { $0 == " " || $0 == "." || $0 == "_" }).filter { $0.first?.isLetter == true }
        return String(words.prefix(2).compactMap(\.first)).uppercased()
    }
}

/// The glass pill above the tab bar, laid out like Apple Music's: art 16pt in,
/// title and artist beside it, then play and next in 44pt cells.
struct MiniPlayer: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let t = model.player.track
        let playing = model.player.status == .playing
        HStack(spacing: 0) {
            art(t)
            VStack(alignment: .leading, spacing: 0) {
                MarqueeText(text: t?.title ?? "Not Playing")
                    .font(.system(size: 15, weight: .semibold))
                    .accessibilityIdentifier("mini.title")
                if let a = t?.artist {
                    Text(a).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            .padding(.leading, 10)
            .id(t?.id)
            .transition(.blurReplace)
            Spacer(minLength: 4)
            Button { model.toggle() } label: {
                Image(systemName: playing ? "pause.fill" : "play.fill")
                    .font(.system(size: 19))
                    .contentTransition(.symbolEffect(.replace))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(PressableStyle(scale: 0.8))
            .foregroundStyle(.primary)
            .disabled(t == nil)
            .accessibilityLabel(playing ? "Pause" : "Play")
            .accessibilityIdentifier("mini.playPause")
            Button { model.next() } label: {
                Image(systemName: "forward.fill")
                    .font(.system(size: 19))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(PressableStyle(scale: 0.8))
            .foregroundStyle(t == nil ? .tertiary : .primary)
            .disabled(t == nil)
            .accessibilityLabel("Next")
            .accessibilityIdentifier("mini.next")
        }
        .padding(.leading, 12)
        .padding(.trailing, 6)
        .frame(minHeight: 56)
        .contentShape(Rectangle())
        .animation(.smooth, value: t?.id)
    }

    @ViewBuilder
    private func art(_ t: Track?) -> some View {
        if let t {
            ArtworkView(url: t.art, size: 38, cornerRadius: 7)
        } else {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(.quaternary)
                .frame(width: 38, height: 38)
                .overlay {
                    Image(systemName: "music.note")
                        .font(.system(size: 16, weight: .medium))
                        .foregroundStyle(.secondary)
                }
        }
    }
}
