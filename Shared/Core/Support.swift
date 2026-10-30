import SwiftUI

enum Haptics {
    @MainActor static func tap() {
        #if os(iOS)
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        #endif
    }

    @MainActor static func success() {
        #if os(iOS)
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        #endif
    }
}

enum Format {
    static func time(_ seconds: Double) -> String {
        let s = max(0, Int(seconds.rounded(.down)))
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }

    static func bytes(_ n: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(n), countStyle: .file)
    }
}

extension Item: Identifiable {}
extension Track: Identifiable {}

extension Item {
    var isCollection: Bool { kind != .track }
    var symbol: String {
        switch kind {
        case .track: "music.note"
        case .album: "square.stack"
        case .artist: "music.mic"
        case .playlist: "music.note.list"
        }
    }
}

extension Track {
    var item: Item {
        Item(id: id, kind: .track, source: source, title: title, subtitle: artist, art: art, downloaded: downloaded)
    }
}

/// Spacing and corner radii shared by every screen, so edges line up.
enum Metrics {
    /// Side margin of pages, lists and headers.
    static let page: CGFloat = 20
    /// Side margin of Now Playing's controls.
    static let nowPlaying: CGFloat = 30
    /// The Now Playing cover sits this much further in than the controls.
    static let coverInset: CGFloat = 10
    /// Gap between artwork and the text beside it.
    static let artGap: CGFloat = 12
    /// Corner radius for artwork at a given size: small thumbnails are tighter.
    static func radius(for size: CGFloat) -> CGFloat {
        switch size {
        case ..<36: 4
        case ..<64: 6
        case ..<160: 10
        default: 14
        }
    }
}

/// A large page title. iPhone shows it in the navigation bar; the Mac window
/// has no title bar, so it sits at the top of the page.
struct PageTitle: View {
    let text: String
    var body: some View {
        #if os(macOS)
        Text(text)
            .font(.largeTitle.weight(.bold))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 8)
            .padding(.bottom, 4)
        #endif
    }
}

/// Motion used across the app, so everything moves with the same physics.
extension Animation {
    static let player = Animation.spring(response: 0.42, dampingFraction: 0.86)
    static let press = Animation.spring(response: 0.25, dampingFraction: 0.7)
}

/// Buttons that dip slightly when pressed, like system controls.
struct PressableStyle: ButtonStyle {
    var scale: CGFloat = 0.94
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? scale : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(.press, value: configuration.isPressed)
    }
}

extension ButtonStyle where Self == PressableStyle {
    static var pressable: PressableStyle { PressableStyle() }
}

/// The ⬇ marker shared by synced local files and Spotify downloads.
struct DownloadedBadge: View {
    var body: some View {
        Image(systemName: "arrow.down.circle.fill")
            .font(.caption2)
            .foregroundStyle(.secondary)
            .accessibilityLabel("Downloaded")
    }
}

/// "FLAC 24/96" in a small glass capsule.
struct QualityChip: View {
    let text: String
    var body: some View {
        if !text.isEmpty {
            Text(text)
                .font(.caption2.weight(.semibold).monospacedDigit())
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .glassEffect(.regular, in: .capsule)
        }
    }
}

struct Toast: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.subheadline.weight(.medium))
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .glassEffect(.regular, in: .capsule)
            .transition(.move(edge: .top).combined(with: .opacity))
    }
}

/// Scroll-in effect for grid tiles and rows: a touch of scale and fade at the edges.
extension View {
    func scrollPolish() -> some View {
        scrollTransition(.interactive, axis: .vertical) { content, phase in
            content
                .opacity(phase.isIdentity ? 1 : 0.6)
                .scaleEffect(phase.isIdentity ? 1 : 0.96)
        }
    }
}

/// Text that opens a page (an artist's name): plain, underlined under the pointer.
struct TextLinkStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Link(configuration: configuration)
    }

    private struct Link: View {
        let configuration: ButtonStyleConfiguration
        @State private var hover = false

        var body: some View {
            configuration.label
                .underline(hover)
                .opacity(configuration.isPressed ? 0.55 : 1)
                .contentShape(Rectangle())
                .onHover { hover = $0 }
        }
    }
}

extension ButtonStyle where Self == TextLinkStyle {
    static var textLink: TextLinkStyle { TextLinkStyle() }
}
