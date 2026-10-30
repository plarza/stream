import SwiftUI

/// One song in a list.
struct TrackRow: View {
    let track: Track
    var showArt = true
    var number: Int?
    var isCurrent = false
    /// Album pages hide the artist when it's the album's own.
    var showArtist = true
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: Metrics.artGap) {
            if let number, !showArt {
                ZStack {
                    if isCurrent {
                        NowPlayingBars(playing: model.player.status == .playing)
                            .foregroundStyle(model.tint)
                    } else {
                        Text("\(number)").font(.subheadline.monospacedDigit()).foregroundStyle(.secondary)
                    }
                }
                .frame(width: 22)
            } else if showArt {
                ArtworkView(url: track.art, size: 48)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(track.title)
                    .font(.body)
                    .foregroundStyle(isCurrent ? model.tint : .primary)
                    .lineLimit(1)
                if showArtist {
                    HStack(spacing: 4) {
                        if track.downloaded && track.source == .spotify { DownloadedBadge() }
                        Text(track.artist).lineLimit(1)
                            // The name opens the artist; the rest of the row plays the song.
                            .onTapGesture { model.openArtist(of: track) }
                            .allowsHitTesting(track.artistId != nil)
                    }
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 8)
            if !showArtist && track.downloaded && track.source == .spotify { DownloadedBadge() }
            if track.liked {
                Image(systemName: "heart.fill").font(.caption).foregroundStyle(model.tint.opacity(0.9))
            }
            Text(Format.time(Double(track.durationMs) / 1000))
                .font(.subheadline.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
        .contentShape(Rectangle())
    }
}

/// Any item (song/album/artist/playlist) as a list row; songs show their length.
struct ItemRow: View {
    let item: Item
    /// Lists of one kind (e.g. Artists) don't need the kind repeated on every row.
    var showSubtitle = true
    /// Wide song lists (the Mac's) give the album its own column.
    var showAlbum = false
    @Environment(AppModel.self) private var model

    var body: some View {
        let song = item.kind == .track ? model.core.track(id: item.id) : nil
        HStack(spacing: Metrics.artGap) {
            ArtworkView(url: item.art, size: 48, circle: item.kind == .artist)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title).lineLimit(1)
                if showSubtitle && !item.subtitle.isEmpty {
                    HStack(spacing: 4) {
                        if item.downloaded && item.source == .spotify { DownloadedBadge() }
                        Text(item.subtitle).lineLimit(1)
                    }
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: showAlbum ? .infinity : nil, alignment: .leading)
            if showAlbum, let song {
                Text(song.album)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            Spacer(minLength: 0)
            if let t = song, t.durationMs > 0 {
                Text(Format.time(Double(t.durationMs) / 1000))
                    .font(.subheadline.monospacedDigit())
                    .foregroundStyle(.tertiary)
            }
        }
        .contentShape(Rectangle())
    }
}

/// A square tile for grids (Home pins, recents, artist albums).
struct ItemTile: View {
    let item: Item
    var size: CGFloat
    var showCaption = true

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ArtworkView(url: item.art, size: size, circle: item.kind == .artist)
                .shadow(color: .black.opacity(0.18), radius: 8, y: 4)
            if showCaption {
                VStack(alignment: .leading, spacing: 1) {
                    Text(item.title).font(.footnote.weight(.medium)).lineLimit(1)
                    Text(item.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                .frame(width: size, alignment: .leading)
            }
        }
    }
}

/// Animated equalizer bars marking the playing song.
struct NowPlayingBars: View {
    var playing: Bool
    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 20, paused: !playing)) { ctx in
            let t = ctx.date.timeIntervalSinceReferenceDate
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(0..<3) { i in
                    Capsule()
                        .frame(width: 3, height: playing ? 4 + 9 * abs(sin(t * (3.1 + Double(i) * 1.3) + Double(i))) : 4)
                }
            }
            .frame(height: 14, alignment: .bottom)
        }
    }
}

/// Long-press actions shared by every list and grid.
struct ItemMenu: ViewModifier {
    let item: Item
    var tracks: (() async -> [Track])?
    @Environment(AppModel.self) private var model

    func body(content: Content) -> some View {
        content.contextMenu {
            Button { model.play(item) } label: { Label("Play", systemImage: "play") }
                .accessibilityIdentifier("menu.play")
            if item.isCollection {
                Button { model.play(item, shuffle: true) } label: { Label("Shuffle", systemImage: "shuffle") }
            }
            Divider()
            Button { Task { model.playNext(await resolve()) } } label: { Label("Play Next", systemImage: "text.line.first.and.arrowtriangle.forward") }
                .accessibilityIdentifier("menu.playNext")
            Button { Task { model.playLater(await resolve()) } } label: { Label("Play Later", systemImage: "text.line.last.and.arrowtriangle.forward") }
                .accessibilityIdentifier("menu.playLater")
            if item.kind == .track, let t = model.core.track(id: item.id) {
                Divider()
                if t.artistId != nil {
                    Button { model.openArtist(of: t) } label: { Label("Go to Artist", systemImage: "music.mic") }
                }
                if t.albumId != nil {
                    Button { model.openAlbum(of: t) } label: { Label("Go to Album", systemImage: "square.stack") }
                }
            }
            Divider()
            let pinned = model.core.isPinned(id: item.id)
            Button { model.togglePin(item) } label: { Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin") }
                .accessibilityIdentifier(pinned ? "menu.unpin" : "menu.pin")
        }
    }

    private func resolve() async -> [Track] {
        if let tracks { return await tracks() }
        if item.kind == .track {
            return model.core.track(id: item.id).map { [$0] } ?? []
        }
        return (try? await model.core.collection(id: item.id).tracks) ?? []
    }
}

extension View {
    func itemMenu(_ item: Item, tracks: (() async -> [Track])? = nil) -> some View {
        modifier(ItemMenu(item: item, tracks: tracks))
    }

    func trackMenu(_ track: Track) -> some View {
        modifier(ItemMenu(item: track.item, tracks: { [track] }))
    }
}
