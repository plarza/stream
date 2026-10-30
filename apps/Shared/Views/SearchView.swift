import SwiftUI

extension SearchFilter: Identifiable, CaseIterable {
    public var id: Self { self }
    public static var allCases: [SearchFilter] { [.all, .songs, .albums, .artists, .playlists] }
    var title: String {
        switch self {
        case .all: "All"
        case .songs: "Songs"
        case .albums: "Albums"
        case .artists: "Artists"
        case .playlists: "Playlists"
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
            library = []; spotify = []; loading = false
            return
        }
        library = model.core.searchLibrary(query: q, filter: filter)
        let filter = self.filter
        task = Task {
            try? await Task.sleep(for: .milliseconds(220))
            guard !Task.isCancelled else { return }
            loading = true
            let found = (try? await model.core.searchSpotify(query: q, filter: filter)) ?? []
            guard !Task.isCancelled else { return }
            withAnimation(.smooth) { spotify = found; loading = false }
        }
    }
}

struct SearchResultsView: View {
    @Bindable var search: SearchModel
    @Environment(AppModel.self) private var model
    @State private var recents: [String] = []

    var body: some View {
        List {
            if search.query.isEmpty {
                recentSection
            } else {
                chips
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 4, leading: 16, bottom: 4, trailing: 0))
                ForEach(search.results) { item in
                    result(item)
                }
                if search.loading {
                    HStack { Spacer(); ProgressView(); Spacer() }.listRowSeparator(.hidden)
                }
            }
        }
        .listStyle(.plain)
        .animation(.smooth, value: search.results.map(\.id))
        .onChange(of: search.query) { search.run(model) }
        .onChange(of: search.filter) { search.run(model) }
        .onAppear { recents = model.core.recentSearches() }
        .onSubmit(of: .search) {
            model.core.addRecentSearch(query: search.query)
        }
    }

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
                            .foregroundStyle(search.filter == f ? Color.black : .primary)
                            .background {
                                if search.filter == f { Capsule().fill(.white) } else { Capsule().fill(.quaternary) }
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
            ContentUnavailableView("Search your library", systemImage: "magnifyingglass")
                .listRowSeparator(.hidden)
        } else {
            Section {
                ForEach(recents, id: \.self) { q in
                    Button { search.query = q } label: {
                        Label(q, systemImage: "clock.arrow.circlepath").foregroundStyle(.primary)
                    }
                    .accessibilityIdentifier("recentSearch.\(q)")
                }
            } header: {
                HStack {
                    Text("Recent")
                    Spacer()
                    Button("Clear") {
                        model.core.clearRecentSearches()
                        withAnimation { recents = [] }
                    }
                    .font(.subheadline)
                }
            }
        }
    }

    @ViewBuilder
    private func result(_ item: Item) -> some View {
        if item.kind == .track {
            Button {
                model.core.addRecentSearch(query: search.query)
                model.play(item)
            } label: { ItemRow(item: item) }
                .buttonStyle(.plain)
                .accessibilityIdentifier("result.\(item.title)")
                .itemMenu(item)
        } else {
            Button {
                model.core.addRecentSearch(query: search.query)
                model.pendingNavigation = item
                model.openRequested &+= 1
            } label: { ItemRow(item: item) }
                .buttonStyle(.plain)
                .accessibilityIdentifier("result.\(item.title)")
                .itemMenu(item)
        }
    }
}
