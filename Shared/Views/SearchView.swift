import StreamBindings
import SwiftUI

extension SearchFilter: Identifiable, CaseIterable {
    public var id: Self { self }
    public static var allCases: [SearchFilter] { [.all, .songs, .albums, .artists, .playlists, .podcasts] }
    var title: String {
        switch self {
        case .all: "All"
        case .songs: "Songs"
        case .albums: "Albums"
        case .artists: "Artists"
        case .playlists: "Playlists"
        case .podcasts: "Podcasts"
        }
    }
}

/// Holds query state so the search field (owned by the tab/toolbar) and the
/// results share it.
@MainActor
@Observable
final class SearchModel {
    var query = ""
    var filter: SearchFilter = .all
    var library: [Item] = []
    var spotify: [Item] = []
    var loading = false
    /// Why Spotify gave nothing, when it failed rather than found nothing.
    var spotifyError: String?
    private var task: Task<Void, Never>?

    /// Library hits first (instant), then Spotify's, without duplicates.
    var results: [Item] {
        let ids = Set(library.map(\.id))
        return library + spotify.filter { !ids.contains($0.id) }
    }

    func run(_ model: AppModel) {
        task?.cancel()
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else {
            library = []; spotify = []; loading = false; spotifyError = nil
            return
        }
        library = model.core.searchLibrary(query: q, filter: filter)
        let filter = self.filter
        task = Task {
            try? await Task.sleep(for: .milliseconds(220))
            guard !Task.isCancelled else { return }
            loading = true
            var found: [Item] = []
            var failure: String?
            do { found = try await model.core.searchSpotify(query: q, filter: filter) } catch CoreError.NotSignedIn {
                failure = "Connect Spotify to search it too"
            } catch {
                failure = AppModel.message(error)
            }
            guard !Task.isCancelled else { return }
            withAnimation(.smooth) { spotify = found; spotifyError = failure; loading = false }
        }
    }
}

struct SearchResultsView: View {
    @Bindable var search: SearchModel
    @Environment(AppModel.self) private var model
    @State private var recents: [String] = []

    var body: some View {
        container
            .animation(.smooth, value: search.results.map(\.id))
            .onChange(of: search.query) { search.run(model) }
            .onChange(of: search.filter) { search.run(model) }
            .onAppear {
                recents = model.core.recentSearches()
                // A search link can set the query before this view exists, so no change fires.
                if !search.query.isEmpty && search.results.isEmpty { search.run(model) }
            }
            .onSubmit(of: .search) {
                model.core.addRecentSearch(query: search.query)
            }
    }

    #if os(macOS)
    /// The Mac draws the page itself: a centered column on the window's own background.
    private var container: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2) {
                if search.query.isEmpty {
                    recentSection
                } else {
                    chips.padding(.bottom, 10)
                    ForEach(search.results) { item in
                        result(item)
                    }
                    if search.loading {
                        ProgressView().controlSize(.small).frame(maxWidth: .infinity).padding(.vertical, 12)
                    } else if let e = search.spotifyError {
                        spotifyFailure(e).padding(.horizontal, 10).padding(.vertical, 12)
                    }
                }
            }
            .frame(maxWidth: 680)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, Metrics.page)
            .padding(.bottom, 120)
        }
    }
    #else
    private var container: some View {
        List {
            if search.query.isEmpty {
                recentSection
            } else {
                chips
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 4, leading: 16, bottom: 4, trailing: 0))
                ForEach(search.results) { item in
                    result(item).listRowSeparator(.hidden)
                }
                if search.loading {
                    HStack { Spacer(); ProgressView(); Spacer() }.listRowSeparator(.hidden)
                } else if let e = search.spotifyError {
                    spotifyFailure(e).listRowSeparator(.hidden)
                }
            }
        }
        .listStyle(.plain)
    }
    #endif

    private var chips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(SearchFilter.allCases) { f in
                    Button {
                        withAnimation(.snappy) { search.filter = f }
                    } label: {
                        Text(f.title)
                            .font(.subheadline.weight(search.filter == f ? .semibold : .regular))
                            .padding(.horizontal, 14)
                            .padding(.vertical, 7)
                            .foregroundStyle(search.filter == f ? AnyShapeStyle(.background) : AnyShapeStyle(.primary))
                            .background {
                                if search.filter == f { Capsule().fill(.primary) } else { Capsule().fill(.quaternary) }
                            }
                    }
                    .buttonStyle(.pressable)
                    .accessibilityIdentifier("search.filter.\(f.title)")
                    .accessibilityAddTraits(search.filter == f ? .isSelected : [])
                }
            }
            .padding(.trailing, 16)
        }
    }

    @ViewBuilder
    private var recentSection: some View {
        if recents.isEmpty {
            ContentUnavailableView("Search Spotify and your Uploads", systemImage: "magnifyingglass")
                .listRowSeparator(.hidden)
        } else {
            #if os(macOS)
            HStack {
                Text("Recent").font(.headline)
                Spacer()
                clearButton.buttonStyle(.plain).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 6)
            ForEach(recents, id: \.self) { q in
                Button { search.query = q } label: {
                    HStack(spacing: 10) {
                        Image(systemName: "clock.arrow.circlepath").foregroundStyle(.secondary)
                        Text(q).foregroundStyle(.primary)
                        Spacer(minLength: 0)
                    }
                }
                .buttonStyle(HoverRowStyle())
                .accessibilityIdentifier("recentSearch.\(q)")
            }
            #else
            Section {
                ForEach(recents, id: \.self) { q in
                    Button { search.query = q } label: {
                        Label {
                            Text(q).foregroundStyle(.primary)
                        } icon: {
                            Image(systemName: "clock.arrow.circlepath").foregroundStyle(.secondary)
                        }
                    }
                    .accessibilityIdentifier("recentSearch.\(q)")
                }
            } header: {
                HStack {
                    Text("Recent")
                    Spacer()
                    clearButton.font(.subheadline)
                }
            }
            #endif
        }
    }

    /// Spotify's half of the results didn't come: say why, rather than looking like it found nothing.
    private func spotifyFailure(_ message: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.circle")
            Text(message)
            #if os(iOS)
            if !model.account.spotifyWeb {
                Spacer(minLength: 0)
                Button("Connect") { model.showSettings = true }
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(.tint)
            }
            #endif
        }
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityIdentifier("search.spotifyError")
    }

    private var clearButton: some View {
        Button("Clear") {
            model.core.clearRecentSearches()
            withAnimation { recents = [] }
        }
    }

    @ViewBuilder
    private func result(_ item: Item) -> some View {
        Button {
            model.core.addRecentSearch(query: search.query)
            if item.kind == .track {
                model.play(item)
            } else {
                model.pendingNavigation = item
                model.openRequested &+= 1
            }
        } label: { ItemRow(item: item) }
            #if os(macOS)
            .buttonStyle(HoverRowStyle())
            #else
            .buttonStyle(.plain)
            #endif
            .accessibilityIdentifier("result.\(item.title)")
            .itemMenu(item)
    }
}

#if os(macOS)
/// A full-width row that lights up under the pointer.
struct HoverRowStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Row(configuration: configuration)
    }

    private struct Row: View {
        let configuration: ButtonStyleConfiguration
        @State private var hover = false

        var body: some View {
            configuration.label
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background {
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(.primary.opacity(configuration.isPressed ? 0.12 : hover ? 0.06 : 0))
                }
                .contentShape(Rectangle())
                .onHover { hover = $0 }
                .animation(.snappy(duration: 0.15), value: hover)
        }
    }
}
#endif
