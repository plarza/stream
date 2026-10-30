import MediaPlayer

#if os(iOS)
import UIKit
typealias PlatformImage = UIImage
#else
import AppKit
typealias PlatformImage = NSImage
#endif

/// Lock screen, Control Center, media keys, AirPlay and headphone buttons.
@MainActor
final class NowPlayingCenter {
    private weak var model: AppModel?
    private var artworkFor: String?

    func attach(model: AppModel) {
        self.model = model
        let c = MPRemoteCommandCenter.shared()
        c.playCommand.addTarget { _ in model.core.resume(); return .success }
        c.pauseCommand.addTarget { _ in model.core.pause(); return .success }
        c.togglePlayPauseCommand.addTarget { _ in model.core.toggle(); return .success }
        c.nextTrackCommand.addTarget { _ in model.core.next(); return .success }
        c.previousTrackCommand.addTarget { _ in model.core.previous(); return .success }
        c.changePlaybackPositionCommand.addTarget { event in
            guard let e = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            model.core.seek(ms: UInt32(max(0, e.positionTime) * 1000))
            return .success
        }
    }

    func update(state: PlayerState) {
        let center = MPNowPlayingInfoCenter.default()
        guard let t = state.track else {
            center.nowPlayingInfo = nil
            center.playbackState = .stopped
            return
        }
        var info = center.nowPlayingInfo ?? [:]
        info[MPMediaItemPropertyTitle] = t.title
        info[MPMediaItemPropertyArtist] = t.artist
        info[MPMediaItemPropertyAlbumTitle] = t.album
        info[MPMediaItemPropertyPlaybackDuration] = Double(t.durationMs) / 1000
        info[MPNowPlayingInfoPropertyElapsedPlaybackTime] = Double(state.positionMs) / 1000
        info[MPNowPlayingInfoPropertyPlaybackRate] = state.status == .playing ? 1.0 : 0.0
        if artworkFor != t.id {
            info[MPMediaItemPropertyArtwork] = nil
            artworkFor = t.id
            let id = t.id
            Task {
                guard let image = await ArtworkStore.shared.image(for: t.art, pixels: 600) else { return }
                guard self.artworkFor == id else { return }
                let art = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
                var i = MPNowPlayingInfoCenter.default().nowPlayingInfo ?? [:]
                i[MPMediaItemPropertyArtwork] = art
                MPNowPlayingInfoCenter.default().nowPlayingInfo = i
            }
        }
        center.nowPlayingInfo = info
        center.playbackState = state.status == .playing ? .playing : .paused
    }
}
