import SwiftUI

/// One song in a list.
struct TrackRow: View {
    let track: Track
    var showArt = true
    var number: Int?
    var isCurrent = false
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 12) {
            if let number, !showArt {
                ZStack {
                    if isCurrent {
                        NowPlayingBars(playing: model.player.status == .playing)
                            .foregroundStyle(model.accent.accent)
                    } else {
                        Text("\(number)").font(.subheadline.monospacedDigit()).foregroundStyle(.secondary)
                    }
                }
                .frame(width: 22)
            } else if showArt {
                ArtworkView(url: track.art, size: 44, cornerRadius: 6)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(track.title)
                    .font(.body)
                    .foregroundStyle(isCurrent ? model.accent.accent : .primary)
                    .lineLimit(1)
                HStack(spacing: 4) {
                    if track.downloaded { DownloadedBadge() }
                    Text(track.artist).lineLimit(1)
                }
                .font(.subheadline)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            if track.liked {
                Image(systemName: "heart.fill").font(.caption).foregroundStyle(model.accent.accent.opacity(0.9))
            }
            Text(Format.time(Double(track.durationMs) / 1000))
                .font(.subheadline.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
        .contentShape(Rectangle())
    }
}

/// Any item (song/album/artist/playlist) as a list row.
struct ItemRow: View {
    let item: Item
    var body: some View {
        HStack(spacing: 12) {
            ArtworkView(url: item.art, size: 48, cornerRadius: 6, circle: item.kind == .artist)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title).lineLimit(1)
                HStack(spacing: 4) {
                    if item.downloaded { DownloadedBadge() }
                    Text(item.subtitle).lineLimit(1)
                }
                .font(.subheadline)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            if item.isCollection {
                Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
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
            ArtworkView(url: item.art, size: size, cornerRadius: item.kind == .artist ? size / 2 : 10, circle: item.kind == .artist)
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
            if item.isCollection {
                Button { model.play(item, shuffle: true) } label: { Label("Shuffle", systemImage: "shuffle") }
            }
            Divider()
            Button { Task { model.playNext(await resolve()) } } label: { Label("Play Next", systemImage: "text.line.first.and.arrowtriangle.forward") }
            Button { Task { model.playLater(await resolve()) } } label: { Label("Play Later", systemImage: "text.line.last.and.arrowtriangle.forward") }
            Divider()
            let pinned = model.core.isPinned(id: item.id)
            Button { model.togglePin(item) } label: { Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin") }
        }
    }

    private func resolve() async -> [Track] {
        if let tracks { return await tracks() }
        if item.kind == .track {
            return [Track(id: item.id, source: item.source, title: item.title, artist: item.subtitle, artistId: nil, album: "", albumId: nil, art: item.art, durationMs: 0, trackNumber: 0, discNumber: 0, downloaded: item.downloaded, liked: false, quality: "")]
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
