import Foundation
import Observation
import SwiftUI

/// The app's single source of truth on the Swift side. Everything durable
/// lives in the Rust core; this mirrors what the UI needs and forwards actions.
@MainActor
@Observable
final class AppModel {
    static let shared = AppModel()

    let core: MusicCore
    private let bridge: CoreBridge
    let audio = AudioOutput()
    let nowPlaying = NowPlayingCenter()

    var player: PlayerState
    var account: AccountState
    var home: Home = Home(pins: [], recents: [])
    /// Bumped whenever the library changes; views that page the library observe it.
    var libraryVersion = 0
    var toast: String?
    var showNowPlaying = false
    var nowPlayingMode: NowPlayingMode = .artwork
    /// Accent derived from the current artwork.
    var accent: Palette = .fallback
    var signInURL: URL?
    /// A page to open after Now Playing closes (Go to Album / Artist).
    var pendingNavigation: Item?
    var handoff: NSUserActivity?

    enum NowPlayingMode: Hashable { case artwork, lyrics, queue }

    private init() {
        let fm = FileManager.default
        let support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("network.aza.music", isDirectory: true)
        let caches = fm.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("network.aza.music", isDirectory: true)
        #if os(iOS)
        let musicDir: URL? = fm.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("Music", isDirectory: true)
        try? fm.createDirectory(at: musicDir!, withIntermediateDirectories: true)
        let isPhone = true
        #else
        let musicDir: URL? = nil
        let isPhone = false
        #endif
        let bridge = CoreBridge()
        self.bridge = bridge
        let config = CoreConfig(dataDir: support.path, cacheDir: caches.path, musicDir: musicDir?.path, isPhone: isPhone)
        // A failure here means the disk is unusable; nothing sensible to show.
        core = try! MusicCore(config: config, delegate: bridge)
        player = core.playerState()
        account = core.account()
        home = core.home()
        bridge.model = self
        audio.onRateChange = { [weak self] rate in self?.core.setOutputRate(rate: rate) }
        audio.start()
        nowPlaying.attach(model: self)
        handleLaunchArguments()
    }

    // MARK: Delegate events (main actor)

    func playerChanged(_ state: PlayerState) {
        let trackChanged = state.track?.id != player.track?.id
        withAnimation(.smooth(duration: 0.35)) { player = state }
        audio.setPlaying(state.status == .playing)
        nowPlaying.update(state: state)
        if trackChanged { trackDidChange(state.track) }
        if let h = handoff {
            h.userInfo = handoffInfo()
            h.needsSave = true
        }
    }

    func libraryChanged() {
        home = core.home()
        libraryVersion &+= 1
    }

    func accountChanged(_ state: AccountState) {
        account = state
    }

    func show(_ message: String) {
        withAnimation(.snappy) { toast = message }
        Task {
            try? await Task.sleep(for: .seconds(3.5))
            if toast == message { withAnimation(.snappy) { toast = nil } }
        }
    }

    private func trackDidChange(_ track: Track?) {
        Task {
            let palette = await Palette.extract(from: track?.art)
            withAnimation(.smooth(duration: 0.8)) { accent = palette }
        }
        updateHandoff(track)
    }

    // MARK: Actions

    func play(_ tracks: [Track], at index: Int = 0, from context: Item? = nil, shuffle: Bool = false) {
        Haptics.tap()
        core.play(tracks: tracks, start: UInt32(index), context: context, shuffle: shuffle)
        home = core.home()
    }

    func play(_ item: Item, shuffle: Bool = false) {
        Haptics.tap()
        Task {
            do { try await core.playItem(item: item, shuffle: shuffle) } catch { show(error.localizedDescription) }
            home = core.home()
        }
    }

    func toggle() { Haptics.tap(); core.toggle() }
    func next() { Haptics.tap(); core.next() }
    func previous() { Haptics.tap(); core.previous() }
    func seek(to ms: UInt32) { core.seek(ms: ms) }

    func setLiked(_ track: Track, _ liked: Bool) {
        Haptics.success()
        Task {
            do { try await core.setLiked(trackId: track.id, liked: liked) } catch { show(error.localizedDescription) }
        }
    }

    func togglePin(_ item: Item) {
        Haptics.success()
        if core.isPinned(id: item.id) { core.unpin(id: item.id) } else { core.pin(item: item) }
    }

    func playNext(_ tracks: [Track]) { Haptics.tap(); core.playNext(tracks: tracks); show("Playing next") }
    func playLater(_ tracks: [Track]) { Haptics.tap(); core.playLater(tracks: tracks); show("Added to queue") }

    /// Current position, cheap enough to call every frame.
    func position() -> Double { Double(core.positionMs()) / 1000 }

    func signInToSpotify() {
        Task {
            do { try await core.spotifySignIn() } catch { show(error.localizedDescription) }
            signInURL = nil
        }
    }

    /// network-aza-music://play?id=…[&shuffle=1] · open?id=… · nowplaying?mode=lyrics|queue
    func handle(url: URL) {
        guard url.scheme == "network-aza-music" else { return }
        let q = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        let value = { (name: String) in q.first { $0.name == name }?.value }
        switch url.host() {
        case "play":
            guard let id = value("id") else { return }
            let item = Item(id: id, kind: Self.kind(of: id), source: id.hasPrefix("local:") ? .local : .spotify, title: "", subtitle: "", art: nil, downloaded: false)
            play(item, shuffle: value("shuffle") == "1")
        case "open":
            guard let id = value("id") else { return }
            pendingNavigation = Item(id: id, kind: Self.kind(of: id), source: id.hasPrefix("local:") ? .local : .spotify, title: "", subtitle: "", art: nil, downloaded: false)
            openRequested &+= 1
        case "nowplaying":
            nowPlayingMode = switch value("mode") { case "lyrics": .lyrics; case "queue": .queue; default: .artwork }
            showNowPlaying = true
        default:
            break
        }
    }

    /// `-url <deep link>` launch arguments, run in order (handy from scripts and Xcode schemes).
    func handleLaunchArguments() {
        let args = ProcessInfo.processInfo.arguments
        let urls = args.indices.filter { args[$0] == "-url" && $0 + 1 < args.count }.compactMap { URL(string: args[$0 + 1]) }
        guard !urls.isEmpty else { return }
        Task {
            for u in urls {
                try? await Task.sleep(for: .seconds(1.5))
                handle(url: u)
            }
        }
    }

    /// Bumped when a deep link asks to open a page.
    var openRequested = 0

    static func kind(of id: String) -> ItemKind {
        if id.contains(":album:") { return .album }
        if id.contains(":artist:") { return .artist }
        if id.contains(":playlist:") { return .playlist }
        return .track
    }

    // MARK: Handoff (Spotify songs only)

    private func updateHandoff(_ track: Track?) {
        guard let track, track.source == .spotify else {
            handoff?.invalidate()
            handoff = nil
            return
        }
        let activity = handoff ?? NSUserActivity(activityType: "network.aza.music.playing")
        activity.title = "\(track.title) — \(track.artist)"
        activity.isEligibleForHandoff = true
        activity.needsSave = true
        activity.userInfo = handoffInfo()
        activity.becomeCurrent()
        handoff = activity
    }

    func handoffInfo() -> [String: Any] {
        var info: [String: Any] = ["position": core.positionMs()]
        if let t = player.track { info["track"] = t.id }
        if let c = core.queue().contextTitle { info["contextTitle"] = c }
        return info
    }

    func continueActivity(_ activity: NSUserActivity) {
        guard let track = activity.userInfo?["track"] as? String else { return }
        let position = (activity.userInfo?["position"] as? NSNumber)?.uint32Value ?? 0
        Task {
            do { try await core.resumeHandoff(trackId: track, positionMs: position, contextId: nil) } catch { show(error.localizedDescription) }
        }
    }
}

/// Receives core callbacks on Rust threads and hops to the main actor.
final class CoreBridge: CoreDelegate, @unchecked Sendable {
    weak var model: AppModel?
    private let keychain = Keychain(service: "network.aza.music")

    func playerChanged(state: PlayerState) {
        Task { @MainActor in self.model?.playerChanged(state) }
    }

    func libraryChanged() {
        Task { @MainActor in self.model?.libraryChanged() }
    }

    func accountChanged(state: AccountState) {
        Task { @MainActor in self.model?.accountChanged(state) }
    }

    func openUrl(url: String) {
        guard let u = URL(string: url) else { return }
        Task { @MainActor in
            #if os(iOS)
            self.model?.signInURL = u
            #else
            NSWorkspace.shared.open(u)
            #endif
        }
    }

    func secretGet(key: String) -> String? { keychain.get(key) }

    func secretSet(key: String, value: String?) { keychain.set(key, value) }

    func message(text: String) {
        Task { @MainActor in self.model?.show(text) }
    }
}
