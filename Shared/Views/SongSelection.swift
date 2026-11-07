#if os(macOS)
import AppKit
import StreamBindings
import SwiftUI

/// The songs picked in a Mac list: click plays one without picking it, ⇧-click picks a run from
/// the song last clicked, ⌘-click adds or takes one away, ⌘A picks them all. Rows are keyed by their place
/// in the page as the core gave it (a playlist can hold a song twice).
@MainActor
@Observable
final class SongSelection {
    var keys: Set<Int> = []
    private var anchor: Int?

    /// Picks for a click on `key`; true when the click should play it (no ⇧ or ⌘ held).
    func click(_ key: Int, in order: [Int]) -> Bool {
        let flags = NSEvent.modifierFlags
        if flags.contains(.shift) {
            if let a = anchor, let from = order.firstIndex(of: a), let to = order.firstIndex(of: key) {
                keys = Set(order[min(from, to)...max(from, to)])
            } else {
                keys = [key]
                anchor = key
            }
            return false
        } else if flags.contains(.command) {
            if keys.contains(key) { keys.remove(key) } else { keys.insert(key) }
            anchor = key
            return false
        } else {
            keys = []
            anchor = key
            return true
        }
    }

    func clear() {
        keys = []
        anchor = nil
    }
}

extension AppModel {
    /// Songs copied in Stream, with the pasteboard's change count when they were: pasting
    /// takes them only while the pasteboard still holds that copy.
    @MainActor private static var copied: (count: Int, tracks: [Track])?

    func copy(_ tracks: [Track]) {
        let board = NSPasteboard.general
        board.clearContents()
        // Elsewhere, songs paste as text: Spotify's links, or the title and artist.
        board.setString(tracks.map { t in
            t.id.hasPrefix("spotify:track:") ? "https://open.spotify.com/track/\(t.id.dropFirst("spotify:track:".count))" : "\(t.title) — \(t.artist)"
        }.joined(separator: "\n"), forType: .string)
        Self.copied = (board.changeCount, tracks)
    }

    var pasteable: [Track]? {
        guard let c = Self.copied, c.count == NSPasteboard.general.changeCount else { return nil }
        return c.tracks
    }
}

/// A list of songs that can be picked, copied, cut and pasted: ⌘A, ⌘C, ⌘X, ⌘V and Delete.
/// `remove` and `paste` are for your own playlists; elsewhere cut copies and paste says where it can go.
struct SongClipboard: ViewModifier {
    let selection: SongSelection
    /// Every row's key, in the order shown.
    let order: [Int]
    let tracks: ([Int]) -> [Track]
    var remove: (([Int]) -> Void)?
    var paste: (([Track]) -> Void)?
    @Environment(AppModel.self) private var model
    @FocusState private var focused: Bool

    func body(content: Content) -> some View {
        content
            .focusable()
            .focusEffectDisabled()
            .focused($focused)
            .onCommand(#selector(NSResponder.selectAll(_:))) {
                selection.keys = Set(order)
            }
            .onCommand(#selector(NSText.copy(_:))) { copy() }
            .onCommand(#selector(NSText.cut(_:))) {
                copy()
                guard let remove, !selection.keys.isEmpty else { return }
                remove(picked)
                selection.clear()
            }
            .onCommand(#selector(NSText.paste(_:))) {
                guard let paste else { return model.show("Paste songs into one of your playlists") }
                guard let songs = model.pasteable, !songs.isEmpty else { return model.show("Copy songs in Stream to paste them here") }
                paste(songs)
                model.show(songs.count == 1 ? "Added “\(songs[0].title)”" : "Added \(songs.count) songs")
            }
            .onDeleteCommand {
                guard let remove, !selection.keys.isEmpty else { return }
                remove(picked)
                selection.clear()
            }
            .onExitCommand { selection.clear() }
            .onChange(of: selection.keys) { if !selection.keys.isEmpty { focused = true } }
            // Your own playlist takes ⌘V as soon as it's open, even before it has a song to click.
            .frame(minHeight: paste != nil && order.isEmpty ? 120 : nil, alignment: .top)
            .onAppear { if paste != nil { focused = true } }
    }

    /// The picked rows in the order shown.
    private var picked: [Int] { order.filter(selection.keys.contains) }

    private func copy() {
        guard !selection.keys.isEmpty else { return }
        let songs = tracks(picked)
        model.copy(songs)
        model.show(songs.count == 1 ? "Copied “\(songs[0].title)”" : "Copied \(songs.count) songs")
    }
}

extension View {
    /// One row: a click plays it, ⇧- and ⌘-click only pick; tinted while picked.
    func pickable(_ key: Int, in selection: SongSelection, order: [Int], play: @escaping () -> Void) -> some View {
        self
            .background {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(selection.keys.contains(key) ? AnyShapeStyle(.tint.opacity(0.28)) : AnyShapeStyle(.clear))
                    .padding(.horizontal, 8)
            }
            .contentShape(Rectangle())
            .onTapGesture { if selection.click(key, in: order) { play() } }
    }

    func songClipboard(_ selection: SongSelection, order: [Int], tracks: @escaping ([Int]) -> [Track],
                       remove: (([Int]) -> Void)? = nil, paste: (([Track]) -> Void)? = nil) -> some View {
        modifier(SongClipboard(selection: selection, order: order, tracks: tracks, remove: remove, paste: paste))
    }
}
#endif
