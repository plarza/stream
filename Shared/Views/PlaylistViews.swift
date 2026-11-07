import StreamBindings
import SwiftUI

extension Service: Identifiable {
    public var id: Self { self }

    var title: String {
        switch self {
        case .files: "Your music"
        case .spotify: "Spotify"
        case .soundCloud: "SoundCloud"
        }
    }
}

extension Item {
    /// One of your own playlists (kept with aza), rather than a Spotify or SoundCloud one.
    var isOwnPlaylist: Bool { id.hasPrefix("stream:playlist:") }
}

/// What "New Playlist" was asked for with: songs to start it with.
struct NewPlaylistRequest: Identifiable {
    let id = UUID()
    var tracks: [Track] = []
    /// From a Spotify or SoundCloud playlist's link instead of a name.
    var importing = false
}

/// Name it, and pick where else it's kept. It's always kept with aza.
struct NewPlaylistSheet: View {
    let request: NewPlaylistRequest
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var link = ""
    @State private var copies: Set<Service> = []
    @State private var working = false
    @FocusState private var focused: Bool

    private var ready: Bool {
        !(request.importing ? link : name).trimmingCharacters(in: .whitespaces).isEmpty && !working
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    if request.importing {
                        TextField("Spotify or SoundCloud link", text: $link)
                            .focused($focused)
                            .autocorrectionDisabled()
                            #if os(iOS)
                            .textInputAutocapitalization(.never)
                            .keyboardType(.URL)
                            #endif
                            .onSubmit(create)
                            .accessibilityIdentifier("newPlaylist.link")
                    } else {
                        TextField("Playlist name", text: $name)
                            .focused($focused)
                            .onSubmit(create)
                            .accessibilityIdentifier("newPlaylist.name")
                    }
                } footer: {
                    if request.importing {
                        Text("A playlist or album. It becomes your own, with the same songs and name.")
                    } else if !request.tracks.isEmpty {
                        Text(request.tracks.count == 1 ? "Starts with “\(request.tracks[0].title)”" : "Starts with \(request.tracks.count) songs")
                    }
                }
                Section {
                    ForEach([Service.spotify, .soundCloud]) { s in
                        let connected = model.connected(s)
                        Toggle(isOn: Binding(get: { copies.contains(s) }, set: { on in
                            if on { copies.insert(s) } else { copies.remove(s) }
                        })) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(s.title)
                                if !connected { Text(s == .soundCloud ? "Not signed in" : "Not connected").font(.caption).foregroundStyle(.secondary) }
                            }
                        }
                        .disabled(!connected)
                        .accessibilityIdentifier("newPlaylist.\(s.title)")
                    }
                } header: {
                    Text("Also keep on")
                } footer: {
                    Text("The same songs on each, found by matching their tags. Changes made there come back here.")
                }
            }
            .formStyle(.grouped)
            .navigationTitle(request.importing ? "Import Playlist" : "New Playlist")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", role: .cancel) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if working {
                        ProgressView().controlSize(.small)
                    } else {
                        Button(request.importing ? "Import" : "Create", action: create)
                            .disabled(!ready)
                            .accessibilityIdentifier("newPlaylist.create")
                    }
                }
            }
        }
        #if os(macOS)
        .frame(width: 420, height: 360)
        #endif
        .onAppear {
            focused = true
            // A link just copied is most likely the one to import.
            if request.importing, let copied = Clipboard.string, copied.contains("spotify") || copied.contains("soundcloud") {
                link = copied.trimmingCharacters(in: .whitespacesAndNewlines)
            }
        }
    }

    private func create() {
        guard ready else { return }
        if request.importing {
            working = true
            Task {
                do {
                    let item = try await model.core.importPlaylist(link: link, copies: Array(copies))
                    Haptics.success()
                    dismiss()
                    model.show("Imported “\(item.title)”")
                    model.pendingNavigation = item
                    model.openRequested &+= 1
                } catch {
                    working = false
                    model.show(AppModel.message(error))
                }
            }
            return
        }
        let item = model.core.createPlaylist(name: name, copies: Array(copies), tracks: request.tracks)
        Haptics.success()
        dismiss()
        model.show("Created “\(item.title)”")
    }
}

/// "Add to Playlist": your playlists, newest first, and a new one. Drawn as a menu item
/// unless it's given its own label (the add button beside what's playing).
struct AddToPlaylistMenu<Content: View>: View {
    let tracks: () async -> [Track]
    let label: Content
    @Environment(AppModel.self) private var model

    init(tracks: @escaping () async -> [Track], @ViewBuilder label: () -> Content) {
        self.tracks = tracks
        self.label = label()
    }

    var body: some View {
        Menu {
            Button { Task { model.newPlaylist = NewPlaylistRequest(tracks: await tracks()) } } label: {
                Label("New Playlist…", systemImage: "plus")
            }
            .accessibilityIdentifier("menu.newPlaylist")
            let lists = model.core.ownPlaylists()
            if !lists.isEmpty {
                Divider()
                ForEach(lists) { p in
                    Button(p.title) {
                        Task {
                            let t = await tracks()
                            model.core.addToPlaylist(id: p.id, tracks: t)
                            Haptics.success()
                            model.show("Added to “\(p.title)”")
                        }
                    }
                }
            }
        } label: {
            label
        }
    }
}

extension AddToPlaylistMenu where Content == Label<Text, Image> {
    init(tracks: @escaping () async -> [Track]) {
        self.init(tracks: tracks) { Label("Add to Playlist", systemImage: "text.badge.plus") }
    }
}

/// Under your playlist's header: where else it's kept, and how much of it each has.
struct PlaylistCopiesRow: View {
    let id: String
    @Environment(AppModel.self) private var model
    @State private var copies: [PlaylistCopy] = []

    var body: some View {
        HStack(spacing: 6) {
            chip(symbol: "checkmark.icloud", text: "aza", tint: Color.secondary)
            ForEach(copies.filter(\.on), id: \.service) { c in
                let missing = c.found < c.total
                chip(
                    symbol: c.problem != nil ? "exclamationmark.triangle" : c.syncing ? "arrow.triangle.2.circlepath" : missing ? "circle.dashed" : "checkmark",
                    text: missing && c.problem == nil ? "\(c.service.title) \(c.found)/\(c.total)" : c.service.title,
                    tint: c.problem != nil ? Color.red : Color.secondary
                )
                .help(c.problem ?? (missing ? "\(c.total - c.found) songs aren’t on \(c.service.title)" : "The same on \(c.service.title)"))
                .symbolEffect(.rotate, isActive: c.syncing)
            }
        }
        .task(id: model.libraryVersion) { copies = model.core.playlistCopies(id: id) }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("playlist.copies")
    }

    private func chip(symbol: String, text: String, tint: Color) -> some View {
        Label(text, systemImage: symbol)
            .font(.caption2.weight(.semibold))
            .monospacedDigit()
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.quaternary.opacity(0.6), in: .capsule)
    }
}

/// Your playlist's "…": rename, where else it's kept, delete.
struct OwnPlaylistMenu: View {
    let item: Item
    var deleted: () -> Void
    @Environment(AppModel.self) private var model
    @State private var renaming = false
    @State private var confirmDelete = false
    @State private var name = ""

    var body: some View {
        let copies = model.core.playlistCopies(id: item.id)
        Menu {
            Button { name = item.title; renaming = true } label: { Label("Rename", systemImage: "pencil") }
            Section("Also keep on") {
                ForEach(copies, id: \.service) { c in
                    let connected = model.connected(c.service)
                    Toggle(isOn: Binding(get: { c.on }, set: { model.core.setPlaylistCopy(id: item.id, service: c.service, on: $0) })) {
                        Text(c.service.title)
                        if !connected { Text(c.service == .soundCloud ? "Not signed in" : "Not connected") }
                        else if c.on, c.found < c.total { Text("\(c.found) of \(c.total) songs found") }
                    }
                    .disabled(!c.on && !connected)
                }
            }
            Divider()
            Button(role: .destructive) { confirmDelete = true } label: { Label("Delete Playlist", systemImage: "trash") }
        } label: {
            Image(systemName: "ellipsis").font(.title3)
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .menuIndicator(.hidden)
        .buttonStyle(.plain)
        .accessibilityLabel("More")
        .accessibilityIdentifier("playlist.more")
        .alert("Rename Playlist", isPresented: $renaming) {
            TextField("Name", text: $name)
            Button("Cancel", role: .cancel) {}
            Button("Rename") { model.core.renamePlaylist(id: item.id, name: name) }
        }
        .confirmationDialog("Delete “\(item.title)”?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete Playlist", role: .destructive) {
                model.core.deletePlaylist(id: item.id)
                deleted()
            }
        } message: {
            Text("It goes from all your devices. Copies on Spotify and SoundCloud stay there.")
        }
    }
}

extension AppModel {
    /// Signed in to `service`, so a playlist can be kept there.
    func connected(_ service: Service) -> Bool {
        switch service {
        case .files: true
        case .spotify: account.spotifyWeb
        case .soundCloud: account.soundcloudUser != nil
        }
    }
}

/// The system clipboard's text, on either platform.
enum Clipboard {
    static var string: String? {
        #if os(macOS)
        NSPasteboard.general.string(forType: .string)
        #else
        UIPasteboard.general.hasStrings ? UIPasteboard.general.string : nil
        #endif
    }
}

/// Library → Playlists' +: a new playlist, or one from a link.
struct NewPlaylistMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Menu {
            Button("New Playlist…", systemImage: "plus") { model.newPlaylist = NewPlaylistRequest() }
                .accessibilityIdentifier("library.newPlaylist.blank")
            Button("Import from Link…", systemImage: "link") { model.newPlaylist = NewPlaylistRequest(importing: true) }
                .accessibilityIdentifier("library.newPlaylist.import")
        } label: {
            Label("New Playlist", systemImage: "plus")
        }
        .accessibilityIdentifier("library.newPlaylist")
    }
}
