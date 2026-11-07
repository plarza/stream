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
    /// Instant results from what's stored here.
    var library: [Item] = []
    /// Everything, from every service that's on, in your order (Settings → Services).
    var everything: [Item]?
    var loading = false
    /// A service that couldn't answer, and why, rather than looking like it found nothing.
    var problem: ServiceProblem?
    private var task: Task<Void, Never>?

    var results: [Item] { everything ?? library }

    func run(_ model: AppModel) {
        task?.cancel()
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else {
            library = []; everything = nil; loading = false; problem = nil
            return
        }
        library = model.core.searchLibrary(query: q, filter: filter)
        everything = nil
        let filter = self.filter
        task = Task {
            try? await Task.sleep(for: .milliseconds(220))
            guard !Task.isCancelled else { return }
            loading = true
            let found = await model.core.search(query: q, filter: filter)
            guard !Task.isCancelled else { return }
            withAnimation(.smooth) { everything = found.items; problem = found.problems.first; loading = false }
        }
    }
}

struct SearchResultsView: View {
    @Bindable var search: SearchModel
    @Environment(AppModel.self) private var model
    @State private var recents: [Item] = []

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
                    } else if let p = search.problem {
                        failure(p).padding(.horizontal, 10).padding(.vertical, 12)
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
                } else if let p = search.problem {
                    failure(p).listRowSeparator(.hidden)
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

    /// Before you type: what you picked from results, like Spotify, not what you typed.
    @ViewBuilder
    private var recentSection: some View {
        if recents.isEmpty {
            ContentUnavailableView("Search your music, Spotify and SoundCloud", systemImage: "magnifyingglass")
                .listRowSeparator(.hidden)
        } else {
            #if os(macOS)
            HStack {
                Text("Recent searches").font(.headline)
                Spacer()
                clearButton.buttonStyle(.plain).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 6)
            ForEach(recents, id: \.id) { item in
                Button { pick(item) } label: {
                    HStack(spacing: 0) {
                        ItemRow(item: item)
                        Color.clear.frame(width: 36, height: 1)
                    }
                }
                .buttonStyle(HoverRowStyle())
                .overlay(alignment: .trailing) { removeButton(item).padding(.trailing, 10) }
                .accessibilityIdentifier("recentSearch.\(item.title)")
                .itemMenu(item)
            }
            #else
            Section {
                ForEach(recents, id: \.id) { item in
                    HStack(spacing: 4) {
                        Button { pick(item) } label: { ItemRow(item: item) }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("recentSearch.\(item.title)")
                        removeButton(item)
                    }
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets(top: 6, leading: 16, bottom: 6, trailing: 8))
                    .itemMenu(item)
                    .swipeActions { Button("Remove", role: .destructive) { remove(item) } }
                }
            } header: {
                HStack {
                    Text("Recent searches")
                    Spacer()
                    clearButton.font(.subheadline)
                }
            }
            #endif
        }
    }

    private func removeButton(_ item: Item) -> some View {
        Button { remove(item) } label: {
            Image(systemName: "xmark")
                .font(.body.weight(.medium))
                .foregroundStyle(.secondary)
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        #if os(macOS)
        .buttonStyle(.plain)
        .frame(width: 28, height: 28)
        #else
        .buttonStyle(.borderless)
        .tint(.secondary)
        #endif
        .accessibilityLabel("Remove \(item.title)")
        .accessibilityIdentifier("recentSearch.remove.\(item.title)")
    }

    private func remove(_ item: Item) {
        model.core.removeRecentSearch(id: item.id)
        withAnimation(.smooth) { recents.removeAll { $0.id == item.id } }
    }

    /// Plays a song or opens anything else, and puts it at the top of recent searches.
    private func pick(_ item: Item) {
        model.core.addRecentSearch(item: item)
        if item.kind == .track {
            model.play(item)
        } else {
            model.pendingNavigation = item
            model.openRequested &+= 1
        }
        let picked = item
        withAnimation(.smooth) { recents = [picked] + recents.filter { $0.id != picked.id } }
    }

    /// A service's results didn't come: say why, rather than looking like it found nothing.
    private func failure(_ problem: ServiceProblem) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.circle")
            Text(problem.message)
            #if os(iOS)
            if problem.service == .spotify, !model.account.spotifyWeb {
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
        .accessibilityIdentifier("search.problem")
    }

    private var clearButton: some View {
        Button("Clear") {
            model.core.clearRecentSearches()
            withAnimation { recents = [] }
        }
    }

    @ViewBuilder
    private func result(_ item: Item) -> some View {
        Button { pick(item) } label: { ItemRow(item: item) }
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
