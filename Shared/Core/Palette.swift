import CoreGraphics
import SwiftUI

/// Colors taken from album art: a vivid accent for highlights and two deep
/// tones for backgrounds. Computed once per cover on a 24×24 thumbnail.
struct Palette: Equatable, Sendable {
    var accent: Color
    var top: Color
    var bottom: Color

    static let fallback = Palette(
        accent: Color(red: 1.0, green: 0.55, blue: 0.4),
        top: Color(red: 0.25, green: 0.12, blue: 0.3),
        bottom: Color(red: 0.06, green: 0.05, blue: 0.08)
    )

    private static let cache = PaletteCache()

    static func extract(from url: String?) async -> Palette {
        guard let url else { return .fallback }
        if let p = await cache.get(url) { return p }
        guard let cg = await ArtworkStore.shared.cgImage(for: url, pixels: 128) else { return .fallback }
        let p = compute(cg)
        await cache.set(url, p)
        return p
    }

    private static func compute(_ image: CGImage) -> Palette {
        let n = 24
        var pixels = [UInt8](repeating: 0, count: n * n * 4)
        let space = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: &pixels, width: n, height: n, bitsPerComponent: 8, bytesPerRow: n * 4,
                                  space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return .fallback }
        ctx.interpolationQuality = .medium
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: n, height: n))

        // Bucket hues; pick the most common reasonably saturated one.
        var buckets = [(count: Double, r: Double, g: Double, b: Double)](repeating: (0, 0, 0, 0), count: 24)
        var avg = (r: 0.0, g: 0.0, b: 0.0)
        for i in stride(from: 0, to: pixels.count, by: 4) {
            let r = Double(pixels[i]) / 255, g = Double(pixels[i + 1]) / 255, b = Double(pixels[i + 2]) / 255
            avg.r += r; avg.g += g; avg.b += b
            let (h, s, v) = hsv(r, g, b)
            guard s > 0.25, v > 0.25 else { continue }
            let k = min(23, Int(h * 24))
            let w = s * v
            buckets[k].count += w
            buckets[k].r += r * w; buckets[k].g += g * w; buckets[k].b += b * w
        }
        let total = Double(n * n)
        avg = (avg.r / total, avg.g / total, avg.b / total)
        var accent = (r: 1.0, g: 0.55, b: 0.4)
        if let best = buckets.max(by: { $0.count < $1.count }), best.count > 0 {
            accent = (best.r / best.count, best.g / best.count, best.b / best.count)
        }
        // Lift the accent so it reads on dark glass.
        var (h, s, v) = hsv(accent.r, accent.g, accent.b)
        s = min(1, max(0.45, s)); v = max(0.85, v)
        let a = rgb(h, s, v)
        let (ah, asat, _) = hsv(avg.r, avg.g, avg.b)
        let top = rgb(ah, min(0.7, asat + 0.1), 0.32)
        let bottom = rgb(ah, min(0.6, asat), 0.1)
        return Palette(
            accent: Color(red: a.0, green: a.1, blue: a.2),
            top: Color(red: top.0, green: top.1, blue: top.2),
            bottom: Color(red: bottom.0, green: bottom.1, blue: bottom.2)
        )
    }

    private static func hsv(_ r: Double, _ g: Double, _ b: Double) -> (Double, Double, Double) {
        let mx = max(r, g, b), mn = min(r, g, b), d = mx - mn
        var h = 0.0
        if d > 0 {
            if mx == r { h = ((g - b) / d).truncatingRemainder(dividingBy: 6) }
            else if mx == g { h = (b - r) / d + 2 }
            else { h = (r - g) / d + 4 }
            h /= 6
            if h < 0 { h += 1 }
        }
        return (h, mx == 0 ? 0 : d / mx, mx)
    }

    private static func rgb(_ h: Double, _ s: Double, _ v: Double) -> (Double, Double, Double) {
        let i = Int(h * 6) % 6, f = h * 6 - floor(h * 6)
        let p = v * (1 - s), q = v * (1 - f * s), t = v * (1 - (1 - f) * s)
        switch i {
        case 0: return (v, t, p)
        case 1: return (q, v, p)
        case 2: return (p, v, t)
        case 3: return (p, q, v)
        case 4: return (t, p, v)
        default: return (v, p, q)
        }
    }
}

actor PaletteCache {
    private var map: [String: Palette] = [:]
    func get(_ k: String) -> Palette? { map[k] }
    func set(_ k: String, _ p: Palette) {
        if map.count > 300 { map.removeAll() }
        map[k] = p
    }
}
