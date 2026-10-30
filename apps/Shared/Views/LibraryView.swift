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

    static let ordered: [LibrarySection] = [.playlists, .artists, .albums, .songs, .likedSongs, .downloaded]
}

struct LibraryView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        List {
            ForEach(LibrarySection.ordered) { s in
                NavigationLink(value: s) {
                    Label(s.title, systemImage: s.symbol)
                        .font(.title3)
                        .padding(.vertical, 4)
                }
            }
        }
        .listStyle(.plain)
        .navigationTitle("Library")
    }
}

/// One library section, paged from the core as it scrolls.
struct SectionView: View {
    let section: LibrarySection
    @Environment(AppModel.self) private var model
    @State private var items: [Item] = []
    @State private var exhausted = false
    private let pageSize: UInt32 = 120

    var body: some View {
        List {
            if section.isSongs, !items.isEmpty {
                HStack(spacing: 12) {
                    Button { playAll(shuffle: false) } label: {
                        Label("Play", systemImage: "play.fill").frame(maxWidth: .infinity)
                    }
                    Button { playAll(shuffle: true) } label: {
                        Label("Shuffle", systemImage: "shuffle").frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.glass)
                .controlSize(.large)
                .listRowSeparator(.hidden)
            }
            ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                row(item, index: i)
                    .onAppear { if i == items.count - 20 { loadMore() } }
            }
        }
        .listStyle(.plain)
        .navigationTitle(section.title)
        .overlay {
            if items.isEmpty && exhausted {
                ContentUnavailableView(emptyText, systemImage: section.symbol)
            }
        }
        .task(id: model.libraryVersion) { reload() }
    }

    @ViewBuilder
    private func row(_ item: Item, index: Int) -> some View {
        if item.kind == .track {
            Button { playFrom(index) } label: { ItemRow(item: item) }
                .buttonStyle(.plain)
                .itemMenu(item)
        } else {
            NavigationLink(value: item) { ItemRow(item: item) }
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
        items = model.core.section(section: section, offset: 0, limit: pageSize)
        exhausted = items.count < Int(pageSize)
    }

    private func loadMore() {
        guard !exhausted else { return }
        let more = model.core.section(section: section, offset: UInt32(items.count), limit: pageSize)
        exhausted = more.count < Int(pageSize)
        items.append(contentsOf: more)
    }

    private func playFrom(_ index: Int) {
        let tracks = model.core.sectionTracks(section: section)
        let id = items[index].id
        let start = tracks.firstIndex { $0.id == id } ?? 0
        model.play(tracks, at: start, from: Item(id: "library:\(section.title)", kind: .playlist, source: .local, title: section.title, subtitle: "Library", art: nil, downloaded: false))
    }

    private func playAll(shuffle: Bool) {
        let tracks = model.core.sectionTracks(section: section)
        model.play(tracks, from: nil, shuffle: shuffle)
    }
}
