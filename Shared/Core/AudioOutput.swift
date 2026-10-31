import AVFoundation
#if canImport(stream_coreFFI)
import stream_coreFFI
#endif

/// The device end of the audio path. An AVAudioSourceNode pulls PCM straight
/// out of the Rust core's ring buffer on the render thread. The engine is
/// stopped whenever nothing plays, so a paused player costs no CPU.
final class AudioOutput: @unchecked Sendable {
    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private var stopWork: DispatchWorkItem?
    var onRateChange: ((UInt32) -> Void)?

    func start() {
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playback, mode: .default, policy: .longFormAudio)
        NotificationCenter.default.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { note in
            guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let type = AVAudioSession.InterruptionType(rawValue: raw) else { return }
            MainActor.assumeIsolated {
                if type == .began { AppModel.shared.core.pause() }
                else if let o = note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt,
                        AVAudioSession.InterruptionOptions(rawValue: o).contains(.shouldResume) {
                    AppModel.shared.core.resume()
                }
            }
        }
        NotificationCenter.default.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { note in
            guard let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
                  AVAudioSession.RouteChangeReason(rawValue: raw) == .oldDeviceUnavailable else { return }
            // Headphones unplugged: pause, like every music app.
            MainActor.assumeIsolated { AppModel.shared.core.pause() }
        }
        #endif
        NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            self?.rebuild()
        }
        rebuild()
    }

    private func rebuild() {
        let wasRunning = engine.isRunning
        engine.stop()
        if let s = source { engine.detach(s) }
        let rate = engine.outputNode.outputFormat(forBus: 0).sampleRate
        let sampleRate = rate > 0 ? rate : 48_000
        let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2)!
        let node = AVAudioSourceNode(format: format) { _, _, frameCount, buffers -> OSStatus in
            let abl = UnsafeMutableAudioBufferListPointer(buffers)
            guard abl.count >= 2,
                  let l = abl[0].mData?.assumingMemoryBound(to: Float.self),
                  let r = abl[1].mData?.assumingMemoryBound(to: Float.self) else { return noErr }
            _ = stream_core_render(l, r, frameCount)
            return noErr
        }
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: format)
        source = node
        onRateChange?(UInt32(sampleRate))
        if wasRunning { run() }
    }

    private func run() {
        guard !engine.isRunning else { return }
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(true)
        #endif
        engine.prepare()
        try? engine.start()
    }

    /// Keeps the engine running while playing; stops it shortly after pausing
    /// (once the fade-out has been rendered).
    func setPlaying(_ playing: Bool) {
        stopWork?.cancel()
        if playing {
            run()
        } else {
            let work = DispatchWorkItem { [weak self] in self?.engine.pause() }
            stopWork = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: work)
        }
    }
}
