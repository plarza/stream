import StreamBindings
import AzaUI
import SwiftUI

/// Album, playlist and artist pages: compact header (art + details side by
/// side) on a page tinted from the artwork, an actions row, then the songs.
struct CollectionView: View {
    @Environment(\.accent) private var accent
    let item: Item
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var scheme
    @State private var page: CollectionPage?
    @State private var palette: Palette = .fallback
    @State private var error: String?
    @State private var pinned = false
    @State private var liked = false
    @State private var showCover = false
    /// Artist pages: the release kind picked, or nil for all of them.
    @State private var releaseFilter: ReleaseKind?
    @State private var bioExpanded = false
    /// Playlist pages: how the songs are sorted, the same for every playlist.
    @AppStorage("playlistSort") private var sort: TrackSort = .playlist
    @AppStorage("playlistSortReversed") private var reversed = false
    @Namespace private var coverZoom

    init(item: Item) {
        self.item = item
        // Open with what we had last time or what's on disk (page and colors), then refresh.
        let cached = AppModel.shared.cachedPage(item.id)
        _page = State(initialValue: cached)
        _liked = State(initialValue: cached?.liked ?? false)
        _pinned = State(initialValue: cached?.pinned ?? false)
        _palette = State(initialValue: Palette.cached(cached?.item.art ?? item.art) ?? .fallback)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.horizontal, Metrics.page)
                    .padding(.top, 12)
                actions
                    .padding(.horizontal, Metrics.page)
                    .padding(.vertical, 16)
                if let page {
                    tracks(page)
                    if !page.releases.isEmpty { releases(page) }
                    if let info = page.artist { about(info) }
                    if let d = page.description, item.kind != .artist {
                        Text(d).font(.footnote).foregroundStyle(.secondary).padding(Metrics.page)
                    }
                } else if let error {
                    ContentUnavailableView("Couldn’t load", systemImage: "exclamationmark.triangle", description: Text(error))
                        .padding(.top, 40)
                } else {
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 60)
                }
            }
            .padding(.bottom, 120)
        }
        .background(alignment: .top) {
            ZStack(alignment: .top) {
                // The art's colors are dark; in light mode they only wash the top.
                LinearGradient(colors: [palette.top, palette.top.opacity(0.5), .clear], startPoint: .top, endPoint: .bottom)
                    .opacity(scheme == .dark ? 1 : 0.35)
                    .frame(height: 520)
                    .animation(.smooth(duration: 0.6), value: palette)
                // An artist's own header photo, as on their Spotify page, fading into the page.
                if let header = page?.artist?.header {
                    FillArtwork(url: header, pixels: 1400)
                        .frame(height: 340)
                        .frame(maxWidth: .infinity)
                        .clipped()
                        .mask(LinearGradient(colors: [.black, .black.opacity(0.6), .clear], startPoint: .top, endPoint: .bottom))
                        .opacity(scheme == .dark ? 0.85 : 0.6)
                        .transition(.opacity)
                        .accessibilityHidden(true)
                }
            }
            .ignoresSafeArea()
        }
        .navigationTitle(page?.item.title ?? item.title)
        .toolbar {
            if item.kind == .playlist {
                ToolbarItem(placement: .primaryAction) { sortMenu }
            }
        }
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(.hidden, for: .navigationBar)
        #endif
        #if os(iOS)
        .fullScreenCover(isPresented: $showCover) {
            CoverViewer(url: shown.art, palette: palette, circle: item.kind == .artist) { showCover = false }
                .navigationTransition(.zoom(sourceID: "cover", in: coverZoom))
        }
        #else
        .overlay {
            if showCover {
                CoverViewer(url: shown.art, palette: palette, circle: item.kind == .artist) {
                    withAnimation(.player) { showCover = false }
                }
                .transition(.opacity.combined(with: .scale(scale: 0.92)))
            }
        }
        .animation(.player, value: showCover)
        #endif
        .onAppear { model.currentPage = item.id }
        .onDisappear { if model.currentPage == item.id { model.currentPage = nil } }
        .task(id: item.id) { await load() }
        .task(id: page?.item.art ?? item.art) {
            let p = await Palette.extract(from: page?.item.art ?? item.art)
            withAnimation(.smooth(duration: 0.6)) { palette = p }
        }
    }

    private var shown: Item { page?.item ?? item }

    private var header: some View {
        HStack(alignment: .bottom, spacing: 16) {
            Button { showCover = true } label: {
                ArtworkView(url: shown.art, size: 132, circle: item.kind == .artist)
            }
            .buttonStyle(.pressable)
            #if os(iOS)
            .matchedTransitionSource(id: "cover", in: coverZoom)
            #endif
            .shadow(color: .black.opacity(0.35), radius: 16, y: 8)
            .accessibilityLabel("Show Cover")
            .accessibilityIdentifier("collection.cover")
            VStack(alignment: .leading, spacing: 4) {
                Text(shown.title).font(.title2.weight(.bold)).lineLimit(3)
                if item.kind != .artist {
                    if item.kind == .album, let lead = page?.tracks.first, lead.artistId != nil {
                        Button { model.openArtist(of: lead) } label: {
                            Text(shown.subtitle).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                        }
                        .buttonStyle(.textLink)
                        .accessibilityIdentifier("collection.artist")
                    } else {
                        Text(shown.subtitle).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                if let page {
                    Text(page.detail).font(.caption).foregroundStyle(.tertiary)
                    if item.kind == .artist, let d = page.description {
                        Text(d).font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                    }
                    if item.kind != .artist, let q = commonQuality(page.tracks) {
                        HStack(spacing: 4) {
                            if page.tracks.allSatisfy(\.downloaded) { DownloadedBadge() }
                            QualityChip(text: q)
                        }
                        .padding(.top, 4)
                    }
                }
            }
            Spacer(minLength: 0)
        }
    }

    private var actions: some View {
        HStack(spacing: 18) {
            Button {
                if let page { model.play(ordered(page), from: page.item) }
            } label: {
                Image(systemName: "play.fill")
                    .font(.title3)
                    .frame(width: 52, height: 52)
                    .foregroundStyle(model.onTint)
                    .background(accent, in: .circle)
            }
            .buttonStyle(.pressable)
            .accessibilityLabel("Play")
            .accessibilityIdentifier("collection.play")
            Button {
                if let page { model.play(ordered(page), from: page.item, shuffle: true) }
            } label: { Image(systemName: "shuffle").font(.title3) }
                .buttonStyle(.pressable)
                .accessibilityLabel("Shuffle")
                .accessibilityIdentifier("collection.shuffle")
            if item.kind == .album, item.source == .spotify {
                Button { toggleSaved() } label: {
                    Image(systemName: liked ? "heart.fill" : "heart").font(.title3)
                        .contentTransition(.symbolEffect(.replace))
                }
                .buttonStyle(.pressable)
                .accessibilityLabel(liked ? "Remove from Library" : "Add to Library")
                .accessibilityIdentifier("collection.like")
            }
            Button {
                model.togglePin(shown)
                withAnimation(.snappy) { pinned.toggle() }
            } label: {
                Image(systemName: pinned ? "pin.fill" : "pin").font(.title3)
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.pressable)
            .accessibilityLabel(pinned ? "Unpin" : "Pin")
            .accessibilityIdentifier("collection.pin")
            #if os(iOS)
            if shown.source == .spotify, item.kind != .artist {
                Button {
                    Task {
                        model.show("Downloading…")
                        do { try await model.core.download(collectionId: item.id) ; model.show("Downloaded") } catch { model.show(error.localizedDescription) }
                    }
                } label: { Image(systemName: "arrow.down.circle").font(.title3) }
                    .buttonStyle(.pressable)
            }
            #endif
            Spacer()
        }
        .foregroundStyle(.primary)
    }

    @ViewBuilder
    private func tracks(_ page: CollectionPage) -> some View {
        if item.kind == .artist, !page.tracks.isEmpty {
            Text("Popular").font(.title3.weight(.bold)).padding(.horizontal, Metrics.page).padding(.bottom, 4)
        }
        LazyVStack(spacing: 0) {
            let tracks = ordered(page)
            ForEach(Array(tracks.prefix(item.kind == .artist ? 10 : .max).enumerated()), id: \.element.id) { i, t in
                Button { model.play(tracks, at: i, from: page.item) } label: {
                    TrackRow(track: t, showArt: item.kind != .album, number: item.kind == .album ? Int(t.trackNumber == 0 ? UInt32(i + 1) : t.trackNumber) : nil,
                             isCurrent: model.player.track?.id == t.id,
                             showArtist: item.kind != .album || t.artist != page.item.subtitle)
                        .padding(.leading, Metrics.page)
                        .padding(.trailing, Metrics.pageTrailing)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("track.\(t.title)")
                .trackMenu(t)
            }
        }
    }

    private var sortMenu: some View {
        Menu {
            Picker("Sort By", selection: $sort.animation(.smooth)) {
                ForEach(TrackSort.allCases, id: \.self) { Text($0.title).tag($0) }
            }
            Divider()
            Toggle("Reverse", isOn: $reversed.animation(.smooth))
        } label: {
            Label("Sort", systemImage: "arrow.up.arrow.down")
        }
        .accessibilityIdentifier("playlist.sort")
    }

    /// The page's songs, sorted as picked when it's a playlist.
    private func ordered(_ page: CollectionPage) -> [Track] {
        guard item.kind == .playlist else { return page.tracks }
        let less: (Track, Track) -> Bool = switch sort {
        case .playlist: { _, _ in false }
        case .title: { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
        case .artist: { ($0.artist, $0.album, $0.discNumber, $0.trackNumber) < ($1.artist, $1.album, $1.discNumber, $1.trackNumber) }
        case .album: { ($0.album, $0.discNumber, $0.trackNumber) < ($1.album, $1.discNumber, $1.trackNumber) }
        case .length: { $0.durationMs < $1.durationMs }
        }
        let sorted = sort == .playlist ? page.tracks : page.tracks.sorted(by: less)
        return reversed ? sorted.reversed() : sorted
    }

    /// Every release, newest first, with pills for each kind there is. No pill picked shows them all.
    private func releases(_ page: CollectionPage) -> some View {
        let kinds = Self.pillOrder.filter { k in page.releases.contains { Self.pill(for: $0.kind) == k } }
        let shown = page.releases.filter { releaseFilter == nil || Self.pill(for: $0.kind) == releaseFilter }
        return VStack(alignment: .leading, spacing: 12) {
            Text(page.artist == nil ? "Albums" : "Discography").font(.title3.weight(.bold)).padding(.horizontal, Metrics.page)
            if kinds.count > 1 {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(kinds, id: \.self) { k in
                            ReleasePill(title: Self.title(k), selected: releaseFilter == k) {
                                withAnimation(.snappy) { releaseFilter = releaseFilter == k ? nil : k }
                            }
                        }
                    }
                    .padding(.horizontal, Metrics.page)
                }
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 150, maximum: 220), spacing: 14, alignment: .top)], alignment: .leading, spacing: 18) {
                ForEach(shown, id: \.item.id) { r in
                    NavigationLink(value: r.item) { ReleaseTile(item: r.item) }
                        .buttonStyle(.pressable)
                        .itemMenu(r.item)
                        .accessibilityIdentifier("release.\(r.item.title)")
                }
            }
            .padding(.horizontal, Metrics.page)
        }
        .padding(.top, 24)
    }

    /// Compilations sit with albums: the pills are Albums, Singles and EPs.
    private static let pillOrder: [ReleaseKind] = [.album, .single, .ep]
    private static func pill(for kind: ReleaseKind) -> ReleaseKind { kind == .compilation ? .album : kind }
    private static func title(_ kind: ReleaseKind) -> String {
        switch kind {
        case .album, .compilation: "Albums"
        case .single: "Singles"
        case .ep: "EPs"
        }
    }

    /// The bio and Spotify's numbers for the artist.
    private func about(_ info: ArtistInfo) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("About").font(.title3.weight(.bold))
            if let bio = info.biography {
                Text(bio)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(bioExpanded ? nil : 4)
                    .onTapGesture { withAnimation(.smooth) { bioExpanded.toggle() } }
                    .accessibilityIdentifier("artist.bio")
                if !bioExpanded {
                    Button("More") { withAnimation(.smooth) { bioExpanded = true } }
                        .font(.callout.weight(.semibold))
                        .buttonStyle(.plain)
                }
            }
            HStack(alignment: .top, spacing: 28) {
                if info.monthlyListeners > 0 { stat(info.monthlyListeners.formatted(), "Monthly listeners") }
                if info.followers > 0 { stat(info.followers.formatted(), "Followers") }
                if let rank = info.worldRank { stat("#\(rank)", "In the world") }
            }
            .padding(.top, 4)
            if !info.topCities.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Top cities").font(.subheadline.weight(.semibold))
                    ForEach(Array(info.topCities.prefix(5).enumerated()), id: \.offset) { _, c in
                        HStack {
                            Text(c.country.isEmpty ? c.city : "\(c.city), \(c.country)")
                            Spacer()
                            Text(c.listeners.formatted()).monospacedDigit().foregroundStyle(.secondary)
                        }
                        .font(.callout)
                    }
                }
                .padding(.top, 4)
            }
        }
        .padding(.horizontal, Metrics.page)
        .padding(.top, 32)
    }

    private func stat(_ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).font(.headline.monospacedDigit())
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
    }

    private func load() async {
        pinned = model.isPinned(item.id)
        do {
            let p = try await model.core.collection(id: item.id)
            model.pages[item.id] = p
            withAnimation(.smooth) {
                page = p
                liked = p.liked
                pinned = p.pinned
            }
        } catch {
            if page == nil { self.error = AppModel.message(error) }
        }
    }

    private func toggleSaved() {
        liked.toggle()
        Haptics.success()
        let id = item.id, on = liked
        Task {
            do { try await model.core.setLiked(trackId: id, liked: on) } catch {
                withAnimation(.snappy) { liked = !on }
                model.show(AppModel.message(error))
            }
        }
    }

    private func commonQuality(_ tracks: [Track]) -> String? {
        guard let first = tracks.first?.quality, !first.isEmpty else { return nil }
        return tracks.allSatisfy { $0.quality == first } ? first : nil
    }
}

/// How a playlist's songs are sorted.
enum TrackSort: String, CaseIterable {
    case playlist, title, artist, album, length

    var title: String {
        switch self {
        case .playlist: "Playlist Order"
        case .title: "Title"
        case .artist: "Artist"
        case .album: "Album"
        case .length: "Length"
        }
    }
}

/// One release in an artist's discography grid: its cover filling the column.
private struct ReleaseTile: View {
    let item: Item

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            FillArtwork(url: item.art, pixels: 640)
                .aspectRatio(1, contentMode: .fit)
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            VStack(alignment: .leading, spacing: 1) {
                Text(item.title).font(.footnote.weight(.medium)).lineLimit(1)
                Text(item.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .foregroundStyle(.primary)
    }
}

/// A filter pill: filled when picked.
struct ReleasePill: View {
    let title: String
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.subheadline.weight(selected ? .semibold : .regular))
                .padding(.horizontal, 14)
                .padding(.vertical, 7)
                .foregroundStyle(selected ? AnyShapeStyle(.background) : AnyShapeStyle(.primary))
                .background {
                    if selected { Capsule().fill(.primary) } else { Capsule().fill(.quaternary) }
                }
        }
        .buttonStyle(.pressable)
        .accessibilityIdentifier("pill.\(title)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

/// A cover filling the screen over its own blurred colors. Tap or swipe down to close.
struct CoverViewer: View {
    let url: String?
    let palette: Palette
    var circle = false
    let close: () -> Void

    var body: some View {
        ZStack {
            ImmersiveBackground(url: url, palette: palette)
            FillArtwork(url: url)
                .aspectRatio(1, contentMode: .fit)
                .clipShape(circle ? AnyShape(Circle()) : AnyShape(RoundedRectangle(cornerRadius: 14, style: .continuous)))
                .shadow(color: .black.opacity(0.45), radius: 30, y: 16)
                .padding(Metrics.page)
                .frame(maxWidth: 900, maxHeight: 900)
                .accessibilityHidden(true)
        }
        .overlay(alignment: .topTrailing) {
            Button(action: close) {
                Image(systemName: "xmark")
                    .font(.system(size: 16, weight: .semibold))
                    .frame(width: 44, height: 44)
                    .glassEffect(.regular.interactive(), in: .circle)
            }
            .buttonStyle(.pressable)
            .padding(.trailing, Metrics.page)
            .padding(.top, 8)
            .accessibilityLabel("Close")
            .accessibilityIdentifier("cover.close")
        }
        .contentShape(Rectangle())
        .onTapGesture(perform: close)
        .environment(\.colorScheme, .dark)
        .foregroundStyle(.white)
    }
}
