import CryptoKit
import ImageIO
import SwiftUI

/// Artwork loading with a small memory cache and a disk cache for remote
/// images. Every image is decoded straight to the size it is shown at with
/// ImageIO, so a grid of covers costs kilobytes, not megabytes.
actor ArtworkStore {
    static let shared = ArtworkStore()

    /// NSCache is thread-safe, so views can read it synchronously on first render.
    nonisolated(unsafe) private static let memory: NSCache<NSString, CGImageBox> = {
        let c = NSCache<NSString, CGImageBox>()
        c.totalCostLimit = 48 * 1024 * 1024
        return c
    }()
    private let dir: URL
    private var inflight: [String: Task<CGImage?, Never>] = [:]
    private let session: URLSession

    init() {
        dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("network.aza.music/remote-art", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let config = URLSessionConfiguration.default
        config.urlCache = nil
        config.httpMaximumConnectionsPerHost = 6
        session = URLSession(configuration: config)
    }

    private static func bucket(_ pixels: Int) -> Int {
        pixels <= 128 ? 128 : pixels <= 320 ? 320 : pixels <= 700 ? 700 : 1400
    }

    /// A cover already decoded at this size, without waiting.
    nonisolated static func cached(_ url: String?, pixels: Int) -> CGImage? {
        guard let url, !url.isEmpty else { return nil }
        return memory.object(forKey: "\(bucket(pixels))|\(url)" as NSString)?.image
    }

    func cgImage(for url: String?, pixels: Int) async -> CGImage? {
        guard let url, !url.isEmpty else { return nil }
        let bucket = Self.bucket(pixels)
        let key = "\(bucket)|\(url)" as NSString
        let memory = Self.memory
        if let hit = memory.object(forKey: key) { return hit.image }
        if let t = inflight[key as String] { return await t.value }
        let task = Task<CGImage?, Never> { [dir, session] in
            guard let file = await Self.localFile(for: url, dir: dir, session: session) else { return nil }
            return Self.downsample(file, pixels: bucket)
        }
        inflight[key as String] = task
        let image = await task.value
        inflight[key as String] = nil
        if let image { memory.setObject(CGImageBox(image), forKey: key, cost: image.bytesPerRow * image.height) }
        return image
    }

    func image(for url: String?, pixels: Int) async -> PlatformImage? {
        guard let cg = await cgImage(for: url, pixels: pixels) else { return nil }
        #if os(iOS)
        return UIImage(cgImage: cg)
        #else
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
        #endif
    }

    private static func localFile(for url: String, dir: URL, session: URLSession) async -> URL? {
        if url.hasPrefix("file://") { return URL(string: url) }
        guard let remote = URL(string: url) else { return nil }
        let name = SHA256.hash(data: Data(url.utf8)).prefix(12).map { String(format: "%02x", $0) }.joined()
        let file = dir.appendingPathComponent(name)
        if FileManager.default.fileExists(atPath: file.path) { return file }
        guard let (tmp, response) = try? await session.download(from: remote),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        try? FileManager.default.moveItem(at: tmp, to: file)
        return file
    }

    private static func downsample(_ file: URL, pixels: Int) -> CGImage? {
        let options = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let src = CGImageSourceCreateWithURL(file as CFURL, options) else { return nil }
        let thumb = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: pixels,
        ] as CFDictionary
        return CGImageSourceCreateThumbnailAtIndex(src, 0, thumb)
    }
}

final class CGImageBox: @unchecked Sendable {
    let image: CGImage
    init(_ image: CGImage) { self.image = image }
}

enum Display {
    /// Pixels per point of the main screen (artwork is decoded at its shown size).
    #if os(iOS)
    static let scale: CGFloat = 3
    #else
    static let scale: CGFloat = 2
    #endif
}

/// A square cover that fades in once decoded.
struct ArtworkView: View {
    let url: String?
    var size: CGFloat
    /// Defaults to the shared radius for this size.
    var cornerRadius: CGFloat? = nil
    var circle = false
    @State private var image: CGImage?

    init(url: String?, size: CGFloat, cornerRadius: CGFloat? = nil, circle: Bool = false) {
        self.url = url
        self.size = size
        self.cornerRadius = cornerRadius
        self.circle = circle
        _image = State(initialValue: ArtworkStore.cached(url, pixels: Int(size * Display.scale)))
    }

    var body: some View {
        ZStack {
            Rectangle().fill(.quaternary)
            if let image {
                Image(decorative: image, scale: 1)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .transition(.opacity)
            } else {
                Image(systemName: "music.note")
                    .font(.system(size: max(10, size * 0.28), weight: .medium))
                    .foregroundStyle(.tertiary)
            }
        }
        .frame(width: size, height: size)
        .clipShape(circle ? AnyShape(Circle()) : AnyShape(RoundedRectangle(cornerRadius: cornerRadius ?? Metrics.radius(for: size), style: .continuous)))
        .overlay {
            (circle ? AnyShape(Circle()) : AnyShape(RoundedRectangle(cornerRadius: cornerRadius ?? Metrics.radius(for: size), style: .continuous)))
                .stroke(.white.opacity(0.08), lineWidth: 0.5)
        }
        .task(id: url) {
            let px = Int(size * Display.scale)
            if let hit = ArtworkStore.cached(url, pixels: px) { image = hit; return }
            let loaded = await ArtworkStore.shared.cgImage(for: url, pixels: px)
            withAnimation(.easeOut(duration: 0.2)) { image = loaded }
        }
    }
}

/// A cover that fills whatever space it is given (Now Playing, headers).
struct FillArtwork: View {
    let url: String?
    var pixels: Int = 1400
    @State private var image: CGImage?

    init(url: String?, pixels: Int = 1400) {
        self.url = url
        self.pixels = pixels
        _image = State(initialValue: ArtworkStore.cached(url, pixels: pixels))
    }

    var body: some View {
        // Color.clear takes exactly the offered size; the image fills it and is clipped,
        // so a cover never widens its container.
        Color.clear
            .overlay {
                ZStack {
                    Rectangle().fill(.quaternary)
                    if let image {
                        Image(decorative: image, scale: 1).resizable().aspectRatio(contentMode: .fill)
                            .transition(.opacity.animation(.easeOut(duration: 0.35)))
                            .id(url)
                    }
                }
            }
            .clipped()
            .task(id: url) {
                if let hit = ArtworkStore.cached(url, pixels: pixels) { image = hit; return }
                let loaded = await ArtworkStore.shared.cgImage(for: url, pixels: pixels)
                withAnimation(.easeOut(duration: 0.35)) { image = loaded }
            }
    }
}
