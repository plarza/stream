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
                if model.home.pins.isEmpty && model.home.recents.isEmpty {
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
            }
            .padding(.horizontal, Metrics.page)
            .padding(.bottom, 120)
        }
        #if os(iOS)
        .refreshable { await model.refreshSpotify() }
        #endif
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
        } description: {
            Text("Long-press an album, song, artist or playlist and choose Pin.")
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
