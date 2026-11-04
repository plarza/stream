import Accelerate
import simd
import SwiftUI

// MARK: Analysis

/// What the visualizers draw from, once per frame: the spectrum in log-spaced
/// bands, the three meter bands, overall energy and beats.
struct AudioFrame {
    static let bands = 48
    var spectrum = [Float](repeating: 0, count: AudioFrame.bands)
    var bass: Float = 0, mids: Float = 0, highs: Float = 0
    /// Loudness against the song's own recent peak, 0…1.
    var energy: Float = 0
    /// Jumps to 1 on a beat and decays over a few hundred ms.
    var kick: Float = 0
    /// Seconds since the visualizer started.
    var time: Float = 0
    /// Like time, but runs faster when the music is louder.
    var flow: Float = 0
    var dt: Float = 0
}

/// Spectrum and beats from the meter's recent samples. Shared, and computed at
/// most once per frame however many layers ask.
@MainActor
final class AudioAnalysis {
    static let shared = AudioAnalysis()

    private let n = 2048
    private let log2n: vDSP_Length = 11
    private let setup: FFTSetup
    private var samples: [Float]
    private let window: [Float]
    private var re: [Float], im: [Float], power: [Float]
    private var frame = AudioFrame()
    private var last: Date?
    private var start = Date()
    private var peak: Float = -30
    private var ceiling: Float = 0.4
    private var bassSlow: Float = 0

    private init() {
        setup = vDSP_create_fftsetup(log2n, FFTRadix(kFFTRadix2))!
        samples = .init(repeating: 0, count: n)
        window = vDSP.window(ofType: Float.self, usingSequence: .hanningDenormalized, count: n, isHalfWindow: false)
        re = .init(repeating: 0, count: n / 2)
        im = .init(repeating: 0, count: n / 2)
        power = .init(repeating: 0, count: n / 2)
    }

    func update(at date: Date, playing: Bool) -> AudioFrame {
        if let last, date == last { return frame }
        let dt = Float(min(max(date.timeIntervalSince(last ?? date), 0), 0.05))
        last = date
        frame.dt = dt
        frame.time = Float(date.timeIntervalSince(start))

        let meter = AppModel.shared.audio.meter
        var targets = [Float](repeating: 0, count: AudioFrame.bands)
        var levels: (Float, Float, Float) = (0, 0, 0)
        if playing {
            levels = meter.levels
            samples.withUnsafeMutableBufferPointer { meter.recent(into: $0.baseAddress!, count: n) }
            vDSP.multiply(samples, window, result: &samples)
            re.withUnsafeMutableBufferPointer { rp in
                im.withUnsafeMutableBufferPointer { ip in
                    var split = DSPSplitComplex(realp: rp.baseAddress!, imagp: ip.baseAddress!)
                    samples.withUnsafeBufferPointer { sp in
                        sp.baseAddress!.withMemoryRebound(to: DSPComplex.self, capacity: n / 2) {
                            vDSP_ctoz($0, 2, &split, 1, vDSP_Length(n / 2))
                        }
                    }
                    vDSP_fft_zrip(setup, &split, 1, log2n, FFTDirection(FFT_FORWARD))
                    power.withUnsafeMutableBufferPointer { vDSP_zvmags(&split, 1, $0.baseAddress!, 1, vDSP_Length(n / 2)) }
                }
            }
            let rate = meter.sampleRate
            var loudest: Float = -120
            var db = [Float](repeating: -120, count: AudioFrame.bands)
            for b in 0..<AudioFrame.bands {
                // 35 Hz to 16 kHz, evenly spaced in pitch.
                let lo = 35 * pow(16000 / 35, Float(b) / Float(AudioFrame.bands))
                let hi = 35 * pow(16000 / 35, Float(b + 1) / Float(AudioFrame.bands))
                let i0 = max(1, Int(lo / rate * Float(n)))
                let i1 = max(i0 + 1, Int(hi / rate * Float(n)))
                var sum: Float = 0
                for i in i0..<min(i1, n / 2) { sum += power[i] }
                let mean = sum / Float(i1 - i0)
                // Tilted +3 dB an octave so the highs aren't buried under the bass.
                let tilt = 3 * log2(max(lo, 35) / 35)
                db[b] = 10 * log10(mean + 1e-9) + tilt
                loudest = max(loudest, db[b])
            }
            // A running peak so quiet and loud masters both fill the range.
            peak = max(loudest, peak - 6 * dt)
            for b in 0..<AudioFrame.bands {
                targets[b] = min(max((db[b] - (peak - 42)) / 42, 0), 1)
            }
        }
        for b in 0..<AudioFrame.bands {
            let t = targets[b], s = frame.spectrum[b]
            frame.spectrum[b] = s + (t - s) * (1 - exp(-(t > s ? 28 : 7) * dt))
        }
        let ease = { (cur: Float, to: Float, up: Float, down: Float) in cur + (to - cur) * (1 - exp(-(to > cur ? up : down) * dt)) }
        frame.bass = ease(frame.bass, levels.0, 20, 6)
        frame.mids = ease(frame.mids, levels.1, 20, 6)
        frame.highs = ease(frame.highs, levels.2, 20, 6)
        let loud = (levels.0 + levels.1 + levels.2) / 3
        ceiling = max(loud, ceiling - 0.06 * dt, 0.25)
        frame.energy = ease(frame.energy, playing ? min(loud / ceiling, 1) : 0, 8, 2.5)
        bassSlow += (levels.0 - bassSlow) * (1 - exp(-4 * dt))
        frame.kick = max(frame.kick * exp(-6 * dt), min(max(levels.0 - bassSlow, 0) * 7, 1))
        frame.flow += dt * (0.35 + 1.4 * frame.energy)
        return frame
    }
}

// MARK: Cover colours

/// A 4×4 grid of the cover's colours, lifted to glow on dark glass.
struct CoverColors: Equatable {
    var grid: [SIMD3<Float>]

    static let fallback = CoverColors(grid: (0..<16).map { i in
        let t = Float(i) / 15
        return SIMD3(0.45 + 0.4 * t, 0.2 + 0.15 * t, 0.5 - 0.2 * t)
    })

    /// The most vivid few, most vivid first.
    var vivid: [SIMD3<Float>] {
        grid.sorted { Self.vividness($0) > Self.vividness($1) }
    }

    private static func vividness(_ c: SIMD3<Float>) -> Float {
        let mx = max(c.x, c.y, c.z), mn = min(c.x, c.y, c.z)
        return (mx - mn) + 0.3 * mx
    }

    @MainActor private static var cache: [String: CoverColors] = [:]

    @MainActor static func load(_ url: String?) async -> CoverColors {
        guard let url else { return .fallback }
        if let c = cache[url] { return c }
        guard let cg = await ArtworkStore.shared.cgImage(for: url, pixels: 128) else { return .fallback }
        let n = 4
        var px = [UInt8](repeating: 0, count: n * n * 4)
        guard let ctx = CGContext(data: &px, width: n, height: n, bitsPerComponent: 8, bytesPerRow: n * 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return .fallback }
        ctx.interpolationQuality = .high
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: n, height: n))
        var grid: [SIMD3<Float>] = []
        for i in 0..<(n * n) {
            var c = SIMD3(Float(px[i * 4]), Float(px[i * 4 + 1]), Float(px[i * 4 + 2])) / 255
            // More colour, and never so dark it's mud nor so bright the text drowns.
            let mx = max(c.x, c.y, c.z, 0.001)
            let grey = SIMD3<Float>(repeating: (c.x + c.y + c.z) / 3)
            c = grey + (c - grey) * 1.45
            c = c.clamped(lowerBound: .zero, upperBound: .one)
            let v = min(max(mx, 0.3), 0.78)
            c *= v / max(max(c.x, c.y, c.z), 0.001)
            grid.append(c)
        }
        let result = CoverColors(grid: grid)
        if cache.count > 100 { cache.removeAll() }
        cache[url] = result
        return result
    }
}

extension Color {
    init(_ c: SIMD3<Float>, gain: Float = 1) {
        self.init(red: Double(c.x * gain), green: Double(c.y * gain), blue: Double(c.z * gain))
    }
}

// MARK: The layer

/// Sits between the blurred cover and the controls in Now Playing.
/// Settings > Playback turns it off (`visualizer`, on by default).
struct NowPlayingVisualizer: View {
    let url: String?
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var colors = CoverColors.fallback

    private var playing: Bool { model.player.status == .playing }

    var body: some View {
        TimelineView(.animation(paused: !playing || reduceMotion)) { tl in
            AuroraLayer(f: AudioAnalysis.shared.update(at: tl.date, playing: playing), colors: colors)
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .opacity(playing ? 1 : 0.55)
        .animation(.smooth(duration: 0.8), value: playing)
        .task(id: url) {
            let c = await CoverColors.load(url)
            withAnimation(.smooth(duration: 0.8)) { colors = c }
        }
    }
}

// MARK: Aurora

/// The cover melted into a mesh that breathes with the song: its colours drift
/// on their own, each row brightens with its part of the spectrum (bass at the
/// bottom, air at the top), and beats push the mesh outward from the middle.
private struct AuroraLayer: View {
    let f: AudioFrame
    let colors: CoverColors

    var body: some View {
        ZStack {
            MeshGradient(width: 4, height: 4, points: points, colors: tints, smoothsColors: true)
            LinearGradient(colors: [.black.opacity(0.1), .black.opacity(0.05), .black.opacity(0.5)],
                           startPoint: .top, endPoint: .bottom)
        }
    }

    private var points: [SIMD2<Float>] {
        var points: [SIMD2<Float>] = []
        for j in 0..<4 {
            for i in 0..<4 {
                let k = Float(j * 4 + i)
                var p = SIMD2(Float(i) / 3, Float(j) / 3)
                let drift = SIMD2(sin(f.flow * 0.9 + k * 1.7), cos(f.flow * 0.7 + k * 2.3)) * 0.11
                let away = p - SIMD2(0.5, 0.42)
                let push = away / max(simd_length(away), 0.01) * 0.05 * f.kick
                // Edges stay on their edge so the mesh always fills the screen.
                if i > 0 && i < 3 { p.x += drift.x + push.x }
                if j > 0 && j < 3 { p.y += drift.y + push.y }
                points.append(p)
            }
        }
        return points
    }

    private var tints: [Color] {
        (0..<16).map { v in
            let row = 3 - v / 4, col = v % 4
            // Rows climb the spectrum from the bottom; columns pick within the row.
            let band = min(AudioFrame.bands - 1, row * 12 + col * 3 + 1)
            let level = (f.spectrum[band] + f.spectrum[max(0, band - 1)] + f.spectrum[min(AudioFrame.bands - 1, band + 1)]) / 3
            return Color(colors.grid[v], gain: 0.62 + 0.45 * level + 0.08 * f.kick)
        }
    }
}
