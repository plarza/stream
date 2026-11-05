import StreamBindings
import AzaUI
import SwiftUI

/// Your account, Services (your music, Spotify, SoundCloud: their order, switches and each
/// one's own page), Last.fm, playback, then Appearance.
struct ProfileView: View {
    @Environment(AppModel.self) private var model
    @State private var clientId = ""
    @State private var lastfmKey = ""
    @State private var lastfmSecret = ""
    @State private var settings = Preferences(normalization: true, autoplay: true, scrobbling: true,
                                              services: [.files, .spotify, .soundCloud], servicesOff: [])
    @State private var soundCloudSignIn = false
    @State private var path: [Service] = []
    @State private var node: NodeStatus?
    @Environment(Account.self) private var aza
    @State private var refreshing = false
    #if os(macOS)
    @State private var folders: [String] = []
    #endif

    var body: some View {
        @Bindable var model = model
        NavigationStack(path: $path) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    AccountRow().padding(.top, 8)
                    SettingsCard("Services") {
                        ServicesList(settings: $settings, soundCloudSignIn: $soundCloudSignIn)
                    } footer: {
                        Text("Search and playback go in this order. A song on more than one plays from the first.")
                    }
                    lastfm
                    SettingsCard("Playback") {
                        Toggle("Normalize loudness", isOn: $settings.normalization)
                        Toggle("Autoplay similar songs", isOn: $settings.autoplay)
                            .accessibilityIdentifier("settings.autoplay")
                        Toggle("Scrobble to Last.fm", isOn: $settings.scrobbling)
                    }
                    heading("Appearance")
                    AppearanceSection()
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 30)
            }
            .onChange(of: settings) { model.core.setSettings(s: settings) }
            .onChange(of: model.settingsPage) {
                if let page = model.settingsPage { path = [page]; model.settingsPage = nil }
            }
            .onChange(of: model.settingsVersion) { settings = model.core.settings() }
            .navigationDestination(for: Service.self) { s in
                ServicePage(service: s) {
                    switch s {
                    case .files: filesPage
                    case .spotify: spotify
                    case .soundCloud: soundCloud
                    }
                }
            }
            .sheet(isPresented: $soundCloudSignIn) { SoundCloudSignInSheet() }
            .navigationTitle("Profile")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { model.showSettings = false }
                }
            }
            #endif
        }
        .onAppear {
            if let page = model.settingsPage { path = [page]; model.settingsPage = nil }
            clientId = model.account.spotifyClientId ?? ""
            settings = model.core.settings()
            #if os(macOS)
            folders = model.core.musicFolders()
            #endif
        }
        .task {
            while !Task.isCancelled {
                node = model.core.nodeStatus()
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    /// Whether a service is on: off, it stays signed in but leaves search, Library and playback.
    private func use(_ s: Service) -> some View {
        Toggle("Use \(s.title)", isOn: Binding(
            get: { !settings.servicesOff.contains(s) },
            set: { on in
                settings.servicesOff.removeAll { $0 == s }
                if !on { settings.servicesOff.append(s) }
            }))
            .accessibilityIdentifier("settings.use.\(s.title)")
    }

    /// Your own music: the Mac's folders and the Stream node they go up to.
    @ViewBuilder
    private var filesPage: some View {
        #if os(macOS)
        musicFolders
        #endif
        nodeSection
    }

    private var soundCloud: some View {
        SettingsCard("Account") {
            use(.soundCloud)
            if let user = model.account.soundcloudUser {
                LabeledContent("Account", value: user)
                    .accessibilityIdentifier("settings.soundcloud.account")
                Button("Sign out", role: .destructive) { model.core.soundcloudSignOut() }
                    .accessibilityIdentifier("settings.soundcloud.signOut")
            } else {
                Button("Sign in to SoundCloud") { soundCloudSignIn = true }
                    .accessibilityIdentifier("settings.soundcloud.signIn")
            }
        } footer: {
            Text(model.account.soundcloudUser == nil
                 ? "Search and play work without an account. Sign in for your likes and playlists, and to keep playlists there."
                 : "Songs a label has only made available encrypted play from Spotify or your files instead.")
        }
    }

    /// The same on iPhone and Mac: everything shown comes from the core's sign-in step.
    private var spotify: some View {
        let a = model.account
        let done = a.spotifyWeb && a.spotifyPlayback
        return SettingsCard("Account") {
            if a.spotifyWeb { use(.spotify) }
            if a.spotifyWeb {
                LabeledContent("Account", value: a.spotifyUser ?? "Signed in")
                    .accessibilityIdentifier("settings.spotify.account")
                LabeledContent("Plan", value: a.spotifyPremium ? "Premium" : "Free (playback needs Premium)")
            } else {
                TextField("Client ID", text: $clientId)
                    .autocorrectionDisabled()
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    #endif
                    .font(.body.monospaced())
                    .onSubmit { model.core.setSpotifyClientId(clientId: clientId) }
                    .disabled(model.signingIn)
                    .accessibilityIdentifier("settings.spotify.clientId")
            }
            if !done || model.signingIn {
                signInSteps(a)
                signInButton(a)
            } else {
                LabeledContent("Playback", value: "Connected")
                    .accessibilityIdentifier("settings.spotify.playback")
                Button {
                    refreshing = true
                    Task {
                        await model.refreshSpotify()
                        refreshing = false
                        model.show("Library up to date")
                    }
                } label: {
                    HStack(spacing: 8) {
                        Text("Refresh library")
                        if refreshing { ProgressView().controlSize(.small) }
                    }
                }
                .disabled(refreshing)
                Button("Sign out", role: .destructive) { model.core.spotifySignOut() }
                    .accessibilityIdentifier("settings.spotify.signOut")
            }
        } footer: {
            if !a.spotifyWeb {
                Text("developer.spotify.com app · redirect http://127.0.0.1:8989/login · Web API on · Premium for playback")
            }
        }
    }

    /// Two approvals and a connection, each marked as it goes.
    @ViewBuilder
    private func signInSteps(_ a: AccountState) -> some View {
        let step: Int = switch a.signIn {
        case let .approving(s): Int(s)
        case .connecting: 3
        default: 0
        }
        stepRow("Spotify account", done: a.spotifyWeb, active: step == 1, hint: "Approve in the Spotify sheet")
        stepRow("Playback", done: a.spotifyPlayback, active: step == 2 || step == 3,
                hint: step == 3 ? "Connecting to Spotify" : "Approve in the Spotify sheet")
        if case let .failed(message) = a.signIn {
            Label(message, systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
                .font(.callout)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(message)
                .accessibilityIdentifier("settings.spotify.error")
        }
    }

    private func stepRow(_ title: String, done: Bool, active: Bool, hint: String) -> some View {
        HStack(spacing: 10) {
            ZStack {
                if done {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                } else if active {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: "circle").foregroundStyle(.tertiary)
                }
            }
            .frame(width: 22)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                if active && !done {
                    Text(hint).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 0)
        }
        .animation(.snappy, value: done)
        .animation(.snappy, value: active)
        .accessibilityElement(children: .combine)
        .accessibilityValue(done ? "done" : active ? hint : "to do")
        .accessibilityIdentifier("settings.spotify.step.\(title)")
    }

    @ViewBuilder
    private func signInButton(_ a: AccountState) -> some View {
        if model.signingIn {
            Button("Cancel", role: .cancel) { model.core.cancelSignIn() }
                .accessibilityIdentifier("settings.spotify.cancel")
        } else {
            let failed = if case .failed = a.signIn { true } else { false }
            Button(failed ? "Try again" : a.spotifyWeb ? "Connect playback" : "Sign in with Spotify") {
                if !a.spotifyWeb { model.core.setSpotifyClientId(clientId: clientId) }
                model.signInToSpotify()
            }
            .disabled(!a.spotifyWeb && clientId.trimmingCharacters(in: .whitespaces).isEmpty)
            .accessibilityIdentifier("settings.spotify.signIn")
        }
    }

    private var lastfm: some View {
        SettingsCard("Last.fm") {
            if let user = model.account.lastfmUser {
                LabeledContent("Scrobbling as", value: user)
                Button("Sign out", role: .destructive) { model.core.lastfmSignOut() }
            } else {
                if !model.core.hasLastfmApi() {
                    TextField("API key", text: $lastfmKey).font(.body.monospaced()).autocorrectionDisabled()
                    SecureField("Shared secret", text: $lastfmSecret).font(.body.monospaced())
                }
                Button("Sign in to Last.fm") {
                    if !lastfmKey.isEmpty { model.core.setLastfmApi(key: lastfmKey, secret: lastfmSecret) }
                    Task {
                        do { try await model.core.lastfmSignIn() } catch { model.show(error.localizedDescription) }
                    }
                }
                .disabled(!model.core.hasLastfmApi() && (lastfmKey.isEmpty || lastfmSecret.isEmpty))
            }
        }
    }

    /// Your own music on the Stream node: how much room it takes, what's still going up, and the
    /// songs here from your other devices.
    private var nodeSection: some View {
        SettingsCard("Stream node") {
            if aza.client == nil {
                Text("Sign in with aza to use it")
                    .foregroundStyle(.secondary)
            } else if let n = node, n.connected || n.quota > 0 {
                if n.quota > 0 {
                    ProgressView(value: Double(min(n.used, n.quota)), total: Double(n.quota)) {
                        Text("Storage")
                    } currentValueLabel: {
                        Text("\(Format.bytes(n.used)) of \(Format.bytes(n.quota))").monospacedDigit()
                    }
                    .accessibilityIdentifier("node.storage")
                }
                if n.uploading {
                    ProgressView(value: Double(n.bytesDone), total: Double(max(1, n.bytesTotal))) {
                        Text("Uploading \(n.filesDone) of \(n.filesTotal)")
                    } currentValueLabel: {
                        Text("\(Format.bytes(n.bytesDone)) of \(Format.bytes(n.bytesTotal))").monospacedDigit()
                    }
                }
                if n.songs > 0 {
                    LabeledContent("From your other devices") {
                        Text(n.downloaded > 0 ? "\(n.songs) songs, \(n.downloaded) downloaded" : "\(n.songs) songs")
                            .monospacedDigit()
                    }
                }
                if let p = n.problem { Text(p).font(.footnote).foregroundStyle(.red) }
            } else if let p = node?.problem {
                Text(p).font(.footnote).foregroundStyle(.red)
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Connecting").foregroundStyle(.secondary)
                }
            }
        }
    }

    #if os(macOS)
    private var musicFolders: some View {
        SettingsCard("Music folders") {
            ForEach(folders, id: \.self) { f in
                HStack {
                    Label(f, systemImage: "folder")
                    Spacer()
                    Button(role: .destructive) {
                        folders.removeAll { $0 == f }
                        model.core.setMusicFolders(folders: folders)
                        FolderWatcher.shared.watch(folders)
                    } label: { Image(systemName: "minus.circle") }
                        .buttonStyle(.borderless)
                }
            }
            Button("Add Folder…") {
                let panel = NSOpenPanel()
                panel.canChooseDirectories = true
                panel.canChooseFiles = false
                panel.allowsMultipleSelection = true
                if panel.runModal() == .OK {
                    folders.append(contentsOf: panel.urls.map(\.path).filter { !folders.contains($0) })
                    model.core.setMusicFolders(folders: folders)
                    FolderWatcher.shared.watch(folders)
                }
            }
        }
    }
    #endif

    private func heading(_ text: String) -> some View {
        Text(text)
            .font(.title3.weight(.bold))
            .padding(.leading, 4)
            .padding(.top, 28)
            .padding(.bottom, 8)
            .accessibilityAddTraits(.isHeader)
    }
}

/// A titled glass card of settings, with an optional note below.
private struct SettingsCard<Content: View, Footer: View>: View {
    let title: String
    @ViewBuilder let content: Content
    @ViewBuilder let footer: Footer

    init(_ title: String, @ViewBuilder content: () -> Content, @ViewBuilder footer: () -> Footer) {
        self.title = title
        self.content = content()
        self.footer = footer()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.title3.weight(.bold))
                .padding(.leading, 4)
                .padding(.top, 20)
                .accessibilityAddTraits(.isHeader)
            VStack(alignment: .leading, spacing: 14) { content }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
                .glassEffect(.regular, in: .rect(cornerRadius: 24))
            footer
                .font(.footnote)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 4)
        }
    }
}

extension SettingsCard where Footer == EmptyView {
    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.init(title, content: content) { EmptyView() }
    }
}
