import StreamBindings
import AVFoundation
import Synchronization
#if canImport(stream_coreFFI)
import stream_coreFFI
#endif

/// The device end of the audio path. An AVAudioSourceNode pulls PCM straight
/// out of the Rust core's ring buffer on the render thread. The engine is
/// stopped whenever nothing plays, so a paused player costs no CPU.
/// Nonisolated: the render block runs on the audio thread, never the main actor.
nonisolated final class AudioOutput: @unchecked Sendable {
    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private var stopWork: DispatchWorkItem?
    var onRateChange: ((UInt32) -> Void)?
    /// Loudness of what's playing in three bands, for the playing-song bars.
    let meter = LevelMeter()

    func start() {
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playback, mode: .default, policy: .longFormAudio)
        NotificationCenter.default.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { note in
            guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let type = AVAudioSession.InterruptionType(rawValue: raw) else { return }
            let resume = (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt)
                .map { AVAudioSession.InterruptionOptions(rawValue: $0).contains(.shouldResume) } ?? false
            MainActor.assumeIsolated {
                if type == .began { AppModel.shared.core.pause() } else if resume { AppModel.shared.core.resume() }
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
        let meter = meter
        meter.sampleRate = Float(sampleRate)
        let node = AVAudioSourceNode(format: format) { _, _, frameCount, buffers -> OSStatus in
            let abl = UnsafeMutableAudioBufferListPointer(buffers)
            guard abl.count >= 2,
                  let l = abl[0].mData?.assumingMemoryBound(to: Float.self),
                  let r = abl[1].mData?.assumingMemoryBound(to: Float.self) else { return noErr }
            _ = stream_core_render(l, r, frameCount)
            meter.measure(l, r, Int(frameCount))
            if meter.muted {
                memset(l, 0, Int(frameCount) * 4)
                memset(r, 0, Int(frameCount) * 4)
            }
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

/// Bass, mids and highs of the audio as it's rendered, each 0…1. The render
/// thread splits the signal with one-pole filters and publishes each band's
/// level per buffer; the UI reads them every frame without locking.
nonisolated final class LevelMeter: @unchecked Sendable {
    var sampleRate: Float = 48_000
    /// The output volume, divided back out so the bars don't shrink when it's turned down.
    var volume: Float = 1
    private let bass = Atomic<UInt32>(0), mids = Atomic<UInt32>(0), highs = Atomic<UInt32>(0)
    // Filter state, touched only on the render thread.
    private var low: Float = 0, mid: Float = 0
    /// Measures but plays nothing (`-muteOutput YES`, for recording the visualizers).
    var muted = false
    /// The last few thousand mono samples, for the Now Playing visualizer's spectrum.
    static let historySize = 4096
    private let history: UnsafeMutablePointer<Float> = {
        let p = UnsafeMutablePointer<Float>.allocate(capacity: LevelMeter.historySize)
        p.initialize(repeating: 0, count: LevelMeter.historySize)
        return p
    }()
    private let written = Atomic<Int>(0)

    func measure(_ l: UnsafePointer<Float>, _ r: UnsafePointer<Float>, _ n: Int) {
        guard n > 0 else { return }
        let a1 = 1 - exp(-2 * Float.pi * 250 / sampleRate)
        let a2 = 1 - exp(-2 * Float.pi * 2500 / sampleRate)
        var e = (Float(0), Float(0), Float(0))
        let w = written.load(ordering: .relaxed)
        let mask = Self.historySize - 1
        for i in 0..<n {
            let x = (l[i] + r[i]) * 0.5
            history[(w + i) & mask] = x
            low += a1 * (x - low)
            mid += a2 * (x - mid)
            let b = (low, mid - low, x - mid)
            e.0 += b.0 * b.0; e.1 += b.1 * b.1; e.2 += b.2 * b.2
        }
        written.store(w + n, ordering: .releasing)
        let count = Float(n) * max(volume * volume, 1e-4)
        bass.store(Self.level(e.0 / count).bitPattern, ordering: .relaxed)
        mids.store(Self.level(e.1 / count).bitPattern, ordering: .relaxed)
        highs.store(Self.level(e.2 / count).bitPattern, ordering: .relaxed)
    }

    /// Mean square to 0…1 on a decibel scale: -54 dB is silent, -6 dB is full.
    private static func level(_ meanSquare: Float) -> Float {
        let db = 10 * log10(max(meanSquare, 1e-12))
        return min(max((db + 54) / 48, 0), 1)
    }

    /// Copies the latest `count` samples (oldest first), scaled back up by the volume.
    /// Reads race the render thread by design: a torn sample only ever shows as a flicker.
    func recent(into out: UnsafeMutablePointer<Float>, count: Int) {
        let end = written.load(ordering: .acquiring)
        let mask = Self.historySize - 1
        let gain = 1 / max(volume, 0.01)
        for i in 0..<count { out[i] = history[(end - count + i) & mask] * gain }
    }

    var levels: (Float, Float, Float) {
        (Float(bitPattern: bass.load(ordering: .relaxed)),
         Float(bitPattern: mids.load(ordering: .relaxed)),
         Float(bitPattern: highs.load(ordering: .relaxed)))
    }
}
