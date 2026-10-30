import SwiftUI

enum SidebarItem: Hashable {
    case home, search, section(LibrarySection), pin(Item)
}

/// Glass sidebar running the full height of the window, and a capsule playing
/// bar floating over the bottom of the page. Clicking the bar's art opens Now
/// Playing across the whole window.
struct MacRootView: View {
    @Environment(AppModel.self) private var model
    @Binding var showPalette: Bool
    @State private var selection: SidebarItem? = .home
    @State private var path = NavigationPath()
    @State private var search = SearchModel()
    @State private var sidebarEdge: CGFloat = 228

    var body: some View {
        ZStack {
            NavigationSplitView {
                sidebar
                    .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 300)
                    .toolbar(removing: .sidebarToggle)
                    .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).maxX } action: { sidebarEdge = $0 }
            } detail: {
                NavigationStack(path: $path) {
                    detail
                        .navigationDestination(for: Item.self) { CollectionView(item: $0) }
                        .navigationDestination(for: LibrarySection.self) { SectionView(section: $0) }
                }
            }
            // The bar floats over the page, which scrolls on beneath it. (Views placed
            // beside the NavigationStack inside the detail column don't get drawn, so it
            // hangs off the split view, lined up with the detail column.)
            .contentMargins(.bottom, 96, for: .scrollContent)
            .overlay(alignment: .bottomLeading) {
                PlayerBar()
                    .padding(.leading, sidebarEdge + 16)
                    .padding([.trailing, .bottom], 16)
            }
            .tint(model.tint)

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
                        .accessibilityLabel("Close Now Playing")
                        .accessibilityIdentifier("nowPlaying.close")
                        .padding(.leading, 80)
                        .padding(.top, 14)
                    }
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .zIndex(2)
            }
        }
        .overlay(alignment: .top) {
            if let t = model.toast { Toast(text: t).padding(.top, 12).accessibilityIdentifier("toast") }
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
        // Clicking the page you're on goes back to its top level.
        let pick = Binding<SidebarItem?>(get: { selection }, set: { new in
            if new == selection { path = NavigationPath() }
            selection = new
        })
        return List(selection: pick) {
            Label("Home", systemImage: "house").tag(SidebarItem.home).accessibilityIdentifier("sidebar.home")
            Label("Search", systemImage: "magnifyingglass").tag(SidebarItem.search).accessibilityIdentifier("sidebar.search")
            Section("Library") {
                ForEach(LibrarySection.ordered) { s in
                    Label(s.title, systemImage: s.symbol)
                        .tag(SidebarItem.section(s))
                        .accessibilityIdentifier("sidebar.\(s.title)")
                }
            }
            if !model.home.pins.isEmpty {
                Section("Pinned") {
                    ForEach(model.home.pins) { p in
                        HStack(spacing: 8) {
                            ArtworkView(url: p.art, size: 20, circle: p.kind == .artist)
                            Text(p.title).lineLimit(1)
                        }
                        .tag(SidebarItem.pin(p))
                        .accessibilityIdentifier("sidebar.pin.\(p.title)")
                        .itemMenu(p)
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .onChange(of: selection) { _, new in
            // A pinned song plays rather than opening a page.
            if case let .pin(item) = new, item.kind == .track {
                model.play(item)
                selection = .home
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch selection ?? .home {
        case .home:
            HomeView(columns: 6)
        case .search:
            MacSearchView(search: search)
        case let .section(s):
            SectionView(section: s).id(s)
        case let .pin(item):
            CollectionView(item: item).id(item.id)
        }
    }
}

/// Search with its field centered at the top of the page.
struct MacSearchView: View {
    @Bindable var search: SearchModel
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search your library", text: $search.query)
                    .textFieldStyle(.plain)
                    .font(.title3)
                    .focused($focused)
                    .accessibilityIdentifier("search.field")
                if !search.query.isEmpty {
                    Button { search.query = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Clear search")
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .glassEffect(.regular, in: .capsule)
            .frame(maxWidth: 520)
            .padding(.top, 14)
            .padding(.bottom, 6)
            SearchResultsView(search: search)
        }
        .navigationTitle("Search")
        .onAppear { focused = true }
    }
}

/// The floating capsule: song on the left, transport and scrubber in the
/// middle, extras on the right. The art sits far enough in that its rounded
/// corners clear the capsule's curve.
struct PlayerBar: View {
    @Environment(AppModel.self) private var model
    private let height: CGFloat = 64
    private let art: CGFloat = 44
    private let artRadius: CGFloat = 10

    var body: some View {
        let t = model.player.track
        HStack(spacing: 0) {
            // Song
            HStack(spacing: 12) {
                Button {
                    withAnimation(.player) { model.showNowPlaying = true }
                } label: {
                    ArtworkView(url: t?.art, size: art, cornerRadius: artRadius)
                }
                .buttonStyle(.pressable)
                .help("Now Playing")
                .accessibilityLabel("Now Playing")
                .accessibilityIdentifier("player.artwork")
                VStack(alignment: .leading, spacing: 2) {
                    Text(t?.title ?? "Not Playing")
                        .font(.callout.weight(.semibold)).lineLimit(1)
                        .accessibilityIdentifier("player.title")
                    Text(t?.artist ?? "").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                .id(t?.id)
                .transition(.blurReplace)
                if let t {
                    Button { model.setLiked(t, !t.liked) } label: {
                        Image(systemName: t.liked ? "heart.fill" : "heart")
                            .foregroundStyle(t.liked ? model.tint : .secondary)
                            .contentTransition(.symbolEffect(.replace))
                    }
                    .buttonStyle(.pressable)
                    .accessibilityLabel(t.liked ? "Unlike" : "Like")
                    .accessibilityIdentifier("player.like")
                }
                Spacer(minLength: 8)
            }
            .padding(.leading, (height - art) / 2 + 4)
            .frame(maxWidth: .infinity, alignment: .leading)

            // Transport + scrubber
            VStack(spacing: 4) {
                HStack(spacing: 22) {
                    Button { model.core.setShuffle(on: !model.player.shuffle) } label: {
                        Image(systemName: "shuffle").foregroundStyle(model.player.shuffle ? model.tint : .secondary)
                    }
                    .accessibilityLabel("Shuffle")
                    .accessibilityValue(model.player.shuffle ? "on" : "off")
                    .accessibilityIdentifier("player.shuffle")
                    Button { model.previous() } label: { Image(systemName: "backward.fill") }
                        .accessibilityLabel("Previous").accessibilityIdentifier("player.previous")
                    Button { model.toggle() } label: {
                        Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                            .font(.title2)
                            .contentTransition(.symbolEffect(.replace))
                            .frame(width: 26)
                    }
                    .accessibilityLabel(model.player.status == .playing ? "Pause" : "Play")
                    .accessibilityIdentifier("player.playPause")
                    Button { model.next() } label: { Image(systemName: "forward.fill") }
                        .accessibilityLabel("Next").accessibilityIdentifier("player.next")
                }
                .buttonStyle(PressableStyle(scale: 0.8))
                .disabled(t == nil)
                BarScrubber()
            }
            .frame(minWidth: 260, maxWidth: 360)

            // Extras
            HStack(spacing: 16) {
                modeButton(.lyrics, "quote.bubble", "Lyrics")
                modeButton(.queue, "list.bullet", "Queue")
                AirPlayButton().frame(width: 22, height: 22)
                HStack(spacing: 6) {
                    @Bindable var model = model
                    Image(systemName: model.volume == 0 ? "speaker.slash.fill" : "speaker.wave.2.fill").foregroundStyle(.secondary)
                    Slider(value: $model.volume, in: 0...1)
                        .frame(width: 90)
                        .controlSize(.small)
                        .accessibilityIdentifier("player.volume")
                }
            }
            .padding(.leading, 16)
            .padding(.trailing, height / 2)
            .fixedSize()
        }
        .frame(height: height)
        .glassEffect(.regular, in: .capsule)
        // Glass alone vanishes over a flat page; a hairline rim and a soft shadow lift it.
        .overlay { Capsule().strokeBorder(.primary.opacity(0.1), lineWidth: 0.5) }
        .shadow(color: .black.opacity(0.25), radius: 18, y: 8)
        .animation(.smooth, value: t?.id)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("player.bar")
    }

    private func modeButton(_ m: AppModel.NowPlayingMode, _ symbol: String, _ label: String) -> some View {
        let on = model.showNowPlaying && model.nowPlayingMode == m
        return Button {
            withAnimation(.player) {
                if on {
                    model.showNowPlaying = false
                } else {
                    model.nowPlayingMode = m
                    model.showNowPlaying = true
                }
            }
        } label: { Image(systemName: symbol).foregroundStyle(on ? model.tint : .primary) }
        .buttonStyle(.pressable)
        .accessibilityLabel(label)
        .accessibilityIdentifier("player.\(label.lowercased())")
    }
}

/// Thin scrubber with times; thickens on hover, click or drag to seek.
private struct BarScrubber: View {
    @Environment(AppModel.self) private var model
    @State private var hover = false
    @State private var dragging: Double?

    var body: some View {
        let dur = max(1, Double(model.player.durationMs) / 1000)
        TimelineView(.periodic(from: .now, by: 0.25)) { _ in
            let pos = dragging ?? min(dur, model.position())
            HStack(spacing: 8) {
                Text(Format.time(pos)).font(.caption2.monospacedDigit()).foregroundStyle(.secondary).frame(width: 34, alignment: .trailing)
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(.quaternary)
                        Capsule().fill(.primary.opacity(0.75)).frame(width: geo.size.width * pos / dur)
                    }
                    .frame(height: hover || dragging != nil ? 6 : 3)
                    .frame(maxHeight: .infinity)
                    .contentShape(Rectangle())
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { v in dragging = max(0, min(1, v.location.x / geo.size.width)) * dur }
                            .onEnded { _ in
                                if let d = dragging { model.seek(to: UInt32(d * 1000)) }
                                dragging = nil
                            }
                    )
                }
                .frame(height: 12)
                .accessibilityElement()
                .accessibilityLabel("Position")
                .accessibilityValue("\(Int(pos)) seconds")
                .accessibilityIdentifier("player.scrubber")
                Text("-" + Format.time(dur - pos)).font(.caption2.monospacedDigit()).foregroundStyle(.secondary).frame(width: 38, alignment: .leading)
            }
        }
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
                ArtworkView(url: t?.art, size: 54)
                VStack(alignment: .leading, spacing: 2) {
                    Text(t?.title ?? "Not Playing").font(.headline).lineLimit(1)
                    Text(t?.artist ?? "").font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
                if let q = t?.quality, !q.isEmpty { QualityChip(text: q) }
            }
            BarScrubber()
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
                    .accessibilityIdentifier("palette.field")
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
                                .background(RoundedRectangle(cornerRadius: 8).fill(i == selected ? model.tint.opacity(0.25) : .clear))
                                .id(i)
                                .onTapGesture { selected = i; choose() }
                                .accessibilityElement(children: .combine)
                                .accessibilityAddTraits(.isButton)
                                .accessibilityIdentifier("palette.result.\(item.title)")
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
