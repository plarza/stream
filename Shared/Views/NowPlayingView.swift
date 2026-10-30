import AVKit
import SwiftUI

/// Immersive Now Playing: the cover blurred edge to edge behind everything,
/// the big cover (which shrinks to a header for lyrics and queue), then the
/// scrubber and transport on the blurred art.
struct NowPlayingView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Namespace private var art
    var compact = false

    private var track: Track? { model.player.track }
    private var playing: Bool { model.player.status == .playing }

    var body: some View {
        GeometryReader { geo in
            ZStack {
                ImmersiveBackground(url: track?.art, palette: model.accent)
                VStack(spacing: 0) {
                    #if os(iOS)
                    Capsule().fill(.white.opacity(0.35)).frame(width: 38, height: 5).padding(.top, 8)
                    #endif
                    content(width: min(geo.size.width - Metrics.nowPlaying * 2, 520), height: geo.size.height)
                        .frame(maxWidth: 520)
                }
                .frame(maxWidth: .infinity)
                .padding(.horizontal, Metrics.nowPlaying)
                .padding(.bottom, 12)
            }
        }
        .environment(\.colorScheme, .dark)
        .foregroundStyle(.white)
    }

    @ViewBuilder
    private func content(width: CGFloat, height: CGFloat) -> some View {
        let mode = model.nowPlayingMode
        VStack(spacing: 0) {
            if mode == .artwork {
                Spacer(minLength: 12)
                // Leave room for the title, scrubber and controls below.
                bigArtwork(width: max(160, min(width - Metrics.coverInset * 2, height - 380)))
                Spacer(minLength: 20)
                titleRow
                    .transition(.opacity)
            } else {
                compactHeader
                    .padding(.top, 18)
                Group {
                    if mode == .lyrics {
                        LyricsView(track: track)
                    } else {
                        QueueView()
                    }
                }
                .transition(.asymmetric(insertion: .opacity.combined(with: .move(edge: .bottom)), removal: .opacity))
                .mask(LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .black, location: 0.06), .init(color: .black, location: 0.88), .init(color: .clear, location: 1)], startPoint: .top, endPoint: .bottom))
            }
            Scrubber()
                .padding(.top, 22)
            Transport()
                .padding(.top, 18)
                .padding(.bottom, 26)
            bottomBar
        }
        .animation(.player, value: mode)
    }

    private func bigArtwork(width: CGFloat) -> some View {
        FillArtwork(url: track?.art)
            .frame(width: width, height: width)
            .clipShape(RoundedRectangle(cornerRadius: Metrics.radius(for: width), style: .continuous))
            .shadow(color: .black.opacity(playing ? 0.4 : 0.22), radius: playing ? 28 : 14, y: playing ? 16 : 8)
            .scaleEffect(playing ? 1 : 0.84)
            .animation(.spring(response: 0.5, dampingFraction: 0.72), value: playing)
            .matchedGeometryEffect(id: "art", in: art)
            .onTapGesture { model.toggle() }
    }

    private var compactHeader: some View {
        HStack(spacing: Metrics.artGap) {
            FillArtwork(url: track?.art, pixels: 300)
                .frame(width: 60, height: 60)
                .clipShape(RoundedRectangle(cornerRadius: Metrics.radius(for: 60), style: .continuous))
                .shadow(color: .black.opacity(0.3), radius: 8, y: 4)
                .matchedGeometryEffect(id: "art", in: art)
            VStack(alignment: .leading, spacing: 2) {
                Text(track?.title ?? "Not Playing").font(.headline).lineLimit(1)
                Text(track?.artist ?? "").font(.subheadline).foregroundStyle(.white.opacity(0.65)).lineLimit(1)
            }
            .id(track?.id)
            .transition(.blurReplace)
            Spacer()
            likeButton
            moreMenu
        }
        .padding(.bottom, 8)
        .animation(.smooth, value: track?.id)
    }

    private var titleRow: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(track?.title ?? "Not Playing")
                    .accessibilityIdentifier("nowPlaying.title")
                    .font(.title2.weight(.bold))
                    .lineLimit(1)
                    .contentTransition(.opacity)
                Text(track?.artist ?? "")
                    .font(.title3)
                    .foregroundStyle(.white.opacity(0.65))
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .id(track?.id)
            .transition(.blurReplace)
            likeButton
            moreMenu
        }
        .animation(.smooth, value: track?.id)
    }

    private var likeButton: some View {
        Button {
            if let t = track { model.setLiked(t, !t.liked) }
        } label: {
            Image(systemName: track?.liked == true ? "heart.fill" : "heart")
                .font(.title3)
                .foregroundStyle(track?.liked == true ? model.tint : .white)
                .contentTransition(.symbolEffect(.replace))
                .frame(width: 38, height: 38)
                .glassEffect(.regular.interactive(), in: .circle)
        }
        .buttonStyle(.pressable)
        .disabled(track == nil)
        .accessibilityLabel(track?.liked == true ? "Unlike" : "Like")
        .accessibilityIdentifier("nowPlaying.like")
    }

    private var moreMenu: some View {
        Menu {
            Toggle(isOn: Binding(get: { model.player.shuffle }, set: { model.core.setShuffle(on: $0) })) {
                Label("Shuffle", systemImage: "shuffle")
            }
            Picker(selection: Binding(get: { model.player.repeat }, set: { model.core.setRepeat(mode: $0) })) {
                Label("Repeat Off", systemImage: "repeat").tag(RepeatMode.off)
                Label("Repeat All", systemImage: "repeat").tag(RepeatMode.all)
                Label("Repeat One", systemImage: "repeat.1").tag(RepeatMode.one)
            } label: {
                Label("Repeat", systemImage: "repeat")
            }
            if let t = track {
                Divider()
                if let album = t.albumId {
                    Button { open(Item(id: album, kind: .album, source: t.source, title: t.album, subtitle: t.artist, art: t.art, downloaded: false)) } label: {
                        Label("Go to Album", systemImage: "square.stack")
                    }
                }
                if let artist = t.artistId {
                    Button { open(Item(id: artist, kind: .artist, source: t.source, title: t.artist, subtitle: "Artist", art: nil, downloaded: false)) } label: {
                        Label("Go to Artist", systemImage: "music.mic")
                    }
                }
                Button { model.togglePin(t.item) } label: { Label("Pin Song", systemImage: "pin") }
            }
        } label: {
            Image(systemName: "ellipsis")
                .font(.title3)
                .frame(width: 38, height: 38)
                .glassEffect(.regular.interactive(), in: .circle)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .accessibilityLabel("More")
        .accessibilityIdentifier("nowPlaying.more")
    }

    private func open(_ item: Item) {
        model.pendingNavigation = item
        model.showNowPlaying = false
        dismiss()
    }

    private var bottomBar: some View {
        HStack {
            modeButton(.lyrics, symbol: "quote.bubble").accessibilityLabel("Lyrics").accessibilityIdentifier("nowPlaying.lyrics")
            Spacer()
            HStack(spacing: 8) {
                if let q = track?.quality, !q.isEmpty { QualityChip(text: q) }
            }
            Spacer()
            AirPlayButton()
                .frame(width: 36, height: 36)
            Spacer()
            modeButton(.queue, symbol: "list.bullet").accessibilityLabel("Queue").accessibilityIdentifier("nowPlaying.queue")
        }
        .font(.title3)
        .foregroundStyle(.white.opacity(0.8))
    }

    private func modeButton(_ mode: AppModel.NowPlayingMode, symbol: String) -> some View {
        let on = model.nowPlayingMode == mode
        return Button {
            Haptics.tap()
            withAnimation(.player) { model.nowPlayingMode = on ? .artwork : mode }
        } label: {
            Image(systemName: on && symbol == "quote.bubble" ? "quote.bubble.fill" : symbol)
                .frame(width: 40, height: 36)
                .background { if on { Capsule().fill(.white.opacity(0.18)) } }
                .contentTransition(.symbolEffect(.replace))
        }
        .buttonStyle(.pressable)
    }
}

/// The current cover, blurred to fill the screen, crossfading between songs.
struct ImmersiveBackground: View {
    let url: String?
    let palette: Palette

    var body: some View {
        ZStack {
            LinearGradient(colors: [palette.top, palette.bottom], startPoint: .top, endPoint: .bottom)
            // A tiny cover blurred is cheap and looks the same as a big one.
            FillArtwork(url: url, pixels: 128)
                .blur(radius: 70, opaque: true)
                .saturation(1.5)
                .opacity(0.85)
                .scaleEffect(1.3)
            LinearGradient(colors: [.black.opacity(0.05), .black.opacity(0.45)], startPoint: .top, endPoint: .bottom)
        }
        .ignoresSafeArea()
        .animation(.smooth(duration: 0.8), value: url)
    }
}

/// Apple Music-style scrubber: a thin bar that thickens while dragged.
struct Scrubber: View {
    @Environment(AppModel.self) private var model
    @State private var dragging: Double?

    var body: some View {
        let duration = max(1, Double(model.player.durationMs) / 1000)
        TimelineView(.animation(minimumInterval: 1 / 15, paused: model.player.status != .playing || dragging != nil)) { _ in
            let pos = dragging ?? min(duration, model.position())
            VStack(spacing: 6) {
                GeometryReader { geo in
                    let w = geo.size.width
                    ZStack(alignment: .leading) {
                        Capsule().fill(.white.opacity(0.22))
                        Capsule().fill(.white.opacity(dragging == nil ? 0.85 : 1))
                            .frame(width: max(0, w * pos / duration))
                    }
                    .frame(height: dragging == nil ? 7 : 12)
                    .frame(maxHeight: .infinity)
                    .contentShape(Rectangle())
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { v in
                                withAnimation(.press) {
                                    dragging = min(duration, max(0, v.location.x / w * duration))
                                }
                            }
                            .onEnded { _ in
                                if let d = dragging { model.seek(to: UInt32(d * 1000)) }
                                Haptics.tap()
                                withAnimation(.press) { dragging = nil }
                            }
                    )
                }
                .frame(height: 20)
                HStack {
                    Text(Format.time(pos))
                    Spacer()
                    Text("-" + Format.time(duration - pos))
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.white.opacity(dragging == nil ? 0.55 : 0.9))
            }
            .scaleEffect(x: dragging == nil ? 1 : 1.03, y: 1)
        }
    }
}

struct Transport: View {
    @Environment(AppModel.self) private var model
    var size: CGFloat = 1

    var body: some View {
        HStack {
            Spacer()
            Button { model.previous() } label: {
                Image(systemName: "backward.fill").font(.system(size: 30 * size))
            }
            .buttonStyle(PressableStyle(scale: 0.82))
            .accessibilityLabel("Previous")
            .accessibilityIdentifier("nowPlaying.previous")
            Spacer()
            Button { model.toggle() } label: {
                Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                    .font(.system(size: 46 * size))
                    .contentTransition(.symbolEffect(.replace.offUp))
                    .frame(width: 70 * size, height: 70 * size)
            }
            .buttonStyle(PressableStyle(scale: 0.86))
            .accessibilityLabel(model.player.status == .playing ? "Pause" : "Play")
            .accessibilityIdentifier("nowPlaying.playPause")
            .keyboardShortcut(.space, modifiers: [])
            Spacer()
            Button { model.next() } label: {
                Image(systemName: "forward.fill").font(.system(size: 30 * size))
            }
            .buttonStyle(PressableStyle(scale: 0.82))
            .accessibilityLabel("Next")
            .accessibilityIdentifier("nowPlaying.next")
            Spacer()
        }
        .foregroundStyle(.white)
        .disabled(model.player.track == nil)
    }
}

struct AirPlayButton: View {
    var body: some View {
        #if os(iOS)
        AirPlayRepresentable()
        #else
        AirPlayRepresentableMac()
        #endif
    }
}

#if os(iOS)
private struct AirPlayRepresentable: UIViewRepresentable {
    func makeUIView(context: Context) -> AVRoutePickerView {
        let v = AVRoutePickerView()
        v.tintColor = .white.withAlphaComponent(0.8)
        v.activeTintColor = .white
        v.prioritizesVideoDevices = false
        return v
    }
    func updateUIView(_ uiView: AVRoutePickerView, context: Context) {}
}
#else
private struct AirPlayRepresentableMac: NSViewRepresentable {
    func makeNSView(context: Context) -> AVRoutePickerView {
        let v = AVRoutePickerView()
        v.isRoutePickerButtonBordered = false
        return v
    }
    func updateNSView(_ nsView: AVRoutePickerView, context: Context) {}
}
#endif
