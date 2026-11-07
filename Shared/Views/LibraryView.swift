import StreamBindings
import SwiftUI

extension LibrarySection: Identifiable {
    public var id: String { title }

    var title: String {
        switch self {
        case .playlists: "Playlists"
        case .artists: "Artists"
        case .albums: "Albums"
        case .uploads: "Uploads"
        case .uploadedAlbums: "Uploaded Albums"
        case .uploadedArtists: "Uploaded Artists"
        case .downloaded: "Downloaded"
        case .podcasts: "Podcasts"
        }
    }

    var symbol: String {
        switch self {
        case .playlists: "music.note.list"
        case .artists: "music.mic"
        case .albums: "square.stack"
        case .uploads: "arrow.up.circle"
        case .uploadedAlbums: "square.stack"
        case .uploadedArtists: "music.mic"
        case .downloaded: "arrow.down.circle"
        case .podcasts: Item.showSymbol
        }
    }

    var isSongs: Bool { self == .uploads || self == .downloaded }

    /// Shown as a grid of covers rather than a list.
    var isGrid: Bool { !isSongs }

    /// Uploads' own pills: its songs, the albums they make up and their artists.
    var parts: [(String, LibrarySection)] {
        self == .uploads ? [("Songs", .uploads), ("Albums", .uploadedAlbums), ("Artists", .uploadedArtists)] : []
    }

    static let ordered: [LibrarySection] = [.playlists, .podcasts, .artists, .albums, .uploads, .downloaded]
}

/// A Library page that isn't one of the core's sections.
enum LibraryPage: Hashable { case history }

/// The Library tab, laid out like Apple Music's: a glyph column, the name in
/// title type, hairlines that start at the text, nothing above the first row.
struct LibraryView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        List {
            Group {
                ForEach(LibrarySection.ordered) { s in
                    NavigationLink(value: s) { Self.row(s.title, s.symbol, progress: s == .downloaded ? downloading : nil) }
                        .accessibilityIdentifier("library.\(s.title)")
                }
                NavigationLink(value: LibraryPage.history) { Self.row("History", "clock") }
                    .accessibilityIdentifier("library.History")
            }
            .listRowInsets(EdgeInsets(top: 0, leading: Metrics.page, bottom: 0, trailing: Metrics.page))
            .listSectionSeparator(.hidden, edges: .top)
        }
        .listStyle(.plain)
    }

    /// How far the download queue is, while there is one.
    private var downloading: Double? {
        let d = model.downloads
        return d.total > 0 ? (Double(d.done) + Double(d.progress)) / Double(d.total) : nil
    }

    private static func row(_ title: String, _ symbol: String, progress: Double? = nil) -> some View {
        HStack(spacing: 0) {
            Image(systemName: symbol)
                .font(.system(size: 22))
                .foregroundStyle(.primary)
                .frame(width: 30)
            Text(title)
                .font(.title3)
                .padding(.leading, 11)
                .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
            if let progress {
                Spacer(minLength: 8)
                ProgressRing(value: progress, lineWidth: 2.2)
                    .frame(width: 18, height: 18)
                    .accessibilityLabel("Downloading")
            }
        }
        .frame(minHeight: 52)
    }
}

/// One library section, filtered by the search field (the Mac window's, or the page's own on
/// iPhone). Covers are paged from the core as they scroll; songs come whole, to sort.
struct SectionView: View {
    let section: LibrarySection
    /// The Mac window's search field, when it is searching this section.
    var filter = ""
    @Environment(AppModel.self) private var model
    /// Uploads' pill picked (its songs, albums or artists); the section itself elsewhere.
    @State private var part: LibrarySection
    @State private var items: [Item] = []
    @State private var exhausted = false
    @State private var loadedVersion: Int?
    /// Every item in the section, loaded the first time someone searches it.
    @State private var everything: [Item]?
    /// Song sections: all their songs.
    @State private var songs: [Track]?
    @State private var typed = ""
    @AppStorage private var sort: TrackSort
    @AppStorage private var reversed: Bool
    #if os(macOS)
    @State private var selection = SongSelection()
    #endif
    private let pageSize: UInt32 = 120

    init(section: LibrarySection, filter: String = "") {
        self.section = section
        self.filter = filter
        _part = State(initialValue: section)
        _sort = AppStorage(wrappedValue: .playlist, "\(section.title)Sort")
        _reversed = AppStorage(wrappedValue: false, "\(section.title)SortReversed")
    }

    private var query: String {
        (filter.isEmpty ? typed : filter).trimmingCharacters(in: .whitespaces)
    }

    private var shown: [Item] {
        guard !query.isEmpty else { return items }
        return (everything ?? items).filter {
            $0.title.localizedStandardContains(query) || $0.subtitle.localizedStandardContains(query)
        }
    }

    /// The songs as listed: searched, then sorted.
    private var shownSongs: [Track] {
        var all = songs ?? []
        if !query.isEmpty {
            all = all.filter {
                $0.title.localizedStandardContains(query) || $0.artist.localizedStandardContains(query) || $0.album.localizedStandardContains(query)
            }
        }
        return sort.apply(all, reversed: reversed, track: { $0 })
    }

    private var isEmpty: Bool { part.isSongs ? (songs?.isEmpty == true || (!query.isEmpty && shownSongs.isEmpty)) : shown.isEmpty && (exhausted || !query.isEmpty) }

    var body: some View {
        Group {
            if part.isSongs { list } else { grid }
        }
        .navigationTitle(section.title)
        #if os(iOS)
        .toolbar {
            if section == .playlists {
                ToolbarItem(placement: .primaryAction) { NewPlaylistMenu() }
            }
            if part.isSongs {
                ToolbarItem(placement: .primaryAction) { SortMenu(sort: $sort, reversed: $reversed) }
            }
        }
        // Always showing, not tucked away until you pull the list down.
        .searchable(text: $typed, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search \(section.title)")
        #endif
        .overlay {
            if isEmpty && !(section == .downloaded && model.downloads.total > 0) {
                if query.isEmpty {
                    ContentUnavailableView(emptyText, systemImage: section.symbol)
                } else {
                    ContentUnavailableView.search(text: query)
                }
            }
        }
        .onAppear { if loadedVersion != model.libraryVersion { reload() } }
        .onChange(of: model.libraryVersion) { reload() }
        .onChange(of: part) { reload() }
        .onChange(of: query) { _, q in
            if !q.isEmpty, everything == nil, !part.isSongs { loadEverything() }
        }
        #if os(iOS)
        .refreshable { await model.refreshSpotify() }
        #endif
    }

    /// Uploads' pills: Songs, Albums, Artists.
    @ViewBuilder
    private var pills: some View {
        if !section.parts.isEmpty {
            HStack(spacing: 8) {
                ForEach(section.parts, id: \.1) { title, p in
                    ReleasePill(title: title, selected: part == p) {
                        withAnimation(.snappy) { part = p }
                    }
                }
            }
        }
    }

    #if os(macOS)
    /// The Mac draws song lists itself: AppKit's table ignores the window's insets,
    /// and these rows bring their own hover highlight.
    private var list: some View {
        ScrollView {
            let rows = shownSongs
            let order = Array(rows.indices)
            LazyVStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline) {
                    PageTitle(text: section.title)
                    Spacer()
                    SortMenu(sort: $sort, reversed: $reversed)
                }
                .padding(.horizontal, 10)
                pills.padding(.horizontal, 10).padding(.bottom, 8)
                if section == .downloaded { DownloadQueueView().padding(.horizontal, 10).padding(.bottom, 8) }
                ForEach(Array(rows.enumerated()), id: \.element.id) { i, t in
                    ItemRow(item: t.item, showAlbum: true)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .pickable(i, in: selection, order: order) { play(rows, at: i) }
                        .accessibilityIdentifier("row.\(t.title)")
                        .itemMenu(t.item, tracks: {
                            guard selection.keys.contains(i), selection.keys.count > 1 else { return [t] }
                            return order.filter(selection.keys.contains).compactMap { rows.indices.contains($0) ? rows[$0] : nil }
                        })
                }
            }
            .songClipboard(selection, order: order, tracks: { keys in keys.compactMap { rows.indices.contains($0) ? rows[$0] : nil } })
            .padding(.leading, Metrics.page - 10)
            .padding(.trailing, Metrics.pageTrailing - 10)
            .padding(.bottom, 120)
        }
    }
    #else
    private var list: some View {
        List {
            if !section.parts.isEmpty {
                pills.listRowSeparator(.hidden)
            }
            if section == .downloaded {
                DownloadQueueView().listRowSeparator(.hidden)
            }
            let rows = shownSongs
            ForEach(Array(rows.enumerated()), id: \.element.id) { i, t in
                Button { play(rows, at: i) } label: { ItemRow(item: t.item) }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("row.\(t.title)")
                    .itemMenu(t.item)
                    .listRowSeparator(.hidden)
            }
        }
        .listStyle(.plain)
    }
    #endif

    /// Albums, artists and playlists: covers (artists in circles) with their names below.
    private var grid: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline) {
                    PageTitle(text: section.title)
                    #if os(macOS)
                    if section == .playlists {
                        NewPlaylistMenu()
                            .labelStyle(.iconOnly)
                            .menuIndicator(.hidden)
                            .font(.title3.weight(.semibold))
                            .fixedSize()
                    }
                    #endif
                }
                pills.padding(.bottom, 4)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: Self.tileMinimum, maximum: 240), spacing: 16, alignment: .top)], spacing: 22) {
                    ForEach(Array(shown.enumerated()), id: \.element.id) { i, item in
                        NavigationLink(value: item) { GridTile(item: item) }
                            .buttonStyle(.pressable)
                            .itemMenu(item)
                            .accessibilityIdentifier("row.\(item.title)")
                            .onAppear { if query.isEmpty, i == items.count - 20 { loadMore() } }
                    }
                }
                .padding(.top, 4)
            }
            .padding(.horizontal, Metrics.page)
            .padding(.bottom, 120)
        }
    }

    #if os(macOS)
    private static let tileMinimum: CGFloat = 160
    #else
    private static let tileMinimum: CGFloat = 150
    #endif

    private var emptyText: String {
        switch section {
        case .downloaded: "Nothing downloaded"
        case .uploads: "No uploads yet"
        case .albums: "No saved albums"
        case .artists: "No followed artists"
        default: "No \(section.title.lowercased())"
        }
    }

    private func reload() {
        loadedVersion = model.libraryVersion
        if part.isSongs {
            let core = model.core, part = part
            Task {
                let all = await Task.detached(priority: .userInitiated) { core.sectionTracks(section: part) }.value
                songs = all
            }
            return
        }
        items = model.sectionFirstPage(part, limit: pageSize)
        exhausted = items.count < Int(pageSize)
        everything = nil
        if !query.isEmpty { loadEverything() }
    }

    /// The whole section, read off the main thread (a big library takes a moment).
    private func loadEverything() {
        let core = model.core, part = part
        Task {
            let all = await Task.detached(priority: .userInitiated) {
                core.section(section: part, offset: 0, limit: 1_000_000)
            }.value
            everything = all
        }
    }

    private func loadMore() {
        guard !exhausted else { return }
        let more = model.core.section(section: part, offset: UInt32(items.count), limit: pageSize)
        exhausted = more.count < Int(pageSize)
        items.append(contentsOf: more)
    }

    /// Plays the songs as listed (searched and sorted) from this one on.
    private func play(_ rows: [Track], at i: Int) {
        model.play(rows, at: i)
    }
}

/// A cover filling its grid cell, with the name below (artists: a centered
/// name under a circle; albums: title and artist).
struct GridTile: View {
    let item: Item

    var body: some View {
        let artist = item.kind == .artist
        VStack(alignment: artist ? .center : .leading, spacing: 8) {
            GeometryReader { geo in
                ArtworkView(url: item.art, size: geo.size.width, circle: artist)
            }
            .aspectRatio(1, contentMode: .fit)
            VStack(alignment: artist ? .center : .leading, spacing: 1) {
                Text(item.title)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.primary)
                if !artist {
                    Text(item.subtitle).font(.footnote).foregroundStyle(.secondary)
                }
            }
            .lineLimit(1)
            .multilineTextAlignment(artist ? .center : .leading)
            .frame(maxWidth: .infinity, alignment: artist ? .center : .leading)
        }
        .contentShape(Rectangle())
    }
}
