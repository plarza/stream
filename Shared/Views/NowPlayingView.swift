import StreamBindings
import AzaUI
import AVKit
import SwiftUI

/// Immersive Now Playing: the cover blurred edge to edge behind everything,
/// the big cover (which shrinks to a header for lyrics and queue), then the
/// scrubber and transport on the blurred art.
struct NowPlayingView: View {
    @Environment(\.accent) private var accent
    @Environment(AppModel.self) private var model
    @Namespace private var art
    @AppStorage("visualizer") private var visualizer = true
    var compact = false

    private var track: Track? { model.player.track }
    private var playing: Bool { model.player.status == .playing }
    private let smallArt: CGFloat = 60

    var body: some View {
        GeometryReader { geo in
            ZStack {
                ImmersiveBackground(url: track?.art, palette: model.accent)
                if visualizer { NowPlayingVisualizer(url: track?.art).transition(.opacity) }
                VStack(spacing: 0) {
                    #if os(iOS)
                    Capsule().fill(.white.opacity(0.35)).frame(width: 38, height: 5).padding(.top, 8)
                    #else
                    // Room for the window's close chevron above.
                    Color.clear.frame(height: 40)
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
        // Leave room for the title, scrubber and controls below.
        let big = max(160, min(width - Metrics.coverInset * 2, height - 400))
        VStack(spacing: 0) {
            if mode == .artwork {
                Spacer(minLength: 12)
                artSlot(big)
                Spacer(minLength: 20)
                titleRow
                    .transition(.opacity)
            } else {
                compactHeader
                    .padding(.top, 18)
                    .transition(.opacity)
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
                .padding(.top, 14)
                .padding(.bottom, 18)
            bottomBar
        }
        // One cover, flying between the big slot and the header's: it morphs
        // continuously rather than crossfading two copies.
        .overlay { cover(size: mode == .artwork ? big : smallArt, expanded: mode == .artwork) }
        .animation(.player, value: mode)
    }

    /// Where the cover sits in the current layout.
    private func artSlot(_ size: CGFloat) -> some View {
        Color.clear
            .frame(width: size, height: size)
            .matchedGeometryEffect(id: "art", in: art, isSource: true)
    }

    private func cover(size: CGFloat, expanded: Bool) -> some View {
        FillArtwork(url: track?.art)
            .clipShape(RoundedRectangle(cornerRadius: Metrics.radius(for: size), style: .continuous))
            .matchedGeometryEffect(id: "art", in: art, isSource: false)
            .shadow(color: .black.opacity(expanded && playing ? 0.4 : 0.25), radius: expanded ? (playing ? 28 : 14) : 8, y: expanded ? (playing ? 16 : 8) : 4)
            .scaleEffect(expanded && !playing ? 0.84 : 1)
            .animation(.spring(response: 0.5, dampingFraction: 0.72), value: playing)
            // Tapping the cover opens its album; pulling it down closes Now Playing.
            .onTapGesture { if expanded, let t = track { model.openAlbum(of: t) } }
            #if os(iOS)
            // The system's own swipe moves the card; this only makes sure a clear
            // pull ends in closing rather than springing back.
            .simultaneousGesture(
                DragGesture(minimumDistance: 10).onEnded { v in
                    if v.translation.height > 90 || v.predictedEndTranslation.height > 260 {
                        model.showNowPlaying = false
                    }
                }
            )
            #endif
            .accessibilityElement()
            .accessibilityLabel("Go to Album")
            .accessibilityAddTraits(.isButton)
            .accessibilityIdentifier("nowPlaying.cover")
    }

    private var compactHeader: some View {
        HStack(spacing: Metrics.artGap) {
            artSlot(smallArt)
            VStack(alignment: .leading, spacing: 2) {
                MarqueeText(text: track?.title ?? "Not Playing").font(.headline)
                artistLink.font(.subheadline)
                quality.padding(.top, 2)
            }
            .id(track?.id)
            .transition(.blurReplace)
            Spacer()
            likeButton
        }
        .padding(.bottom, 8)
        .animation(.smooth, value: track?.id)
    }

    private var titleRow: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                MarqueeText(text: track?.title ?? "Not Playing")
                    .font(.title2.weight(.bold))
                    .accessibilityIdentifier("nowPlaying.title")
                artistLink.font(.title3)
                quality.padding(.top, 4)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .id(track?.id)
            .transition(.blurReplace)
            likeButton
        }
        .animation(.smooth, value: track?.id)
    }

    /// The artists' names, each opening its own page.
    @ViewBuilder
    private var artistLink: some View {
        if let track {
            ArtistLinks(track: track)
                .tint(.white.opacity(0.65))
                .foregroundStyle(.white.opacity(0.65))
                .lineLimit(1)
                .accessibilityIdentifier("nowPlaying.artist")
        }
    }

    @ViewBuilder
    private var quality: some View {
        if let q = track?.quality, !q.isEmpty {
            QualityChip(text: q).accessibilityIdentifier("nowPlaying.quality")
        }
    }

    private var likeButton: some View {
        Button {
            if let t = track { model.setLiked(t, !t.liked) }
        } label: {
            Image(systemName: track?.liked == true ? "heart.fill" : "heart")
                .font(.title3)
                .foregroundStyle(track?.liked == true ? accent : .white)
                .contentTransition(.symbolEffect(.replace))
                .frame(width: 38, height: 38)
                .glassEffect(.regular.interactive(), in: .circle)
                .padding(3)
                .contentShape(.circle)
        }
        .buttonStyle(.pressable)
        .disabled(track == nil)
        .accessibilityLabel(track?.liked == true ? "Unlike" : "Like")
        .accessibilityIdentifier("nowPlaying.like")
    }

    private var bottomBar: some View {
        HStack(spacing: 0) {
            modeButton(.lyrics, symbol: "quote.bubble").accessibilityLabel("Lyrics").accessibilityIdentifier("nowPlaying.lyrics")
            Spacer()
            // The route picker draws its glyph about half the size of its frame.
            AirPlayButton()
                .frame(width: NowPlayingMetrics.glyph * 2, height: NowPlayingMetrics.glyph * 2)
                .frame(width: 64, height: 48)
            Spacer()
            visualizerButton
            Spacer()
            modeButton(.queue, symbol: "list.bullet").accessibilityLabel("Queue").accessibilityIdentifier("nowPlaying.queue")
        }
        .font(.system(size: NowPlayingMetrics.glyph, weight: .medium))
        .foregroundStyle(.white.opacity(0.7))
        .padding(.horizontal, 28)
    }

    /// Aurora on and off, right where you see it.
    private var visualizerButton: some View {
        Button {
            Haptics.tap()
            withAnimation(.smooth(duration: 0.5)) { visualizer.toggle() }
        } label: {
            Image(systemName: visualizer ? "waveform" : "waveform.slash")
                .font(.system(size: NowPlayingMetrics.glyph, weight: .medium))
                .foregroundStyle(visualizer ? AnyShapeStyle(accent) : AnyShapeStyle(.white.opacity(0.7)))
                .contentTransition(.symbolEffect(.replace))
                // The whole cell answers, not just the glyph.
                .frame(width: 64, height: 48)
                .contentShape(Rectangle())
        }
        .buttonStyle(.pressable)
        .accessibilityLabel("Visualizer")
        .accessibilityValue(visualizer ? "on" : "off")
        .accessibilityIdentifier("nowPlaying.visualizer")
    }

    private func modeButton(_ mode: AppModel.NowPlayingMode, symbol: String) -> some View {
        let on = model.nowPlayingMode == mode
        return Button {
            Haptics.tap()
            withAnimation(.player) { model.nowPlayingMode = on ? .artwork : mode }
        } label: {
            Image(systemName: on && symbol == "quote.bubble" ? "quote.bubble.fill" : symbol)
                .foregroundStyle(on ? AnyShapeStyle(accent) : AnyShapeStyle(.white.opacity(0.7)))
                .contentTransition(.symbolEffect(.replace))
                // The whole cell answers, not just the glyph.
                .frame(width: 64, height: 48)
                .contentShape(Rectangle())
        }
        .buttonStyle(.pressable)
    }
}

enum NowPlayingMetrics {
    /// Every secondary control (shuffle, repeat, lyrics, AirPlay, visualizer, queue) draws at this size.
    static let glyph: CGFloat = 18
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

/// Shuffle · previous · play · next · repeat.
struct Transport: View {
    @Environment(\.accent) private var accent
    @Environment(AppModel.self) private var model
    var size: CGFloat = 1

    var body: some View {
        let shuffle = model.player.shuffle
        let loop = model.player.repeat == .one
        HStack(spacing: 0) {
            toggle("shuffle", on: shuffle, label: "Shuffle", id: "nowPlaying.shuffle") {
                model.setShuffle(!shuffle)
            }
            Spacer(minLength: 0)
            Button { model.previous() } label: {
                Image(systemName: "backward.fill").font(.system(size: 30 * size))
                    .frame(width: 64 * size, height: 64 * size)
                    .contentShape(Rectangle())
            }
            .buttonStyle(PressableStyle(scale: 0.82))
            .accessibilityLabel("Previous")
            .accessibilityIdentifier("nowPlaying.previous")
            Spacer(minLength: 0)
            Button { model.toggle() } label: {
                Image(systemName: model.player.status == .playing ? "pause.fill" : "play.fill")
                    .font(.system(size: 46 * size))
                    .contentTransition(.symbolEffect(.replace.offUp))
                    .frame(width: 76 * size, height: 76 * size)
                    .contentShape(Rectangle())
            }
            .buttonStyle(PressableStyle(scale: 0.86))
            .accessibilityLabel(model.player.status == .playing ? "Pause" : "Play")
            .accessibilityIdentifier("nowPlaying.playPause")
            .keyboardShortcut(.space, modifiers: [])
            Spacer(minLength: 0)
            Button { model.next() } label: {
                Image(systemName: "forward.fill").font(.system(size: 30 * size))
                    .frame(width: 64 * size, height: 64 * size)
                    .contentShape(Rectangle())
            }
            .buttonStyle(PressableStyle(scale: 0.82))
            .accessibilityLabel("Next")
            .accessibilityIdentifier("nowPlaying.next")
            Spacer(minLength: 0)
            // Repeat here means one thing: this song, forever.
            toggle("repeat", on: loop, label: "Repeat", id: "nowPlaying.repeat") {
                model.setRepeat(!loop)
            }
        }
        .foregroundStyle(.white)
        .disabled(model.player.track == nil)
    }

    private func toggle(_ symbol: String, on: Bool, label: String, id: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: NowPlayingMetrics.glyph * size, weight: on ? .semibold : .medium))
                .foregroundStyle(on ? accent : .white.opacity(0.7))
                .frame(width: 48 * size, height: 64 * size)
                .contentShape(Rectangle())
        }
        .buttonStyle(PressableStyle(scale: 0.82))
        .animation(.snappy, value: on)
        .accessibilityLabel(label)
        .accessibilityValue(on ? "on" : "off")
        .accessibilityIdentifier(id)
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
