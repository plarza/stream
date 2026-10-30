import SwiftUI

/// Synced lyrics: big left-aligned lines, the active one lit with a sweep
/// across its words, the rest dimmed and blurred by distance. Tap a line to
/// jump there.
struct LyricsView: View {
    let track: Track?
    @Environment(AppModel.self) private var model
    @State private var lyrics: Lyrics?
    @State private var loaded = false
    @State private var userScrolling = false
    @State private var resumeFollow: Task<Void, Never>?

    var body: some View {
        Group {
            if let lyrics {
                if lyrics.sync == .unsynced {
                    ScrollView {
                        Text(lyrics.lines.map(\.text).joined(separator: "\n"))
                            .font(.title3.weight(.semibold))
                            .foregroundStyle(.white.opacity(0.85))
                            .lineSpacing(8)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.vertical, 24)
                        source(lyrics)
                    }
                    .scrollIndicators(.never)
                } else {
                    synced(lyrics)
                }
            } else if loaded {
                ContentUnavailableView("No lyrics", systemImage: "quote.bubble")
                    .foregroundStyle(.white.opacity(0.6))
                    .frame(maxHeight: .infinity)
            } else {
                ProgressView().tint(.white).frame(maxHeight: .infinity)
            }
        }
        .task(id: track?.id) {
            lyrics = nil
            loaded = false
            guard let track else { loaded = true; return }
            let found = await model.core.lyrics(track: track)
            withAnimation(.smooth) {
                lyrics = found
                loaded = true
            }
        }
    }

    private func synced(_ lyrics: Lyrics) -> some View {
        TimelineView(.animation(paused: model.player.status != .playing)) { _ in
            let ms = Int(model.core.positionMs()) + 250 // Lead slightly, like singing along.
            let active = activeIndex(lyrics, at: ms)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 26) {
                        Color.clear.frame(height: 40)
                        ForEach(Array(lyrics.lines.enumerated()), id: \.offset) { i, line in
                            LyricLineView(line: line, next: i + 1 < lyrics.lines.count ? lyrics.lines[i + 1].startMs : nil,
                                          state: i == active ? .active(ms) : .inactive(distance: abs(i - (active ?? -1))),
                                          accent: model.tint)
                                .id(i)
                                .onTapGesture {
                                    if let s = line.startMs { model.seek(to: s); Haptics.tap() }
                                }
                        }
                        source(lyrics)
                        Color.clear.frame(height: 220)
                    }
                    .padding(.trailing, 24)
                }
                .scrollIndicators(.never)
                .onScrollPhaseChange { _, phase in
                    if phase == .interacting {
                        userScrolling = true
                        resumeFollow?.cancel()
                    } else if phase == .idle {
                        resumeFollow = Task {
                            try? await Task.sleep(for: .seconds(3))
                            if !Task.isCancelled { userScrolling = false }
                        }
                    }
                }
                .onChange(of: active) { _, new in
                    guard let new, !userScrolling else { return }
                    withAnimation(.spring(response: 0.6, dampingFraction: 0.9)) {
                        proxy.scrollTo(new, anchor: UnitPoint(x: 0, y: 0.28))
                    }
                }
                .onAppear {
                    if let active { proxy.scrollTo(active, anchor: UnitPoint(x: 0, y: 0.28)) }
                }
            }
        }
    }

    private func source(_ l: Lyrics) -> some View {
        Text("Lyrics from \(l.source)").font(.caption).foregroundStyle(.white.opacity(0.4)).padding(.top, 12)
    }

    private func activeIndex(_ l: Lyrics, at ms: Int) -> Int? {
        var lo = 0, hi = l.lines.count - 1, found: Int?
        while lo <= hi {
            let mid = (lo + hi) / 2
            if let s = l.lines[mid].startMs, Int(s) <= ms { found = mid; lo = mid + 1 } else { hi = mid - 1 }
        }
        return found
    }
}

private struct LyricLineView: View {
    enum LineState: Equatable { case active(Int), inactive(distance: Int) }
    let line: LyricLine
    let next: UInt32?
    let state: LineState
    let accent: Color

    var body: some View {
        let isActive = if case .active = state { true } else { false }
        Text(line.text.isEmpty ? "♪" : line.text)
            .font(.system(size: 30, weight: .bold, design: .default))
            .tracking(-0.4)
            .foregroundStyle(isActive ? AnyShapeStyle(fill) : AnyShapeStyle(.white.opacity(0.32)))
            .blur(radius: blur)
            .scaleEffect(isActive ? 1 : 0.97, anchor: .leading)
            .animation(.spring(response: 0.45, dampingFraction: 0.85), value: isActive)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
    }

    private var blur: CGFloat {
        if case let .inactive(d) = state { return min(2.4, CGFloat(max(0, d - 1)) * 0.8) }
        return 0
    }

    /// Fill that sweeps left to right through the line as words are sung.
    private var fill: some ShapeStyle {
        guard case let .active(ms) = state, !line.words.isEmpty else { return AnyShapeStyle(Color.white) }
        let p = progress(ms)
        return AnyShapeStyle(LinearGradient(stops: [
            .init(color: .white, location: 0),
            .init(color: .white, location: max(0, p - 0.02)),
            .init(color: .white.opacity(0.4), location: min(1, p + 0.04)),
            .init(color: .white.opacity(0.4), location: 1),
        ], startPoint: .leading, endPoint: .trailing))
    }

    /// Share of the line's characters sung so far, interpolated within the current word.
    private func progress(_ ms: Int) -> Double {
        let words = line.words
        let total = Double(words.reduce(0) { $0 + $1.text.count })
        guard total > 0 else { return 1 }
        var done = 0.0
        for (i, w) in words.enumerated() {
            let start = Int(w.startMs)
            let end = i + 1 < words.count ? Int(words[i + 1].startMs) : Int(next ?? UInt32(start + 600))
            if ms >= end { done += Double(w.text.count); continue }
            if ms >= start {
                let f = Double(ms - start) / Double(max(1, end - start))
                done += Double(w.text.count) * min(1, f)
            }
            break
        }
        return done / total
    }
}
