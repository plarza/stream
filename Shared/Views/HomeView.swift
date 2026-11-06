import StreamBindings
import SwiftUI

/// Pinned grid (albums, songs, artists, playlists) and recents.
struct HomeView: View {
    @Environment(AppModel.self) private var model
    var columns = 4
    @Namespace private var pins

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                PageTitle(text: "Home")
                if model.home.pins.isEmpty && model.home.recents.isEmpty && model.home.songs.isEmpty {
                    empty
                }
                if !model.home.pins.isEmpty {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 12), count: columns), spacing: 14) {
                        ForEach(model.home.pins) { item in
                            pinTile(item)
                                .accessibilityIdentifier("pin.\(item.title)")
                                .transition(.scale(scale: 0.85).combined(with: .opacity))
                        }
                    }
                    .animation(.player, value: model.home.pins.map(\.id))
                    .padding(.top, 4)
                }
                #if os(macOS)
                recentShelves
                #else
                if !model.home.recents.isEmpty {
                    Text("Recents")
                        .font(.title3.weight(.bold))
                        .padding(.top, 20)
                    LazyVStack(spacing: 0) {
                        ForEach(model.home.recents) { item in
                            link(item) { ItemRow(item: item).padding(.vertical, 6) }
                                .accessibilityIdentifier("recent.\(item.title)")
                                .scrollPolish()
                        }
                    }
                }
                #endif
            }
            .padding(.horizontal, Metrics.page)
            .padding(.bottom, 120)
        }
        #if os(iOS)
        .refreshable { await model.refreshSpotify() }
        #endif
    }

    /// The Mac's room: what you played lately, a shelf per kind.
    @ViewBuilder
    private var recentShelves: some View {
        let home = model.home
        if !home.artists.isEmpty {
            heading("Recent artists")
            shelf(home.artists) { item in
                VStack(spacing: 8) {
                    ArtworkView(url: item.art, size: 150, circle: true)
                    Text(item.title).font(.footnote.weight(.medium)).lineLimit(1)
                }
                .foregroundStyle(.primary)
            }
        }
        if !home.albums.isEmpty {
            heading("Recent albums")
            shelf(home.albums) { ReleaseTile(item: $0) }
        }
        if !home.songs.isEmpty {
            heading("Recent songs")
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 24), GridItem(.flexible(), spacing: 24)], alignment: .leading, spacing: 0) {
                ForEach(home.songs.prefix(8)) { item in
                    link(item) { ItemRow(item: item).padding(.vertical, 6) }
                        .accessibilityIdentifier("recent.\(item.title)")
                }
            }
        }
        let playlists = home.recents.filter { $0.kind == .playlist || $0.kind == .show }
        if !playlists.isEmpty {
            heading("Recent playlists")
            shelf(playlists) { ReleaseTile(item: $0) }
        }
    }

    private func heading(_ text: String) -> some View {
        Text(text).font(.title3.weight(.bold)).padding(.top, 24).padding(.bottom, 4)
    }

    /// One row of tiles that scrolls sideways, flush with the page's margin.
    private func shelf(_ items: [Item], @ViewBuilder tile: @escaping (Item) -> some View) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            LazyHStack(alignment: .top, spacing: 16) {
                ForEach(items) { item in
                    link(item) { tile(item).frame(width: 150) }
                        .accessibilityIdentifier("recent.\(item.title)")
                }
            }
            .padding(.horizontal, Metrics.page)
        }
        .padding(.horizontal, -Metrics.page)
        .scrollClipDisabled()
    }

    private func pinTile(_ item: Item) -> some View {
        link(item) {
            GeometryReader { geo in
                ArtworkView(url: item.art, size: geo.size.width, circle: item.kind == .artist)
            }
            .aspectRatio(1, contentMode: .fit)
            .overlay(alignment: .bottomTrailing) {
                if item.kind == .track {
                    Image(systemName: "play.fill")
                        .font(.caption2)
                        .padding(6)
                        .glassEffect(.regular, in: .circle)
                        .padding(4)
                }
            }
            .accessibilityLabel(item.title)
        }
    }

    @ViewBuilder
    private func link(_ item: Item, @ViewBuilder label: () -> some View) -> some View {
        if item.kind == .track {
            Button { model.play(item) } label: { label() }
                .buttonStyle(.pressable)
                .itemMenu(item)
        } else {
            NavigationLink(value: item) { label() }
                .buttonStyle(.pressable)
                .itemMenu(item)
        }
    }

    private var empty: some View {
        ContentUnavailableView {
            Label("Nothing pinned yet", systemImage: "pin")
        } actions: {
            #if os(iOS)
            if !model.account.spotifyWeb {
                Button("Connect Spotify") { model.showSettings = true }
                    .buttonStyle(.glassProminent)
                    .accessibilityIdentifier("home.connectSpotify")
            }
            #endif
        }
        .padding(.top, 80)
    }
}
