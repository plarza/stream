import SwiftUI

extension LibrarySection: Identifiable {
    public var id: String { title }

    var title: String {
        switch self {
        case .playlists: "Playlists"
        case .artists: "Artists"
        case .albums: "Albums"
        case .songs: "Songs"
        case .downloaded: "Downloaded"
        case .likedSongs: "Liked Songs"
        }
    }

    var symbol: String {
        switch self {
        case .playlists: "music.note.list"
        case .artists: "music.mic"
        case .albums: "square.stack"
        case .songs: "music.note"
        case .downloaded: "arrow.down.circle"
        case .likedSongs: "heart"
        }
    }

    var isSongs: Bool { self == .songs || self == .downloaded || self == .likedSongs }

    /// Shown as a grid of covers rather than a list.
    var isGrid: Bool { self == .albums || self == .artists || self == .playlists }

    static let ordered: [LibrarySection] = [.playlists, .artists, .albums, .songs, .likedSongs, .downloaded]
}

/// The Library tab, laid out like Apple Music's: a glyph column, the name in
/// title type, hairlines that start at the text, nothing above the first row.
struct LibraryView: View {
    var body: some View {
        List {
            ForEach(LibrarySection.ordered) { s in
                NavigationLink(value: s) {
                    HStack(spacing: 0) {
                        Image(systemName: s.symbol)
                            .font(.system(size: 22))
                            .foregroundStyle(.primary)
                            .frame(width: 30)
                        Text(s.title)
                            .font(.title3)
                            .padding(.leading, 11)
                            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                    }
                    .frame(minHeight: 52)
                }
                .listRowInsets(EdgeInsets(top: 0, leading: Metrics.page, bottom: 0, trailing: Metrics.page))
                .accessibilityIdentifier("library.\(s.title)")
            }
            .listSectionSeparator(.hidden, edges: .top)
        }
        .listStyle(.plain)
    }
}

/// One library section, paged from the core as it scrolls, and filtered by
/// the search field (the Mac window's, or the page's own on iPhone).
struct SectionView: View {
    let section: LibrarySection
    /// The Mac window's search field, when it is searching this section.
    var filter = ""
    @Environment(AppModel.self) private var model
    @State private var items: [Item] = []
    @State private var exhausted = false
    @State private var loadedVersion: Int?
    /// Every item in the section, loaded the first time someone searches it.
    @State private var everything: [Item]?
    @State private var typed = ""
    private let pageSize: UInt32 = 120

    init(section: LibrarySection, filter: String = "") {
        self.section = section
        self.filter = filter
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

    var body: some View {
        Group {
            if section.isGrid { grid } else { list }
        }
        .navigationTitle(section.title)
        #if os(iOS)
        .searchable(text: $typed, prompt: "Search \(section.title)")
        #endif
        .overlay {
            if shown.isEmpty && (exhausted || !query.isEmpty) {
                if query.isEmpty {
                    ContentUnavailableView(emptyText, systemImage: section.symbol)
                } else {
                    ContentUnavailableView.search(text: query)
                }
            }
        }
        .onAppear { if loadedVersion != model.libraryVersion { reload() } }
        .onChange(of: model.libraryVersion) { reload() }
        .onChange(of: query) { _, q in
            if !q.isEmpty, everything == nil { loadEverything() }
        }
        #if os(iOS)
        .refreshable { await model.refreshSpotify() }
        #endif
    }

    #if os(macOS)
    /// The Mac draws song lists itself: AppKit's table ignores the window's insets,
    /// and these rows bring their own hover highlight.
    private var list: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2) {
                PageTitle(text: section.title).padding(.horizontal, 10)
                ForEach(Array(shown.enumerated()), id: \.element.id) { i, item in
                    row(item)
                        .onAppear { if query.isEmpty, i == items.count - 20 { loadMore() } }
                }
            }
            .padding(.leading, Metrics.page - 10)
            .padding(.trailing, Metrics.pageTrailing - 10)
            .padding(.bottom, 120)
        }
    }
    #else
    private var list: some View {
        List {
            ForEach(Array(shown.enumerated()), id: \.element.id) { i, item in
                row(item)
                    .listRowSeparator(.hidden)
                    .onAppear { if query.isEmpty, i == items.count - 20 { loadMore() } }
            }
        }
        .listStyle(.plain)
    }
    #endif

    /// Albums, artists and playlists: covers (artists in circles) with their names below.
    private var grid: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                PageTitle(text: section.title)
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

    @ViewBuilder
    private func row(_ item: Item) -> some View {
        if item.kind == .track {
            #if os(macOS)
            Button { play(item) } label: { ItemRow(item: item, showAlbum: true) }
                .buttonStyle(HoverRowStyle())
                .accessibilityIdentifier("row.\(item.title)")
                .itemMenu(item)
            #else
            Button { play(item) } label: { ItemRow(item: item, showSubtitle: section != .artists) }
                .buttonStyle(.plain)
                .accessibilityIdentifier("row.\(item.title)")
                .itemMenu(item)
            #endif
        } else {
            NavigationLink(value: item) { ItemRow(item: item, showSubtitle: section != .artists) }
                .accessibilityIdentifier("row.\(item.title)")
                .itemMenu(item)
        }
    }

    private var emptyText: String {
        switch section {
        case .downloaded: "Nothing downloaded"
        case .likedSongs: "No liked songs"
        default: "No \(section.title.lowercased())"
        }
    }

    private func reload() {
        items = model.sectionFirstPage(section, limit: pageSize)
        exhausted = items.count < Int(pageSize)
        everything = nil
        if !query.isEmpty { loadEverything() }
        loadedVersion = model.libraryVersion
    }

    /// The whole section, read off the main thread (a big library takes a moment).
    private func loadEverything() {
        let core = model.core, section = section
        Task {
            let all = await Task.detached(priority: .userInitiated) {
                core.section(section: section, offset: 0, limit: 1_000_000)
            }.value
            everything = all
        }
    }

    private func loadMore() {
        guard !exhausted else { return }
        let more = model.core.section(section: section, offset: UInt32(items.count), limit: pageSize)
        exhausted = more.count < Int(pageSize)
        items.append(contentsOf: more)
    }

    /// Plays the section (or what the search left of it) from this song on.
    private func play(_ item: Item) {
        let core = model.core, section = section
        let ids = query.isEmpty ? nil : Set(shown.map(\.id))
        Task {
            var tracks = await Task.detached(priority: .userInitiated) { core.sectionTracks(section: section) }.value
            if let ids { tracks = tracks.filter { ids.contains($0.id) } }
            model.play(tracks, at: tracks.firstIndex { $0.id == item.id } ?? 0)
        }
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
