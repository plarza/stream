import Accelerate
import simd
import SwiftUI

/// The music, drawn behind Now Playing. One of four looks while we pick
/// (`-visualizer aurora|halo|silk|bokeh`, or the `visualizer` default).
enum Visualizer: String, CaseIterable, Identifiable {
    case aurora, halo, silk, bokeh
    var id: String { rawValue }

    static var current: Visualizer {
        Visualizer(rawValue: UserDefaults.standard.string(forKey: "visualizer") ?? "") ?? .aurora
    }
}

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
struct NowPlayingVisualizer: View {
    let style: Visualizer
    let url: String?
    /// Where the cover is, in global coordinates.
    let cover: CGRect
    let expanded: Bool
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var colors = CoverColors.fallback
    @State private var bokeh = BokehField()

    private var playing: Bool { model.player.status == .playing }

    var body: some View {
        GeometryReader { geo in
            let origin = geo.frame(in: .global).origin
            let art = cover.offsetBy(dx: -origin.x, dy: -origin.y)
            TimelineView(.animation(paused: !playing || reduceMotion)) { tl in
                let f = AudioAnalysis.shared.update(at: tl.date, playing: playing)
                switch style {
                case .aurora: AuroraLayer(f: f, colors: colors)
                case .halo: HaloLayer(f: f, url: url, art: art, colors: colors).opacity(expanded ? 1 : 0)
                case .silk: SilkLayer(f: f, art: art, colors: colors, expanded: expanded)
                case .bokeh: BokehLayer(f: bokeh.advanced(f, in: geo.size), field: bokeh, colors: colors)
                }
            }
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .opacity(playing ? 1 : 0.55)
        .animation(.smooth(duration: 0.8), value: playing)
        .animation(.player, value: expanded)
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
                let drift = SIMD2(sin(f.flow * 0.9 + k * 1.7), cos(f.flow * 0.7 + k * 2.3)) * 0.15
                let away = p - SIMD2(0.5, 0.42)
                let push = away / max(simd_length(away), 0.01) * 0.1 * f.kick
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
            return Color(colors.grid[v], gain: 0.62 + 0.75 * level + 0.15 * f.kick)
        }
    }
}

// MARK: Halo

/// The cover lights the room: its own colours bloom behind it on the bass, and
/// a fine ring of rays around it traces the spectrum, mirrored left and right,
/// bass reaching up from the top and the air settling below.
private struct HaloLayer: View {
    let f: AudioFrame
    let url: String?
    let art: CGRect
    let colors: CoverColors

    var body: some View {
        ZStack {
            FillArtwork(url: url, pixels: 128)
                .frame(width: art.width, height: art.height)
                .clipShape(RoundedRectangle(cornerRadius: Metrics.radius(for: art.width), style: .continuous))
                .scaleEffect(1.02 + 0.08 * CGFloat(f.bass) + 0.06 * CGFloat(f.kick))
                .blur(radius: 34)
                .saturation(1.5)
                .opacity(Double(0.3 + 0.45 * f.energy))
                .position(x: art.midX, y: art.midY)
            Canvas { ctx, _ in
                let rays = rays()
                // A soft coloured bloom under crisp white strokes.
                ctx.drawLayer { glow in
                    glow.addFilter(.blur(radius: 7))
                    glow.blendMode = .plusLighter
                    for r in rays {
                        glow.stroke(r.path, with: .color(Color(r.tint, gain: 1.2).opacity(Double(0.25 + 0.75 * r.level))),
                                    style: StrokeStyle(lineWidth: 6, lineCap: .round))
                    }
                }
                ctx.blendMode = .plusLighter
                for r in rays {
                    ctx.stroke(r.path, with: .color(.white.opacity(Double(0.18 + 0.6 * r.level))),
                               style: StrokeStyle(lineWidth: 2, lineCap: .round))
                }
            }
        }
    }

    private struct Ray { var path: Path; var level: Float; var tint: SIMD3<Float> }

    private func rays() -> [Ray] {
        guard art.width > 40 else { return [] }
        let count = 132
        let c = CGPoint(x: art.midX, y: art.midY)
        let a = art.width / 2 + 14, b = art.height / 2 + 14
        let vivid = colors.vivid
        var out: [Ray] = []
        out.reserveCapacity(count)
        for k in 0..<count {
            let phi = Double(k) / Double(count) * 2 * .pi
            let dir = CGPoint(x: sin(phi), y: -cos(phi))
            // 0 at the top, 1 at the bottom, the same on both sides.
            let m = (phi <= .pi ? phi : 2 * .pi - phi) / .pi
            let pos = Float(m) * Float(AudioFrame.bands - 6)
            let i = Int(pos), t = pos - Float(i)
            var level = f.spectrum[i] * (1 - t) + f.spectrum[min(i + 1, AudioFrame.bands - 1)] * t
            level = pow(level, 1.4)
            // A squircle a little outside the cover, so the rays hug its shape.
            let p = 5.0
            let r = pow(pow(abs(dir.x) / a, p) + pow(abs(dir.y) / b, p), -1 / p)
            // Shorter below, where the title sits.
            let room = 1 - 0.55 * max(0, dir.y)
            let length = (3 + 64 * CGFloat(level) * (1 + 0.25 * CGFloat(f.kick))) * room
            var path = Path()
            path.move(to: CGPoint(x: c.x + dir.x * r, y: c.y + dir.y * r))
            path.addLine(to: CGPoint(x: c.x + dir.x * (r + length), y: c.y + dir.y * (r + length)))
            out.append(Ray(path: path, level: level, tint: vivid[Int(m * 3.99)]))
        }
        return out
    }
}

// MARK: Silk

/// Ribbons of light flowing under the controls, like silk in a breeze. Each
/// ribbon is a few slow waves whose heights are its own slice of the
/// spectrum, so the bass ribbon heaves and the treble one flutters.
private struct SilkLayer: View {
    let f: AudioFrame
    let art: CGRect
    let colors: CoverColors
    let expanded: Bool

    var body: some View {
        Canvas { ctx, size in
            let vivid = colors.vivid
            // Under the cover, through the title and the scrubber.
            let base = expanded ? art.maxY + (size.height - art.maxY) * 0.38 : size.height * 0.62
            ctx.blendMode = .plusLighter
            for r in 0..<4 {
                let ribbon = ribbon(r, width: size.width, base: base)
                let tint = Color(vivid[r % vivid.count], gain: 1.15)
                let next = Color(vivid[(r + 2) % vivid.count], gain: 1.15)
                ctx.drawLayer { glow in
                    glow.addFilter(.blur(radius: 16))
                    glow.fill(ribbon.body, with: .linearGradient(Gradient(colors: [tint.opacity(0), tint.opacity(0.5), next.opacity(0.5), next.opacity(0)]),
                                                                  startPoint: .zero, endPoint: CGPoint(x: size.width, y: 0)))
                }
                ctx.fill(ribbon.body, with: .linearGradient(Gradient(colors: [tint.opacity(0), tint.opacity(0.22), next.opacity(0.22), next.opacity(0)]),
                                                            startPoint: .zero, endPoint: CGPoint(x: size.width, y: 0)))
                ctx.stroke(ribbon.edge, with: .linearGradient(Gradient(colors: [.white.opacity(0), .white.opacity(0.55), .white.opacity(0)]),
                                                              startPoint: .zero, endPoint: CGPoint(x: size.width, y: 0)),
                           lineWidth: 1.2)
            }
        }
    }

    private func ribbon(_ r: Int, width: CGFloat, base: CGFloat) -> (body: Path, edge: Path) {
        let slice = AudioFrame.bands / 4
        let level = { (k: Int) -> Float in
            let lo = r * slice + k * slice / 3
            return (lo..<(lo + slice / 3)).map { f.spectrum[min($0, AudioFrame.bands - 1)] }.reduce(0, +) / Float(slice / 3)
        }
        let amps = (0..<3).map { CGFloat(0.25 + level($0)) }
        let speed = Float(r) * 0.35 + 0.8
        let steps = 90
        var top: [CGPoint] = [], under: [CGPoint] = []
        for s in 0...steps {
            let u = CGFloat(s) / CGFloat(steps)
            let x = u * width
            // Tapers to nothing at both edges of the screen.
            let env = pow(sin(.pi * u), 1.6)
            let t = CGFloat(f.flow * speed) + CGFloat(r) * 1.9
            let wave = amps[0] * sin(u * 2.1 * .pi + t)
                + amps[1] * 0.6 * sin(u * 4.3 * .pi - t * 1.3 + 1)
                + amps[2] * 0.35 * sin(u * 7.7 * .pi + t * 1.9 + 2)
            let height = (34 + 70 * CGFloat(f.energy) + 26 * CGFloat(f.kick)) * env
            let y = base + CGFloat(r - 1) * 10 + wave * height
            top.append(CGPoint(x: x, y: y))
            // The ribbon's other edge lags a little, so it twists as it moves.
            let twist = amps[0] * sin(u * 2.1 * .pi + t - 0.9)
            under.append(CGPoint(x: x, y: base + CGFloat(r - 1) * 10 + twist * height * 0.55))
        }
        var edge = Path()
        edge.addLines(top)
        var body = Path()
        body.addLines(top)
        body.addLines(under.reversed())
        body.closeSubpath()
        return (body, edge)
    }
}

// MARK: Bokeh

/// Out-of-focus lights in the cover's colours drifting up through the room.
/// The music sets how fast they rise and how bright they glow; on a beat they
/// all swell together.
@MainActor
final class BokehField {
    struct Light {
        var x, y, radius, depth, speed, phase: CGFloat
        var band: Int
        var tint: Int
    }
    var lights: [Light] = []
    private var size: CGSize = .zero

    func step(_ f: AudioFrame, in size: CGSize) {
        if lights.isEmpty || size != self.size {
            self.size = size
            lights = (0..<34).map { _ in spawn(anywhere: true) }
        }
        let dt = CGFloat(f.dt)
        for i in lights.indices {
            var l = lights[i]
            let rise = l.speed * (0.25 + 1.6 * CGFloat(f.energy) + 1.5 * CGFloat(f.kick)) / l.depth
            l.y -= rise * dt * 40
            l.x += sin(CGFloat(f.flow) * 0.8 + l.phase) * dt * 10 / l.depth
            if l.y < -l.radius * 2 { l = spawn(anywhere: false) }
            lights[i] = l
        }
    }

    /// Moves the lights on by one frame, then hands the frame back for drawing.
    func advanced(_ f: AudioFrame, in size: CGSize) -> AudioFrame {
        step(f, in: size)
        return f
    }

    private func spawn(anywhere: Bool) -> Light {
        let depth = CGFloat.random(in: 1...3)
        let radius = CGFloat.random(in: 14...44) * (1.6 - depth * 0.3)
        return Light(x: .random(in: 0...max(size.width, 1)),
                     y: anywhere ? .random(in: 0...max(size.height, 1)) : size.height + radius * 2,
                     radius: radius, depth: depth, speed: .random(in: 0.6...1.4), phase: .random(in: 0...(2 * .pi)),
                     band: .random(in: 0..<AudioFrame.bands), tint: .random(in: 0..<5))
    }
}

private struct BokehLayer: View {
    let f: AudioFrame
    let field: BokehField
    let colors: CoverColors

    var body: some View {
        Canvas { ctx, size in
            let vivid = colors.vivid
            ctx.blendMode = .plusLighter
            // Far lights first, blurrier and dimmer.
            for l in field.lights.sorted(by: { $0.depth > $1.depth }) {
                let level = CGFloat(f.spectrum[l.band])
                let r = l.radius * (1 + 0.18 * level + 0.22 * CGFloat(f.kick))
                let rect = CGRect(x: l.x - r, y: l.y - r, width: r * 2, height: r * 2)
                let tint = Color(vivid[l.tint % vivid.count], gain: 1.25)
                let alpha = (0.2 + 0.5 * level + 0.2 * CGFloat(f.kick)) / sqrt(l.depth)
                var layer = ctx
                layer.addFilter(.blur(radius: (l.depth - 1) * 3 + 1))
                // A lens disc: nearly flat, with a slightly brighter rim.
                layer.fill(Path(ellipseIn: rect), with: .radialGradient(
                    Gradient(stops: [.init(color: tint.opacity(alpha * 0.7), location: 0),
                                     .init(color: tint.opacity(alpha * 0.85), location: 0.82),
                                     .init(color: tint.opacity(alpha), location: 0.93),
                                     .init(color: tint.opacity(0), location: 1)]),
                    center: CGPoint(x: rect.midX, y: rect.midY), startRadius: 0, endRadius: r))
            }
        }
    }
}
