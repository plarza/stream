import SwiftUI

/// Album, playlist and artist pages: compact header (art + details side by
/// side) on a page tinted from the artwork, an actions row, then the songs.
struct CollectionView: View {
    let item: Item
    @Environment(AppModel.self) private var model
    @State private var page: CollectionPage?
    @State private var palette: Palette = .fallback
    @State private var error: String?
    @State private var pinned = false
    @State private var liked = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                    .padding(.horizontal, Metrics.page)
                    .padding(.top, 12)
                actions
                    .padding(.horizontal, Metrics.page)
                    .padding(.vertical, 16)
                if let page {
                    tracks(page)
                    if !page.albums.isEmpty { albums(page) }
                    if let d = page.description, item.kind != .artist {
                        Text(d).font(.footnote).foregroundStyle(.secondary).padding(Metrics.page)
                    }
                } else if let error {
                    ContentUnavailableView("Couldn’t load", systemImage: "exclamationmark.triangle", description: Text(error))
                        .padding(.top, 40)
                } else {
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 60)
                }
            }
            .padding(.bottom, 120)
        }
        .background(alignment: .top) {
            LinearGradient(colors: [palette.top, palette.top.opacity(0.5), .clear], startPoint: .top, endPoint: .bottom)
                .frame(height: 520)
                .ignoresSafeArea()
                .animation(.smooth(duration: 0.6), value: palette)
        }
        .navigationTitle(page?.item.title ?? item.title)
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(.hidden, for: .navigationBar)
        #endif
        .task(id: item.id) { await load() }
        .task(id: page?.item.art ?? item.art) {
            let p = await Palette.extract(from: page?.item.art ?? item.art)
            withAnimation(.smooth(duration: 0.6)) { palette = p }
        }
    }

    private var shown: Item { page?.item ?? item }

    private var header: some View {
        HStack(alignment: .bottom, spacing: 16) {
            ArtworkView(url: shown.art, size: 132, circle: item.kind == .artist)
                .shadow(color: .black.opacity(0.35), radius: 16, y: 8)
            VStack(alignment: .leading, spacing: 4) {
                Text(shown.title).font(.title2.weight(.bold)).lineLimit(3)
                if item.kind != .artist {
                    Text(shown.subtitle).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                }
                if let page {
                    Text(page.detail).font(.caption).foregroundStyle(.tertiary)
                    if item.kind == .artist, let d = page.description {
                        Text(d).font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                    }
                    if let q = commonQuality(page.tracks) {
                        HStack(spacing: 4) {
                            if page.tracks.allSatisfy(\.downloaded) { DownloadedBadge() }
                            QualityChip(text: q)
                        }
                        .padding(.top, 4)
                    }
                }
            }
            Spacer(minLength: 0)
        }
    }

    private var actions: some View {
        HStack(spacing: 18) {
            Button {
                if let page { model.play(page.tracks, from: page.item) }
            } label: {
                Image(systemName: "play.fill")
                    .font(.title3)
                    .frame(width: 52, height: 52)
                    .foregroundStyle(.black)
                    .background(model.tint, in: .circle)
            }
            .buttonStyle(.pressable)
            .accessibilityLabel("Play")
            .accessibilityIdentifier("collection.play")
            Button {
                if let page { model.play(page.tracks, from: page.item, shuffle: true) }
            } label: { Image(systemName: "shuffle").font(.title3) }
                .buttonStyle(.pressable)
                .accessibilityLabel("Shuffle")
                .accessibilityIdentifier("collection.shuffle")
            if item.kind == .album, item.source == .spotify {
                Button { toggleSaved() } label: {
                    Image(systemName: liked ? "heart.fill" : "heart").font(.title3)
                        .contentTransition(.symbolEffect(.replace))
                }
                .buttonStyle(.pressable)
                .accessibilityLabel(liked ? "Remove from Library" : "Add to Library")
                .accessibilityIdentifier("collection.like")
            }
            Button {
                model.togglePin(shown)
                withAnimation(.snappy) { pinned.toggle() }
            } label: {
                Image(systemName: pinned ? "pin.fill" : "pin").font(.title3)
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.pressable)
            .accessibilityLabel(pinned ? "Unpin" : "Pin")
            .accessibilityIdentifier("collection.pin")
            #if os(iOS)
            if shown.source == .spotify, item.kind != .artist {
                Button {
                    Task {
                        model.show("Downloading…")
                        do { try await model.core.download(collectionId: item.id) ; model.show("Downloaded") } catch { model.show(error.localizedDescription) }
                    }
                } label: { Image(systemName: "arrow.down.circle").font(.title3) }
                    .buttonStyle(.pressable)
            }
            #endif
            Spacer()
        }
        .foregroundStyle(.primary)
    }

    @ViewBuilder
    private func tracks(_ page: CollectionPage) -> some View {
        if item.kind == .artist, !page.tracks.isEmpty {
            Text("Popular").font(.title3.weight(.bold)).padding(.horizontal, Metrics.page).padding(.bottom, 4)
        }
        LazyVStack(spacing: 0) {
            ForEach(Array(page.tracks.prefix(item.kind == .artist ? 10 : .max).enumerated()), id: \.element.id) { i, t in
                Button { model.play(page.tracks, at: i, from: page.item) } label: {
                    TrackRow(track: t, showArt: item.kind != .album, number: item.kind == .album ? Int(t.trackNumber == 0 ? UInt32(i + 1) : t.trackNumber) : nil,
                             isCurrent: model.player.track?.id == t.id,
                             showArtist: item.kind != .album || t.artist != page.item.subtitle)
                        .padding(.horizontal, Metrics.page)
                        .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("track.\(t.title)")
                .trackMenu(t)
                Divider().padding(.leading, item.kind == .album ? Metrics.page + 34 : Metrics.page + 60)
            }
        }
    }

    private func albums(_ page: CollectionPage) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Albums").font(.title3.weight(.bold)).padding(.horizontal, Metrics.page)
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 14) {
                    ForEach(page.albums) { a in
                        NavigationLink(value: a) { ItemTile(item: a, size: 140) }
                            .buttonStyle(.pressable)
                            .itemMenu(a)
                    }
                }
                .padding(.horizontal, Metrics.page)
            }
        }
        .padding(.top, 24)
    }

    private func load() async {
        pinned = model.core.isPinned(id: item.id)
        do {
            let p = try await model.core.collection(id: item.id)
            withAnimation(.smooth) {
                page = p
                liked = p.liked
                pinned = p.pinned
            }
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func toggleSaved() {
        liked.toggle()
        Haptics.success()
        let id = item.id, on = liked
        Task {
            do { try await model.core.setLiked(trackId: id, liked: on) } catch { model.show(error.localizedDescription) }
        }
    }

    private func commonQuality(_ tracks: [Track]) -> String? {
        guard let first = tracks.first?.quality, !first.isEmpty else { return nil }
        return tracks.allSatisfy { $0.quality == first } ? first : nil
    }
}
