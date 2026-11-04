import StreamBindings
import SwiftUI

extension Play: Identifiable {
    public var id: String { "\(device)-\(playedAt)-\(track.id)" }
}

/// Every song played, on every device signed in to your aza account, newest first and
/// grouped by day, each with the time it started and where.
struct HistoryView: View {
    @Environment(AppModel.self) private var model
    @State private var plays: [Play] = []
    @State private var exhausted = false
    @State private var loadedVersion: Int?
    private let pageSize: UInt32 = 200

    /// Plays in runs of one day each, in order.
    private var days: [(day: Date, plays: [Play])] {
        let calendar = Calendar.current
        var out: [(day: Date, plays: [Play])] = []
        for p in plays {
            let day = calendar.startOfDay(for: Date(timeIntervalSince1970: TimeInterval(p.playedAt)))
            if out.last?.day == day { out[out.count - 1].plays.append(p) } else { out.append((day, [p])) }
        }
        return out
    }

    var body: some View {
        content
            .navigationTitle("History")
            .overlay {
                if plays.isEmpty && exhausted {
                    ContentUnavailableView("Nothing played yet", systemImage: "clock",
                                           description: Text("Songs you play on any of your devices show up here."))
                }
            }
            .onAppear { if loadedVersion != model.historyVersion { reload() } }
            .onChange(of: model.historyVersion) { reload() }
    }

    #if os(macOS)
    private var content: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2) {
                PageTitle(text: "History").padding(.horizontal, 10)
                ForEach(days, id: \.day) { day in
                    Text(Self.title(day.day))
                        .font(.headline)
                        .padding(.horizontal, 10)
                        .padding(.top, 18)
                        .padding(.bottom, 6)
                    ForEach(day.plays) { p in
                        Button { play(p) } label: { HistoryRow(play: p) }
                            .buttonStyle(HoverRowStyle())
                            .disabled(!p.playable)
                            .trackMenu(p.track)
                            .accessibilityIdentifier("history.\(p.track.title)")
                            .onAppear { if p.id == plays.last?.id { loadMore() } }
                    }
                }
            }
            .padding(.leading, Metrics.page - 10)
            .padding(.trailing, Metrics.pageTrailing - 10)
            .padding(.bottom, 120)
        }
    }
    #else
    private var content: some View {
        List {
            ForEach(days, id: \.day) { day in
                Section(Self.title(day.day)) {
                    ForEach(day.plays) { p in
                        Button { play(p) } label: { HistoryRow(play: p) }
                            .buttonStyle(.plain)
                            .disabled(!p.playable)
                            .listRowSeparator(.hidden)
                            .trackMenu(p.track)
                            .accessibilityIdentifier("history.\(p.track.title)")
                            .onAppear { if p.id == plays.last?.id { loadMore() } }
                    }
                }
            }
        }
        .listStyle(.plain)
    }
    #endif

    /// "Today", "Yesterday", then "Monday 3 November" (with the year once it's not this one).
    static func title(_ day: Date) -> String {
        let calendar = Calendar.current
        if calendar.isDateInToday(day) { return "Today" }
        if calendar.isDateInYesterday(day) { return "Yesterday" }
        if calendar.isDate(day, equalTo: .now, toGranularity: .year) {
            return day.formatted(.dateTime.weekday(.wide).day().month(.wide))
        }
        return day.formatted(.dateTime.weekday(.wide).day().month(.wide).year())
    }

    private func play(_ p: Play) {
        model.play([p.track])
    }

    private func reload() {
        plays = model.core.history(offset: 0, limit: pageSize)
        exhausted = plays.count < Int(pageSize)
        loadedVersion = model.historyVersion
    }

    private func loadMore() {
        guard !exhausted else { return }
        let more = model.core.history(offset: UInt32(plays.count), limit: pageSize)
        exhausted = more.count < Int(pageSize)
        plays.append(contentsOf: more)
    }
}

/// A played song: art, title, artist and where, and the time it started.
struct HistoryRow: View {
    let play: Play

    var body: some View {
        HStack(spacing: Metrics.artGap) {
            ArtworkView(url: play.track.art, size: 48)
            VStack(alignment: .leading, spacing: 2) {
                SongTitle(id: play.track.id, title: play.track.title)
                Text("\(play.track.artist) · \(play.device)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Text(Date(timeIntervalSince1970: TimeInterval(play.playedAt)), format: .dateTime.hour().minute())
                .font(.subheadline.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
        .opacity(play.playable ? 1 : 0.45)
        .contentShape(Rectangle())
    }
}
