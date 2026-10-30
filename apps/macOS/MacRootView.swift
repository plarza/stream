import SwiftUI

enum SidebarItem: Hashable {
    case home, search, section(LibrarySection), pin(Item)
}

/// Glass sidebar, content, and a floating glass player capsule over it.
/// Clicking the capsule's art opens Now Playing across the whole window.
struct MacRootView: View {
    @Environment(AppModel.self) private var model
    @Binding var showPalette: Bool
    @State private var selection: SidebarItem? = .home
    @State private var path = NavigationPath()
    @State private var search = SearchModel()

    var body: some View {
        @Bindable var model = model
        ZStack {
            NavigationSplitView {
                sidebar
                    .navigationSplitViewColumnWidth(min: 190, ideal: 220)
            } detail: {
                NavigationStack(path: $path) {
                    detail
                        .withPlayer()
                        .navigationDestination(for: Item.self) { CollectionView(item: $0).withPlayer() }
                        .navigationDestination(for: LibrarySection.self) { SectionView(section: $0).withPlayer() }
                }
            }
            .tint(model.accent.accent)

            if model.showNowPlaying {
                NowPlayingView()
                    .overlay(alignment: .topLeading) {
                        Button { close() } label: {
                            Image(systemName: "chevron.down").font(.title3.weight(.semibold))
                                .frame(width: 36, height: 36)
                                .glassEffect(.regular.interactive(), in: .circle)
                        }
                        .buttonStyle(.pressable)
                        .keyboardShortcut(.escape, modifiers: [])
                        .padding(.leading, 80)
                        .padding(.top, 14)
                    }
                    .transition(.asymmetric(insertion: .move(edge: .bottom).combined(with: .opacity), removal: .move(edge: .bottom).combined(with: .opacity)))
                    .zIndex(2)
            }
        }
        .overlay(alignment: .top) {
            if let t = model.toast { Toast(text: t).padding(.top, 12) }
        }
        .sheet(isPresented: $showPalette) {
            CommandPalette { item in
                showPalette = false
                open(item)
            }
            .environment(model)
        }
        .onChange(of: selection) { path = NavigationPath() }
        .onChange(of: model.openRequested) {
            if let item = model.pendingNavigation {
                model.pendingNavigation = nil
                path.append(item)
            }
        }
        .onChange(of: model.showNowPlaying) { _, shown in
            if !shown, let item = model.pendingNavigation {
                model.pendingNavigation = nil
                path.append(item)
            }
        }
    }

    private func close() {
        withAnimation(.player) { model.showNowPlaying = false }
    }

    private func open(_ item: Item) {
        if item.kind == .track { model.play(item) } else { path.append(item) }
    }

    private var sidebar: some View {
        List(selection: $selection) {
            Label("Home", systemImage: "house").tag(SidebarItem.home)
            Label("Search", systemImage: "magnifyingglass").tag(SidebarItem.search)
            Section("Library") {
                ForEach(LibrarySection.ordered) { s in
                    Label(s.title, systemImage: s.symbol).tag(SidebarItem.section(s))
                }
            }
            if !model.home.pins.isEmpty {
                Section("Pinned") {
                    ForEach(model.home.pins) { p in
                        HStack(spacing: 8) {
                            ArtworkView(url: p.art, size: 20, cornerRadius: 4, circle: p.kind == .artist)
                            Text(p.title).lineLimit(1)
                        }
                        .tag(SidebarItem.pin(p))
                        .itemMenu(p)
                    }
                }
            }
        }
        .listStyle(.sidebar)
    }

    @ViewBuilder
    private var detail: some View {
        switch selection ?? .home {
        case .home:
            HomeView(columns: 6)
        case .search:
            SearchResultsView(search: search)
                .navigationTitle("Search")
                .searchable(text: $search.query, placement: .toolbar, prompt: "Songs, albums, artists, playlists")
        case let .section(s):
            SectionView(section: s)
        case let .pin(item):
            if item.kind == .track {
                HomeView(columns: 6).onAppear { model.play(item) }
            } else {
                CollectionView(item: item).id(item.id)
            }
        }
    }
}

extension View {
    /// Every page carries the capsule, so it floats over whatever is shown.
    func withPlayer() -> some View {
        safeAreaInset(edge: .bottom) {
            PlayerCapsule()
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
        }
    }
}

/// The floating glass player at the bottom of the window.
struct PlayerCapsule: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let t = model.player.track
        GlassEffectContainer {
            HStack(spacing: 14) {
                Button {
                    withAnimation(.player) { model.showNowPlaying = true }
                } label: {
                    ArtworkView(url: t?.art, size: 40, cornerRadius: 7)
                }
                .buttonStyle(.pressable)
                .help("Now Playing")
                VStack(alignment: .leading, spacing: 1) {
                    Text(t?.title ?? "Not Playing").font(.callout.weight(.semibold)).lineLimit(1)
                    Text(t?.artist ?? "").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                .frame(width: 170, alignment: .leading)
                .id(t?.id)
                .transition(.push(from: .bottom).combined(with: .opacity))
                HStack(spacing: 18) {
                    Button { model.previous() } label: { Image(systemName: "backward.fill") }
                    Button { model.toggle() } label: {
                        Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                            .font(.title2)
                            .contentTransition(.symbolEffect(.replace))
                            .frame(width: 26)
                    }
                    Button { model.next() } label: { Image(systemName: "forward.fill") }
                }
                .buttonStyle(PressableStyle(scale: 0.8))
                .disabled(t == nil)
                CapsuleScrubber()
                if let q = t?.quality, !q.isEmpty { QualityChip(text: q) }
                if let t {
                    Button { model.setLiked(t, !t.liked) } label: {
                        Image(systemName: t.liked ? "heart.fill" : "heart")
                            .foregroundStyle(t.liked ? model.accent.accent : .primary)
                            .contentTransition(.symbolEffect(.replace))
                    }
                    .buttonStyle(.pressable)
                }
                modeButton(.lyrics, "quote.bubble")
                modeButton(.queue, "list.bullet")
                AirPlayButton().frame(width: 24, height: 24)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .glassEffect(.regular, in: .capsule)
        }
        .animation(.smooth, value: t?.id)
    }

    private func modeButton(_ m: AppModel.NowPlayingMode, _ symbol: String) -> some View {
        Button {
            withAnimation(.player) {
                model.nowPlayingMode = m
                model.showNowPlaying = true
            }
        } label: { Image(systemName: symbol) }
        .buttonStyle(.pressable)
    }
}

private struct CapsuleScrubber: View {
    @Environment(AppModel.self) private var model
    @State private var hover = false

    var body: some View {
        let dur = max(1, Double(model.player.durationMs) / 1000)
        TimelineView(.periodic(from: .now, by: 0.5)) { _ in
            let pos = min(dur, model.position())
            HStack(spacing: 8) {
                Text(Format.time(pos)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(.quaternary)
                        Capsule().fill(.primary.opacity(0.8)).frame(width: geo.size.width * pos / dur)
                    }
                    .frame(height: hover ? 7 : 4)
                    .frame(maxHeight: .infinity)
                    .contentShape(Rectangle())
                    .onTapGesture { p in model.seek(to: UInt32(p.x / geo.size.width * dur * 1000)) }
                    .gesture(DragGesture(minimumDistance: 2).onEnded { v in
                        model.seek(to: UInt32(max(0, min(1, v.location.x / geo.size.width)) * dur * 1000))
                    })
                }
                .frame(height: 14)
                Text("-" + Format.time(dur - pos)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }
        }
        .frame(minWidth: 140)
        .onHover { h in withAnimation(.press) { hover = h } }
    }
}

/// Menu bar: compact card with art, scrubber and transport.
struct MenuBarCard: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        let t = model.player.track
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                ArtworkView(url: t?.art, size: 54, cornerRadius: 9)
                VStack(alignment: .leading, spacing: 2) {
                    Text(t?.title ?? "Not Playing").font(.headline).lineLimit(1)
                    Text(t?.artist ?? "").font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
                if let q = t?.quality, !q.isEmpty { QualityChip(text: q) }
            }
            CapsuleScrubber()
            HStack(spacing: 28) {
                Spacer()
                Button { model.previous() } label: { Image(systemName: "backward.fill") }
                Button { model.toggle() } label: {
                    Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                        .font(.title2).contentTransition(.symbolEffect(.replace)).frame(width: 28)
                }
                Button { model.next() } label: { Image(systemName: "forward.fill") }
                Spacer()
            }
            .buttonStyle(PressableStyle(scale: 0.8))
            .font(.title3)
            Divider()
            HStack {
                Button("Open Music") {
                    NSApp.setActivationPolicy(.regular)
                    openWindow(id: "main")
                    NSApp.activate()
                }
                Spacer()
                Button("Quit") { NSApp.terminate(nil) }
            }
            .buttonStyle(.borderless)
            .font(.callout)
        }
        .padding(14)
        .frame(width: 290)
    }
}

/// ⌘K: search everything and jump straight to it.
struct CommandPalette: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var search = SearchModel()
    @State private var selected = 0
    @FocusState private var focused: Bool
    let open: (Item) -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Play or go to…", text: $search.query)
                    .textFieldStyle(.plain)
                    .font(.title3)
                    .focused($focused)
                    .onSubmit { choose() }
                    .onKeyPress(.downArrow) { selected = min(selected + 1, max(0, results.count - 1)); return .handled }
                    .onKeyPress(.upArrow) { selected = max(selected - 1, 0); return .handled }
                    .onKeyPress(.escape) { dismiss(); return .handled }
            }
            .padding(14)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(Array(results.enumerated()), id: \.element.id) { i, item in
                            ItemRow(item: item)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 4)
                                .background(RoundedRectangle(cornerRadius: 8).fill(i == selected ? Color.accentColor.opacity(0.25) : .clear))
                                .id(i)
                                .onTapGesture { selected = i; choose() }
                        }
                    }
                    .padding(8)
                }
                .onChange(of: selected) { _, s in withAnimation(.snappy) { proxy.scrollTo(s, anchor: .center) } }
            }
        }
        .frame(width: 560, height: 420)
        .onAppear { focused = true }
        .onChange(of: search.query) { selected = 0; search.run(model) }
    }

    private var results: [Item] { Array(search.results.prefix(40)) }

    private func choose() {
        guard results.indices.contains(selected) else { return }
        model.core.addRecentSearch(query: search.query)
        open(results[selected])
    }
}
