import AzaUI
import StreamBindings
import SwiftUI

/// Where a song's download is.
enum DownloadState: Equatable {
    case none
    /// Queued behind other songs.
    case waiting
    /// On its way now, 0–1.
    case downloading(Double)
    case downloaded
}

/// A song's download as Spotify shows it, beside its artist: a filled arrow once it's on this
/// device, a ring filling while it downloads, a dotted arrow while it waits its turn.
struct DownloadMark: View {
    let id: String
    /// What the row knows; the model's lookup is fresher when it has the song.
    let downloaded: Bool
    @Environment(AppModel.self) private var model
    @Environment(\.accent) private var accent

    var body: some View {
        switch model.downloadState(id, downloaded: model.track(id)?.downloaded ?? downloaded) {
        case .none: EmptyView()
        case .waiting:
            Image(systemName: "arrow.down.circle.dotted")
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .accessibilityLabel("Waiting to download")
                .accessibilityIdentifier("download.waiting")
        case let .downloading(p):
            ProgressRing(value: p)
                .frame(width: 10, height: 10)
                .padding(1)
                .accessibilityLabel("Downloading")
                .accessibilityIdentifier("download.downloading")
        case .downloaded:
            Image(systemName: "arrow.down.circle.fill")
                .font(.caption2)
                .foregroundStyle(accent)
                .accessibilityLabel("Downloaded")
                .accessibilityIdentifier("download.done")
        }
    }
}

/// A ring that fills clockwise from the top.
struct ProgressRing: View {
    let value: Double
    var lineWidth: CGFloat = 1.6
    @Environment(\.accent) private var accent

    var body: some View {
        ZStack {
            Circle().stroke(.quaternary, lineWidth: lineWidth)
            Circle()
                .trim(from: 0, to: max(0.02, min(1, value)))
                .stroke(accent, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
                .rotationEffect(.degrees(-90))
                .animation(.linear(duration: 0.25), value: value)
        }
    }
}

/// How far a set of songs (an album, a playlist, everything queued) has downloaded.
struct DownloadProgress {
    /// Songs here that can be downloaded.
    let total: Int
    /// Of those, on this device.
    let done: Int
    /// Of those, still queued or downloading.
    let pending: Int
    /// `done` plus the part of the current song that's arrived, over `total`.
    let fraction: Double
    let secondsLeft: UInt32?

    var active: Bool { pending > 0 }

    @MainActor
    init(_ tracks: [Track], model: AppModel) {
        let songs = tracks.filter(\.downloadable)
        let status = model.downloads
        var done = 0, pending = 0, partial = 0.0
        for t in songs {
            switch model.downloadState(t.id, downloaded: model.track(t.id)?.downloaded ?? t.downloaded) {
            case .downloaded: done += 1
            case .waiting: pending += 1
            case let .downloading(p): pending += 1; partial = p
            case .none: break
            }
        }
        total = songs.count
        self.done = done
        self.pending = pending
        fraction = songs.isEmpty ? 0 : (Double(done) + partial) / Double(songs.count)
        // The queue's time left, in proportion to how much of it is these songs.
        let left = Double(status.total) - Double(status.done) - Double(status.progress)
        secondsLeft = status.secondsLeft.flatMap { s in
            left > 0 ? UInt32((Double(s) * (Double(pending) - partial) / left).rounded(.up)) : nil
        }
    }

    /// "Downloading 3 of 12 · About 2 min left".
    var text: String {
        var line = "Downloading \(done + 1 > total ? total : done + 1) of \(total)"
        if let s = secondsLeft { line += " · \(Self.timeLeft(s))" }
        return line
    }

    static func timeLeft(_ seconds: UInt32) -> String {
        if seconds < 60 { return "Less than a minute left" }
        if seconds < 3600 { return "About \(Int((Double(seconds) / 60).rounded())) min left" }
        let h = Int(seconds / 3600), m = Int(seconds % 3600 / 60)
        return m == 0 ? "About \(h) hr left" : "About \(h) hr \(m) min left"
    }
}

/// The top of Library → Downloaded while songs are on their way: how many of how many, the
/// time left, a bar, then the song downloading now and the ones waiting.
struct DownloadQueueView: View {
    @Environment(AppModel.self) private var model
    /// Waiting songs listed before "and N more".
    private let shownWaiting = 50

    var body: some View {
        let s = model.downloads
        if s.total > 0 {
            VStack(alignment: .leading, spacing: 10) {
                VStack(alignment: .leading, spacing: 6) {
                    HStack(alignment: .firstTextBaseline) {
                        Text("Downloading \(min(s.done + 1, s.total)) of \(s.total)")
                            .font(.headline)
                            .contentTransition(.numericText())
                        Spacer()
                        if let left = s.secondsLeft {
                            Text(DownloadProgress.timeLeft(left))
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                    }
                    ProgressView(value: (Double(s.done) + Double(s.progress)) / Double(max(1, s.total)))
                        .animation(.linear(duration: 0.25), value: s.progress)
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("downloads.progress")
                let ids = (s.current.map { [$0] } ?? []) + s.queued.prefix(shownWaiting)
                ForEach(ids, id: \.self) { id in
                    if let t = model.track(id) {
                        TrackRow(track: t)
                            .accessibilityIdentifier("downloading.\(t.title)")
                    }
                }
                if s.queued.count > shownWaiting {
                    Text("and \(s.queued.count - shownWaiting) more")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Button("Cancel Downloads", role: .destructive) {
                    model.core.removeDownloads(trackIds: (s.current.map { [$0] } ?? []) + s.queued)
                }
                .font(.subheadline)
                .buttonStyle(.borderless)
                .accessibilityIdentifier("downloads.cancel")
                Text("Downloaded").font(.title3.weight(.bold)).padding(.top, 16)
            }
        }
    }
}
