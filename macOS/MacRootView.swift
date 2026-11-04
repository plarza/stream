import StreamBindings
import AzaUI
import SwiftUI

enum SidebarItem: Hashable {
    case home, search, section(LibrarySection), pin(Item)
}

/// A glass sidebar floating at the top left, only as tall as what's in it; the
/// page beside it, under one search field that stays put across tabs; and a
/// capsule playing bar floating centered over the bottom of the page. Clicking
/// the bar's art opens Now Playing across the whole window.
struct MacRootView: View {
    @Environment(\.accent) private var accent
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    @Binding var showPalette: Bool
    @State private var selection: SidebarItem = .home
    @State private var path = NavigationPath()
    @State private var search = SearchModel()
    /// What the field holds while a library section is showing (it filters that section).
    @State private var sectionFilter = ""
    @State private var sidebarHeight: CGFloat = 0
    @FocusState private var searchFocused: Bool
    @AppStorage(Zoom.key) private var zoom = Zoom.standard

    private let sidebarWidth: CGFloat = 220
    /// The sidebar's margin from the window edge: macOS's own 8pt at any zoom, so the
    /// traffic lights (which macOS places and never scales) keep the same room around them.
    private var inset: CGFloat { 8 / zoom }
    private var pageLeading: CGFloat { sidebarWidth + inset * 2 }
    /// The row holding the back button and search field, level with the traffic lights.
    private let topBar: CGFloat = 56

    var body: some View {
        ZStack(alignment: .topLeading) {
            if model.pureBlack && scheme == .dark {
                Color.black.ignoresSafeArea()
            }
            NavigationStack(path: $path) {
                detail
                    .navigationDestination(for: Item.self) { CollectionView(item: $0).navigationBarBackButtonHidden() }
                    .navigationDestination(for: LibrarySection.self) { SectionView(section: $0).navigationBarBackButtonHidden() }
            }
            .toolbar(removing: .title)
            // A toolbar gives the window its 52pt toolbar row, which is what puts the
            // traffic lights in from the corner, inside the sidebar. One invisible item
            // makes it exist; a spacer would cover the search field and take its clicks.
            .toolbar {
                ToolbarItem(placement: .primaryAction) { Color.clear.frame(width: 1, height: 1) }
                    .sharedBackgroundVisibility(.hidden)
            }
            .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
            // Pages start under the top row (an inset, which they still scroll under)
            // instead of the title bar's, and scroll on beneath the playing bar.
            .safeAreaPadding(.top, topBar)
            .ignoresSafeArea(.container, edges: .top)
            .contentMargins(.bottom, 96, for: .scrollContent)
            // Inset rather than padded, so page backgrounds run on under the sidebar.
            .safeAreaPadding(.leading, pageLeading)
            .overlay(alignment: .top) {
                topRow
                    .padding(.leading, pageLeading)
                    .ignoresSafeArea(.container, edges: .top)
            }
            .overlay(alignment: .bottom) {
                PlayerBar()
                    .frame(maxWidth: 990)
                    .padding(.horizontal, 16)
                    .padding(.leading, pageLeading)
                    .padding(.bottom, 16)
            }
            .tint(accent)

            GeometryReader { geo in
                sidebar(maxHeight: geo.size.height - inset * 2 - 96)
            }
            .padding(inset)
            .ignoresSafeArea()

            if model.showNowPlaying {
                NowPlayingView()
                    .overlay(alignment: .top) {
                        Button { close() } label: {
                            Image(systemName: "chevron.down")
                                .font(.system(size: 20, weight: .semibold))
                                .foregroundStyle(.white.opacity(0.75))
                                .frame(width: 64, height: 40)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.pressable)
                        .keyboardShortcut(.escape, modifiers: [])
                        .help("Close")
                        .accessibilityLabel("Close Now Playing")
                        .accessibilityIdentifier("nowPlaying.close")
                        .padding(.top, 6)
                    }
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .zIndex(2)
            }
        }
        .overlay(alignment: .top) {
            if let t = model.toast { Toast(text: t).padding(.top, 12).accessibilityIdentifier("toast") }
        }
        .background {
            // ⌘F goes to the search field from anywhere.
            Button("Search") { focusSearch() }
                .keyboardShortcut("f", modifiers: .command)
                .hidden()
        }
        .sheet(isPresented: $showPalette) {
            CommandPalette { item in
                showPalette = false
                open(item)
            }
            .environment(model)
        }
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

    // MARK: Top row

    /// Root pages search; pages you've gone into have the back button instead.
    private var searchable: Bool {
        guard path.isEmpty else { return false }
        if case .pin = selection { return false }
        return true
    }

    private var topRow: some View {
        ZStack {
            if searchable {
                searchField
                    .frame(maxWidth: 560)
                    .padding(.horizontal, 72)
                    .transition(.opacity)
            }
            HStack {
                backButton
                Spacer()
            }
            .padding(.leading, Metrics.page - 4)
        }
        .frame(height: 36)
        // Centred on the traffic lights (26pt down in the window, whatever the zoom).
        .padding(.top, max(0, 26 / zoom - 18))
        .animation(.snappy(duration: 0.2), value: searchable)
        .animation(.snappy(duration: 0.2), value: path.isEmpty)
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField(searchPrompt, text: searchText)
                .textFieldStyle(.plain)
                .focused($searchFocused)
                .accessibilityIdentifier("search.field")
            if !searchText.wrappedValue.isEmpty {
                Button { searchText.wrappedValue = "" } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 34)
        .glassEffect(.regular.interactive(), in: .capsule)
        // Glass alone vanishes over a flat page; a hairline rim keeps the field findable.
        .overlay { Capsule().strokeBorder(.primary.opacity(0.12), lineWidth: 0.5) }
    }

    private var searchPrompt: String {
        if case let .section(s) = selection { return "Search \(s.title)" }
        return "Search your library"
    }

    /// In a library section the field filters it; elsewhere it searches everything,
    /// and typing on Home moves over to Search.
    private var searchText: Binding<String> {
        if case .section = selection { return $sectionFilter }
        return Binding(get: { search.query }, set: { q in
            search.query = q
            if selection == .home, !q.isEmpty { selection = .search }
        })
    }

    private func focusSearch() {
        if !searchable {
            path = NavigationPath()
            if case .pin = selection { selection = .search }
        }
        searchFocused = true
    }

    @ViewBuilder
    private var backButton: some View {
        if !path.isEmpty {
            Button { path.removeLast() } label: {
                Image(systemName: "chevron.left").font(.body.weight(.semibold))
                    .frame(width: 34, height: 34)
                    .glassEffect(.regular.interactive(), in: .circle)
                    .contentShape(.circle)
            }
            .buttonStyle(.pressable)
            .keyboardShortcut("[", modifiers: .command)
            .help("Back")
            .accessibilityLabel("Back")
            .accessibilityIdentifier("back")
            .transition(.opacity)
        }
    }

    // MARK: Sidebar

    private func sidebar(maxHeight: CGFloat) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                // Room for the window's traffic lights, which sit in the sidebar's corner.
                Color.clear.frame(height: 30)
                row(.home, "Home", "house").accessibilityIdentifier("sidebar.home")
                row(.search, "Search", "magnifyingglass").accessibilityIdentifier("sidebar.search")
                header("Library")
                ForEach(LibrarySection.ordered) { s in
                    row(.section(s), s.title, s.symbol).accessibilityIdentifier("sidebar.\(s.title)")
                }
                if !model.home.pins.isEmpty {
                    header("Pinned")
                    ForEach(model.home.pins) { p in
                        SidebarRow(selected: selection == .pin(p)) {
                            pick(.pin(p))
                        } label: {
                            ArtworkView(url: p.art, size: 20, circle: p.kind == .artist)
                            Text(p.title).lineLimit(1)
                        }
                        .accessibilityIdentifier("sidebar.pin.\(p.title)")
                        .itemMenu(p)
                    }
                }
            }
            .padding(8)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { sidebarHeight = $0 }
        }
        .scrollIndicators(.never)
        .scrollBounceBehavior(.basedOnSize)
        .frame(width: sidebarWidth, height: min(sidebarHeight, max(120, maxHeight)))
        // Concentric with the close button, whose centre macOS puts 18pt in from the
        // sidebar's edges (26pt from the window's, less the 8pt margin), as in Finder.
        .glassEffect(.regular, in: .rect(cornerRadius: 18 / zoom))
        .animation(.smooth, value: sidebarHeight)
    }

    private func header(_ text: String) -> some View {
        Text(text)
            .font(.caption.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 10)
            .padding(.top, 14)
            .padding(.bottom, 4)
    }

    private func row(_ item: SidebarItem, _ title: String, _ symbol: String) -> some View {
        SidebarRow(selected: selection == item) { pick(item) } label: {
            Image(systemName: symbol)
                .foregroundStyle(selection == item ? accent : .secondary)
                .frame(width: 20)
            Text(title)
        }
    }

    /// Clicking the page you're on goes back to its top level; a pinned song plays.
    private func pick(_ item: SidebarItem) {
        if case let .pin(p) = item, p.kind == .track {
            model.play(p)
            return
        }
        path = NavigationPath()
        if item != selection { sectionFilter = "" }
        selection = item
        if item == .search { searchFocused = true }
    }

    @ViewBuilder
    private var detail: some View {
        switch selection {
        case .home:
            HomeView(columns: 6)
        case .search:
            SearchResultsView(search: search)
        case let .section(s):
            SectionView(section: s, filter: sectionFilter).id(s)
        case let .pin(item):
            CollectionView(item: item).id(item.id)
        }
    }
}

/// One sidebar entry: a rounded highlight when selected, a fainter one on hover.
private struct SidebarRow<Label: View>: View {
    var selected: Bool
    var action: () -> Void
    @ViewBuilder var label: Label
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) { label }
                .font(.body)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background {
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(.primary.opacity(selected ? 0.12 : hover ? 0.06 : 0))
                }
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .animation(.snappy(duration: 0.18), value: hover)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// The floating capsule: song on the left, transport and scrubber in the
/// middle, extras on the right. The art sits far enough in that its rounded
/// corners clear the capsule's curve.
struct PlayerBar: View {
    @Environment(\.accent) private var accent
    @Environment(AppModel.self) private var model
    private let height: CGFloat = 64
    private let art: CGFloat = 44
    private let artRadius: CGFloat = 10
    /// Every control in the bar answers across this much, whatever its glyph.
    private let target: CGFloat = 32

    var body: some View {
        let t = model.player.track
        // Song and extras share what's left equally, so the transport sits dead center.
        HStack(spacing: 16) {
            song(t)
                .frame(maxWidth: .infinity, alignment: .leading)
            transport(t)
                .frame(width: 340)
            extras
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .padding(.leading, (height - art) / 2 + 4)
        .padding(.trailing, height / 2 - 8)
        .frame(height: height)
        .glassEffect(.regular, in: .capsule)
        // Glass alone vanishes over a flat page; a hairline rim and a soft shadow lift it.
        .overlay { Capsule().strokeBorder(.primary.opacity(0.1), lineWidth: 0.5) }
        .shadow(color: .black.opacity(0.25), radius: 18, y: 8)
        .animation(.smooth, value: t?.id)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("player.bar")
    }

    private func song(_ t: Track?) -> some View {
        HStack(spacing: 12) {
            Button {
                withAnimation(.player) { model.showNowPlaying = true }
            } label: {
                ExpandableArt(url: t?.art, size: art, radius: artRadius)
            }
            .buttonStyle(.pressable)
            .help("Now Playing")
            .accessibilityLabel("Now Playing")
            .accessibilityIdentifier("player.artwork")
            VStack(alignment: .leading, spacing: 2) {
                MarqueeText(text: t?.title ?? "Not Playing")
                    .font(.callout.weight(.semibold))
                    .accessibilityIdentifier("player.title")
                if let t, t.artists.count > 1 {
                    // Several artists: each name opens its own page.
                    ArtistLinks(track: t)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .tint(.secondary)
                        .lineLimit(1)
                        .accessibilityIdentifier("player.artist")
                } else {
                    Button { if let t { model.openArtist(of: t) } } label: {
                        MarqueeText(text: t?.artist ?? "")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.textLink)
                    .disabled(t?.artistId == nil)
                    .accessibilityIdentifier("player.artist")
                }
            }
            .id(t?.id)
            .transition(.blurReplace)
            if let t {
                Button { model.setLiked(t, !t.liked) } label: {
                    Image(systemName: t.liked ? "heart.fill" : "heart")
                        .foregroundStyle(t.liked ? accent : .secondary)
                        .contentTransition(.symbolEffect(.replace))
                        .frame(width: target, height: target)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.pressable)
                .accessibilityLabel(t.liked ? "Unlike" : "Like")
                .accessibilityIdentifier("player.like")
            }
        }
    }

    private func transport(_ t: Track?) -> some View {
        VStack(spacing: 2) {
            HStack(spacing: 10) {
                control("shuffle", tint: model.player.shuffle) { model.setShuffle(!model.player.shuffle) }
                    .accessibilityLabel("Shuffle")
                    .accessibilityValue(model.player.shuffle ? "on" : "off")
                    .accessibilityIdentifier("player.shuffle")
                control("backward.fill") { model.previous() }
                    .accessibilityLabel("Previous").accessibilityIdentifier("player.previous")
                Button { model.toggle() } label: {
                    Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                        .font(.title2)
                        .contentTransition(.symbolEffect(.replace))
                        .frame(width: target + 6, height: target)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel(model.player.status == .playing ? "Pause" : "Play")
                .accessibilityIdentifier("player.playPause")
                control("forward.fill") { model.next() }
                    .accessibilityLabel("Next").accessibilityIdentifier("player.next")
                control("repeat", tint: model.player.repeat == .one) { model.setRepeat(model.player.repeat != .one) }
                    .help("Repeat")
                    .accessibilityLabel("Repeat")
                    .accessibilityValue(model.player.repeat == .one ? "on" : "off")
                    .accessibilityIdentifier("player.repeat")
            }
            .buttonStyle(PressableStyle(scale: 0.8))
            .disabled(t == nil)
            BarScrubber()
        }
    }

    private func control(_ symbol: String, tint: Bool? = nil, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .foregroundStyle(tint == true ? AnyShapeStyle(accent) : tint == false ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                .frame(width: target, height: target)
                .contentShape(Rectangle())
        }
    }

    private var extras: some View {
        HStack(spacing: 4) {
            modeButton(.lyrics, "quote.bubble", "Lyrics")
            modeButton(.queue, "list.bullet", "Queue")
            // The route picker draws its glyph a size smaller than its frame.
            AirPlayButton()
                .frame(width: 22, height: 22)
                .frame(width: target, height: target)
            HStack(spacing: 6) {
                @Bindable var model = model
                Image(systemName: model.volume == 0 ? "speaker.slash.fill" : "speaker.wave.2.fill")
                    .foregroundStyle(.secondary)
                    .frame(width: 20)
                Slider(value: $model.volume, in: 0...1)
                    .frame(width: 90)
                    .controlSize(.small)
                    .accessibilityIdentifier("player.volume")
            }
            .padding(.leading, 6)
        }
        .fixedSize()
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
        } label: {
            Image(systemName: symbol)
                .foregroundStyle(on ? accent : .primary)
                .frame(width: target, height: target)
                .contentShape(Rectangle())
        }
        .buttonStyle(.pressable)
        .accessibilityLabel(label)
        .accessibilityIdentifier("player.\(label.lowercased())")
    }
}

/// The bar's cover: under the pointer it lifts and shows that a click opens it full screen.
private struct ExpandableArt: View {
    let url: String?
    let size: CGFloat
    let radius: CGFloat
    @State private var hover = false

    var body: some View {
        ArtworkView(url: url, size: size, cornerRadius: radius)
            .overlay {
                RoundedRectangle(cornerRadius: radius, style: .continuous)
                    .fill(.black.opacity(hover ? 0.35 : 0))
                Image(systemName: "arrow.up.left.and.arrow.down.right")
                    .font(.system(size: 14, weight: .bold))
                    .foregroundStyle(.white)
                    .opacity(hover ? 1 : 0)
                    .scaleEffect(hover ? 1 : 0.6)
            }
            .scaleEffect(hover ? 1.08 : 1)
            .shadow(color: .black.opacity(hover ? 0.3 : 0), radius: 6, y: 3)
            .onHover { h in withAnimation(.spring(response: 0.28, dampingFraction: 0.7)) { hover = h } }
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
                Button("Open Stream") {
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
    @Environment(\.accent) private var accent
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
                                .background(RoundedRectangle(cornerRadius: 8).fill(i == selected ? accent.opacity(0.25) : .clear))
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
