import StreamBindings
import SwiftUI

/// "Playing Next" (songs you queued) above "Continue from …" (the rest of the
/// album or playlist). Drag to reorder, swipe to remove, tap to jump.
struct QueueView: View {
    @Environment(AppModel.self) private var model
    @State private var snapshot = QueueSnapshot(upNext: [], context: [], contextTitle: nil)

    var body: some View {
        List {
            if !snapshot.upNext.isEmpty {
                // Headers are plain rows so they share the rows' left edge.
                HStack {
                    Text("Playing Next")
                    Spacer()
                    Button("Clear") {
                        for e in snapshot.upNext { model.core.queueRemove(key: e.key) }
                    }
                    .buttonStyle(.plain)
                    .font(.subheadline)
                    .foregroundStyle(.white.opacity(0.7))
                    .accessibilityIdentifier("queue.clear")
                }
                .sectionHeader()
                ForEach(snapshot.upNext, id: \.key) { e in row(e) }
                    .onMove { from, to in move(in: snapshot.upNext, from: from, to: to, end: snapshot.context.first?.key) }
                    .onDelete { idx in idx.forEach { model.core.queueRemove(key: snapshot.upNext[$0].key) } }
            }
            if !snapshot.context.isEmpty {
                Text(snapshot.contextTitle.map { "Continue from \($0)" } ?? "Up Next")
                    .lineLimit(1)
                    .sectionHeader()
                ForEach(snapshot.context, id: \.key) { e in row(e) }
                    .onMove { from, to in move(in: snapshot.context, from: from, to: to, end: nil) }
                    .onDelete { idx in idx.forEach { model.core.queueRemove(key: snapshot.context[$0].key) } }
            }
            if snapshot.upNext.isEmpty && snapshot.context.isEmpty {
                Text("Nothing up next").foregroundStyle(.white.opacity(0.5)).listRowBackground(Color.clear)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .environment(\.defaultMinListRowHeight, 52)
        .task(id: model.player.queueVersion) {
            withAnimation(.smooth) { snapshot = model.core.queue() }
        }
    }

    private func row(_ e: QueueEntry) -> some View {
        Button { model.core.queueJump(key: e.key) } label: {
            HStack(spacing: Metrics.artGap) {
                ArtworkView(url: e.track.art, size: 48)
                VStack(alignment: .leading, spacing: 2) {
                    Text(e.track.title).lineLimit(1)
                    Text(e.track.artist).font(.subheadline).foregroundStyle(.white.opacity(0.6)).lineLimit(1)
                }
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("queue.\(e.track.title)")
        .listRowBackground(Color.clear)
        .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
        .listRowSeparator(.hidden)
    }

    private func move(in list: [QueueEntry], from: IndexSet, to: Int, end: UInt64?) {
        guard let f = from.first else { return }
        let key = list[f].key
        let before = to < list.count ? list[to].key : end
        Haptics.tap()
        model.core.queueMove(key: key, before: before == key ? nil : before)
    }
}

private extension View {
    func sectionHeader() -> some View {
        self.font(.headline).foregroundStyle(.white).textCase(nil)
            .padding(.top, 14)
            .padding(.bottom, 2)
            .listRowInsets(EdgeInsets())
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
            .moveDisabled(true)
            .deleteDisabled(true)
    }
}
