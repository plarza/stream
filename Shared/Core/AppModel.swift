import StreamBindings
import Foundation
import Observation
import SwiftUI

/// The app's single source of truth on the Swift side. Everything durable
/// lives in the Rust core; this mirrors what the UI needs and forwards actions.
@MainActor
@Observable
final class AppModel {
    static let shared = AppModel()

    let core: StreamCore
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
    /// iPhone's Settings sheet (the account button, and prompts to connect Spotify).
    var showSettings = false
    var nowPlayingMode: NowPlayingMode = .artwork
    /// Colors derived from the current artwork (backgrounds, Now Playing).
    var accent: Palette = .fallback
    /// Output volume, 0…1 (always 0 under UI tests).
    var volume: Double = AppModel.isTesting ? 0 : (UserDefaults.standard.object(forKey: "volume") as? Double ?? 1) {
        didSet {
            core.setVolume(volume: Float(volume))
            if !AppModel.isTesting { UserDefaults.standard.set(volume, forKey: "volume") }
        }
    }

    /// Launched by the UI tests (`-uiTesting YES`): isolated library, silent, nothing persisted.
    /// Arguments go in `-key value` pairs; AppKit misreads a bare flag followed by a pair.
    static let isTesting = ProcessInfo.processInfo.arguments.contains("-uiTesting")
    static func argument(_ name: String) -> String? {
        let a = ProcessInfo.processInfo.arguments
        guard let i = a.firstIndex(of: name), i + 1 < a.count else { return nil }
        return a[i + 1]
    }
    /// The app's accent everywhere.
    /// Text and icons drawn on the accent.
    let onTint = Color.white
    /// A page to open after Now Playing closes (Go to Album / Artist).
    var pendingNavigation: Item?
    /// The album, artist or playlist page on screen, so opening it again doesn't stack a copy.
    @ObservationIgnored var currentPage: String?
    var handoff: NSUserActivity?

    enum NowPlayingMode: Hashable { case artwork, lyrics, queue }

    private init() {
        let fm = FileManager.default
        var support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("network.aza.stream", isDirectory: true)
        var caches = fm.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("network.aza.stream", isDirectory: true)
        if Self.isTesting {
            let root = fm.temporaryDirectory.appendingPathComponent("stream-uitest-\(UUID().uuidString)", isDirectory: true)
            support = root.appendingPathComponent("data")
            caches = root.appendingPathComponent("cache")
        }
        #if os(iOS)
        var musicDir: URL? = fm.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("Music", isDirectory: true)
        if Self.isTesting, let m = Self.argument("-musicFolder") { musicDir = URL(fileURLWithPath: m) }
        try? fm.createDirectory(at: musicDir!, withIntermediateDirectories: true)
        let isPhone = true
        #else
        let musicDir: URL? = nil
        let isPhone = false
        #endif
        let bridge = CoreBridge()
        self.bridge = bridge
        // UI tests stand in for Spotify's accounts service and Web API (see tests/Shared/FakeSpotify.swift).
        let config = CoreConfig(dataDir: support.path, cacheDir: caches.path, musicDir: musicDir?.path, isPhone: isPhone,
                                spotifyAccountsUrl: Self.isTesting ? Self.argument("-spotifyAccounts") : nil,
                                spotifyApiUrl: Self.isTesting ? Self.argument("-spotifyApi") : nil)
        // A failure here means the disk is unusable; nothing sensible to show.
        core = try! StreamCore(config: config, delegate: bridge)
        player = core.playerState()
        account = core.account()
        home = core.home()
        bridge.model = self
        core.setVolume(volume: Float(volume))
        #if os(macOS)
        if Self.isTesting, let m = Self.argument("-musicFolder") {
            core.setMusicFolders(folders: [m])
        }
        #endif
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
        sections = [:]
        tracks = [:]
        libraryVersion &+= 1
    }

    // MARK: Page caches (so switching tabs and pages shows what was there at once)

    /// Album, playlist and artist pages already loaded this session.
    @ObservationIgnored var pages: [String: CollectionPage] = [:]
    /// The first page of each library section, for the current library version.
    @ObservationIgnored private var sections: [LibrarySection: [Item]] = [:]

    /// Songs looked up by id (rows ask for lengths and albums), for this library version.
    @ObservationIgnored private var tracks: [String: Track?] = [:]

    func track(_ id: String) -> Track? {
        if let hit = tracks[id] { return hit }
        let t = core.track(id: id)
        tracks[id] = t
        return t
    }

    /// Pins are on Home already; no need to ask the core per row.
    func isPinned(_ id: String) -> Bool { home.pins.contains { $0.id == id } }

    /// A page to open with: this session's copy, else whatever the core has on disk.
    func cachedPage(_ id: String) -> CollectionPage? {
        if let hit = pages[id] { return hit }
        let page = core.cachedCollection(id: id)
        pages[id] = page
        return page
    }

    func sectionFirstPage(_ section: LibrarySection, limit: UInt32) -> [Item] {
        if let hit = sections[section] { return hit }
        let items = core.section(section: section, offset: 0, limit: limit)
        sections[section] = items
        return items
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

    /// Buttons answer at once; the core (and Spotify) catch up, and a failure puts it back.
    func setLiked(_ track: Track, _ liked: Bool) {
        Haptics.success()
        if player.track?.id == track.id { player.track?.liked = liked }
        Task {
            do { try await core.setLiked(trackId: track.id, liked: liked) } catch {
                if player.track?.id == track.id { player.track?.liked = !liked }
                show(Self.message(error))
            }
        }
    }

    func setShuffle(_ on: Bool) {
        Haptics.tap()
        player.shuffle = on
        core.setShuffle(on: on)
    }

    /// Repeat is one thing here: this song, forever.
    func setRepeat(_ on: Bool) {
        Haptics.tap()
        player.repeat = on ? .one : .off
        core.setRepeat(mode: on ? .one : .off)
    }

    /// Opens a page from anywhere, closing Now Playing first if it's up.
    func open(_ item: Item) {
        pendingNavigation = item
        #if os(iOS)
        // iPhone pushes the page straight away, under Now Playing as it closes.
        openRequested &+= 1
        #else
        if showNowPlaying {
            withAnimation(.player) { showNowPlaying = false }
        } else {
            openRequested &+= 1
        }
        #endif
    }

    func openArtist(of track: Track) {
        guard let id = track.artistId else { return }
        open(Item(id: id, kind: .artist, source: track.source, title: track.artist, subtitle: "Artist", art: nil, downloaded: false))
    }

    func openAlbum(of track: Track) {
        guard let id = track.albumId else { return }
        open(Item(id: id, kind: .album, source: track.source, title: track.album, subtitle: track.artist, art: track.art, downloaded: false))
    }

    // MARK: Appearance

    /// Pure black instead of dark grey in dark mode, for OLED screens.
    var pureBlack: Bool = UserDefaults.standard.bool(forKey: "pureBlack") {
        didSet { UserDefaults.standard.set(pureBlack, forKey: "pureBlack") }
    }

    func togglePin(_ item: Item) {
        Haptics.success()
        if isPinned(item.id) { core.unpin(id: item.id) } else { core.pin(item: item) }
    }

    func playNext(_ tracks: [Track]) { Haptics.tap(); core.playNext(tracks: tracks); show("Playing next") }
    func playLater(_ tracks: [Track]) { Haptics.tap(); core.playLater(tracks: tracks); show("Added to queue") }

    /// Current position, cheap enough to call every frame.
    func position() -> Double { Double(core.positionMs()) / 1000 }

    /// Pull to refresh: fetches the Spotify library now (no-op when signed out).
    func refreshSpotify() async {
        guard account.spotifyWeb else { return }
        do { try await core.refreshSpotify(force: true) } catch { show(Self.message(error)) }
    }

    /// True while a Spotify sign-in is under way (the core reports each step).
    var signingIn: Bool {
        switch account.signIn {
        case .approving, .connecting: true
        case .idle, .failed: false
        }
    }

    /// Signs in to Spotify: the core does whichever steps are still missing and
    /// reports them in `account.signIn`, failures included, so there's nothing to catch.
    func signInToSpotify() {
        guard !signingIn else { return }
        Task { try? await core.spotifySignIn() }
    }

    /// The core's own message rather than Swift's generic description of the error enum.
    static func message(_ error: Error) -> String {
        if case let CoreError.Failed(message) = error { return message }
        return error.localizedDescription
    }

    /// network-aza-stream://play?id=…[&shuffle=1] · open?id=… · nowplaying?mode=lyrics|queue
    func handle(url: URL) {
        guard url.scheme == "network-aza-stream" else { return }
        let q = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        let value = { (name: String) in q.first { $0.name == name }?.value }
        switch url.host() {
        case "reset" where Self.isTesting:
            // UI tests share one launch; this puts the app back on Home, silent and with nothing open.
            core.pause()
            core.spotifySignOut()
            showNowPlaying = false
            resetRequested &+= 1
        case "play":
            guard let id = value("id") else { return }
            let item = Item(id: id, kind: Self.kind(of: id), source: id.hasPrefix("local:") ? .local : .spotify, title: "", subtitle: "", art: nil, downloaded: false)
            play(item, shuffle: value("shuffle") == "1")
        case "open":
            guard let id = value("id") else { return }
            pendingNavigation = Item(id: id, kind: Self.kind(of: id), source: id.hasPrefix("local:") ? .local : .spotify, title: "", subtitle: "", art: nil, downloaded: false)
            openRequested &+= 1
        case "pin":
            guard let id = value("id") else { return }
            Task {
                if let t = core.track(id: id) { core.pin(item: t.item) }
                else if let page = try? await core.collection(id: id) { core.pin(item: page.item) }
            }
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

    /// Bumped when UI tests reset the app between tests.
    var resetRequested = 0

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
        let activity = handoff ?? NSUserActivity(activityType: "network.aza.stream.playing")
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
    private let keychain = Keychain(service: "network.aza.stream")
    private let testing = ProcessInfo.processInfo.arguments.contains("-uiTesting")
    private var memory: [String: String] = [:]
    private let lock = NSLock()

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
            WebAuth.shared.open(u) { self.model?.core.cancelSignIn() }
        }
    }

    func secretGet(key: String) -> String? {
        guard testing else { return keychain.get(key) }
        return lock.withLock { memory[key] }
    }

    func secretSet(key: String, value: String?) {
        guard testing else { return keychain.set(key, value) }
        lock.withLock { memory[key] = value }
    }

    func message(text: String) {
        Task { @MainActor in self.model?.show(text) }
    }
}
