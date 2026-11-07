import StreamBindings
import AzaUI
import SwiftUI

/// One song in a list.
struct TrackRow: View {
    let track: Track
    var showArt = true
    var number: Int?
    /// Album pages hide the artist when it's the album's own.
    var showArtist = true
    @Environment(\.accent) private var accent

    var body: some View {
        HStack(spacing: Metrics.artGap) {
            if let number, !showArt {
                Text("\(number)").font(.subheadline.monospacedDigit()).foregroundStyle(.secondary)
                    .frame(width: 22)
            } else if showArt {
                ArtworkView(url: track.art, size: 48)
            }
            VStack(alignment: .leading, spacing: 2) {
                SongTitle(id: track.id, title: track.title)
                if showArtist {
                    HStack(spacing: 4) {
                        if track.downloadable { DownloadMark(id: track.id, downloaded: track.downloaded) }
                        Text(track.artist).lineLimit(1)
                    }
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 8)
            if !showArtist && track.downloadable { DownloadMark(id: track.id, downloaded: track.downloaded) }
            if track.liked {
                Image(systemName: "heart.fill").font(.caption).foregroundStyle(accent.opacity(0.9))
            }
            Text(Format.time(Double(track.durationMs) / 1000))
                .font(.subheadline.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
        .contentShape(Rectangle())
    }
}

/// A song's title in any list: pink, with the bars before it, while it's the one playing.
struct SongTitle: View {
    let id: String
    let title: String
    @Environment(\.accent) private var accent
    @Environment(AppModel.self) private var model

    var body: some View {
        let current = model.player.track?.id == id
        HStack(spacing: 6) {
            if current {
                NowPlayingBars(playing: model.player.status == .playing)
                    .foregroundStyle(accent)
                    .transition(.scale(scale: 0.5).combined(with: .opacity))
            }
            Text(title)
                .foregroundStyle(current ? accent : .primary)
                .lineLimit(1)
        }
        .animation(.snappy(duration: 0.25), value: current)
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
        let song = item.kind == .track ? model.track(item.id) : nil
        HStack(spacing: Metrics.artGap) {
            ArtworkView(url: item.art, size: 48, circle: item.kind == .artist)
            VStack(alignment: .leading, spacing: 2) {
                if item.kind == .track {
                    SongTitle(id: item.id, title: item.title)
                } else {
                    Text(item.title).lineLimit(1)
                }
                if showSubtitle && !item.subtitle.isEmpty {
                    HStack(spacing: 4) {
                        // A device's own file is "downloaded" in the core, but it's no download.
                        if let song {
                            if song.downloadable { DownloadMark(id: song.id, downloaded: song.downloaded) }
                        } else if item.downloaded {
                            DownloadedBadge()
                        }
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

/// Three bars marking the playing song. They dance on their own gentle waves,
/// with the music setting how big and how quick (loudness) and kicking them up
/// on each beat, redrawn every display frame (120 Hz on ProMotion screens).
/// Paused, they rest in a little staircase.
struct NowPlayingBars: View {
    var playing: Bool
    private static let rest: [CGFloat] = [0.45, 0.8, 0.6]

    var body: some View {
        TimelineView(.animation(paused: !playing)) { ctx in
            let levels = playing ? BarMotion.shared.heights(at: ctx.date).map { CGFloat($0) } : Self.rest
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(0..<3, id: \.self) { i in
                    RoundedRectangle(cornerRadius: 1.5, style: .continuous)
                        .frame(width: 3, height: max(3, 14 * levels[i]))
                }
            }
            .frame(width: 13, height: 14, alignment: .bottom)
        }
        .animation(.spring(duration: 0.4, bounce: 0.3), value: playing)
        .accessibilityHidden(true)
    }
}

/// Turns the meter into bar heights. The levels only steer the bars: a smooth
/// loudness envelope sets their reach and speed, bass onsets give a kick, and
/// each bar rides its own pair of slow waves so they never move in lockstep.
/// Shared, so every set of bars on screen moves together.
@MainActor
final class BarMotion {
    static let shared = BarMotion()
    private var energy: Float = 0
    private var ceiling: Float = 0.5
    private var bassSlow: Float = 0
    private var kick: Float = 0
    private var phase: Float = 0
    private var shown: [Float] = [0.45, 0.8, 0.6]
    private var last = Date.distantPast
    private let speeds: [Float] = [1.0, 1.43, 0.79]
    private let offsets: [Float] = [0, 2.1, 4.3]
    private let kicks: [Float] = [0.8, 1, 0.65]

    func heights(at date: Date) -> [Float] {
        let dt = Float(min(max(date.timeIntervalSince(last), 0), 0.05))
        guard dt > 0 else { return shown }
        last = date
        let raw = AppModel.shared.audio.meter.levels
        let loud = (raw.0 + raw.1 + raw.2) / 3
        // Loudness against the song's own recent peak, eased in and out.
        ceiling = max(loud, ceiling - 0.08 * dt, 0.25)
        let target = min(loud / ceiling, 1)
        energy += (target - energy) * (1 - exp(-(target > energy ? 10 : 3) * dt))
        // A beat: bass jumping above where it's been lately.
        bassSlow += (raw.0 - bassSlow) * (1 - exp(-4 * dt))
        kick = max(kick * exp(-7 * dt), min(max(raw.0 - bassSlow, 0) * 6, 1))
        phase += dt * (2.2 + 3.5 * energy)
        for i in 0..<3 {
            let p = phase * speeds[i] + offsets[i]
            let wave = 0.5 + 0.3 * sin(p) + 0.2 * sin(p * 2.37 + offsets[i] * 1.7)
            let goal = 0.22 + 0.78 * min(energy * (0.35 + 0.65 * wave) + 0.4 * kick * kicks[i], 1)
            shown[i] += (goal - shown[i]) * (1 - exp(-18 * dt))
        }
        return shown
    }
}

/// Long-press actions shared by every list and grid.
struct ItemMenu: ViewModifier {
    let item: Item
    var tracks: (() async -> [Track])?
    /// In one of your playlists: takes it out.
    var remove: (() -> Void)?
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
            if item.kind != .artist, item.kind != .show {
                AddToPlaylistMenu(tracks: resolve)
            }
            if let remove {
                Button(role: .destructive, action: remove) { Label("Remove from Playlist", systemImage: "minus.circle") }
                    .accessibilityIdentifier("menu.removeFromPlaylist")
            }
            if item.kind == .track, let t = model.track(item.id) {
                Divider()
                if t.artists.count > 1 {
                    Menu {
                        ForEach(t.artists, id: \.id) { a in
                            Button(a.name) { model.openArtist(a, source: t.source) }
                        }
                    } label: { Label("Go to Artist", systemImage: "music.mic") }
                } else if t.artistId != nil {
                    Button { model.openArtist(of: t) } label: { Label("Go to Artist", systemImage: "music.mic") }
                }
                if t.albumId != nil {
                    Button { model.openAlbum(of: t) } label: { Label("Go to Album", systemImage: "square.stack") }
                }
            }
            if let downloaded = downloadState {
                Button { model.setDownloaded(item, !downloaded) } label: {
                    Label(downloaded ? "Remove Download" : "Download", systemImage: downloaded ? "arrow.down.circle.fill" : "arrow.down.circle")
                }
                .accessibilityIdentifier(downloaded ? "menu.removeDownload" : "menu.download")
            }
            Divider()
            let pinned = model.isPinned(item.id)
            Button { model.togglePin(item) } label: { Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin") }
                .accessibilityIdentifier(pinned ? "menu.unpin" : "menu.pin")
        }
    }

    /// Whether it's downloaded, for songs and collections that can be; nil for the rest
    /// (artists, and a device's own files).
    private var downloadState: Bool? {
        switch item.kind {
        case .track: model.track(item.id).flatMap { $0.downloadable ? $0.downloaded : nil }
        case .album, .playlist: item.downloaded
        default: nil
        }
    }

    private func resolve() async -> [Track] {
        if let tracks { return await tracks() }
        if item.kind == .track {
            return model.track(item.id).map { [$0] } ?? []
        }
        return (try? await model.core.collection(id: item.id).tracks) ?? []
    }
}

extension View {
    func itemMenu(_ item: Item, tracks: (() async -> [Track])? = nil) -> some View {
        modifier(ItemMenu(item: item, tracks: tracks))
    }

    func trackMenu(_ track: Track, remove: (() -> Void)? = nil) -> some View {
        modifier(ItemMenu(item: track.item, tracks: { [track] }, remove: remove))
    }
}

/// A song's artists, each name opening its own page ("A, B & C" stays one line of text).
struct ArtistLinks: View {
    let track: Track
    @Environment(AppModel.self) private var model

    var body: some View {
        Text(text)
            .environment(\.openURL, OpenURLAction { url in
                guard let a = track.artists.first(where: { Self.url($0) == url }) else { return .discarded }
                model.openArtist(a, source: track.source)
                return .handled
            })
    }

    private var text: AttributedString {
        // No artist pages (a local file with no match): the plain credit.
        guard !track.artists.isEmpty else { return AttributedString(track.artist) }
        var out = AttributedString()
        for (i, a) in track.artists.enumerated() {
            if i > 0 { out += AttributedString(i == track.artists.count - 1 ? " & " : ", ") }
            var name = AttributedString(a.name)
            name.link = Self.url(a)
            out += name
        }
        return out
    }

    private static func url(_ a: ArtistCredit) -> URL? {
        URL(string: "network-aza-stream://open?id=\(a.id.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? a.id)")
    }
}
