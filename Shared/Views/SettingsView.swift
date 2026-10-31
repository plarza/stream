import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var clientId = ""
    @State private var lastfmKey = ""
    @State private var lastfmSecret = ""
    @State private var settings = Preferences(normalization: true, crossfadeSeconds: 0, autoplay: false, bitPerfect: false, scrobbling: true)
    @State private var sync: SyncStatus?
    #if os(macOS)
    @State private var folders: [String] = []
    #endif

    var body: some View {
        @Bindable var model = model
        Form {
            spotify
            #if os(macOS)
            musicFolders
            #else
            syncSection
            #endif
            lastfm
            Section("Playback") {
                Toggle("Normalize loudness", isOn: $settings.normalization)
                Toggle("Scrobble local files to Last.fm", isOn: $settings.scrobbling)
            }
            .onChange(of: settings) { model.core.setSettings(s: settings) }
            Section("Appearance") {
                Picker("Appearance", selection: $model.appearance) {
                    ForEach(AppModel.Appearance.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier("settings.appearance")
                #if os(macOS)
                // iPhone's dark mode is already pure black.
                Toggle("Pure black in dark mode", isOn: $model.pureBlack)
                    .accessibilityIdentifier("settings.pureBlack")
                #endif
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Settings")
        .preferredColorScheme(model.colorScheme)
        .onAppear {
            clientId = model.account.spotifyClientId ?? ""
            settings = model.core.settings()
            #if os(macOS)
            folders = model.core.musicFolders()
            #endif
        }
        #if os(iOS)
        .task {
            while !Task.isCancelled {
                sync = model.core.syncStatus()
                try? await Task.sleep(for: .seconds(1))
            }
        }
        #endif
    }

    private var spotify: some View {
        Section {
            if model.account.spotifyWeb {
                LabeledContent("Account", value: model.account.spotifyUser ?? "Signed in")
                LabeledContent("Plan", value: model.account.spotifyPremium ? "Premium" : "Free (playback needs Premium)")
                LabeledContent("Playback", value: model.account.spotifyPlayback ? "Connected" : "Not connected")
                if !model.account.spotifyPlayback {
                    Button { model.signInToSpotify() } label: {
                        HStack {
                            Text("Connect playback")
                            if model.signingIn { Spacer(); ProgressView() }
                        }
                    }
                    .disabled(model.signingIn)
                }
                Button("Refresh library") {
                    Task { try? await model.core.refreshSpotify(force: true) }
                }
                Button("Sign out", role: .destructive) { model.core.spotifySignOut() }
            } else {
                TextField("Client ID", text: $clientId)
                    .autocorrectionDisabled()
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    #endif
                    .font(.body.monospaced())
                    .onSubmit { model.core.setSpotifyClientId(clientId: clientId) }
                Button {
                    model.core.setSpotifyClientId(clientId: clientId)
                    model.signInToSpotify()
                } label: {
                    HStack {
                        Text("Sign in with Spotify")
                        if model.signingIn { Spacer(); ProgressView() }
                    }
                }
                .disabled(clientId.trimmingCharacters(in: .whitespaces).isEmpty || model.signingIn)
            }
        } header: {
            Text("Spotify")
        } footer: {
            if !model.account.spotifyWeb {
                Text("Create an app at developer.spotify.com with redirect URI http://127.0.0.1:8989/login and Web API enabled, then paste its Client ID. Playback needs Premium.")
            }
        }
    }

    private var lastfm: some View {
        Section {
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
        } header: {
            Text("Last.fm")
        } footer: {
            Text("Only local files scrobble here. Spotify’s own Last.fm connection covers Spotify.")
        }
    }

    #if os(iOS)
    private var syncSection: some View {
        Section {
            if let s = sync, s.running {
                ProgressView(value: Double(s.bytesDone), total: Double(max(1, s.bytesTotal))) {
                    Text("Copying \(s.filesDone) of \(s.filesTotal)")
                } currentValueLabel: {
                    Text("\(Format.bytes(s.bytesDone)) of \(Format.bytes(s.bytesTotal))")
                }
            } else {
                Button("Sync now") { SyncClient.shared.syncNow() }
                if let e = sync?.lastError { Text(e).font(.footnote).foregroundStyle(.red) }
            }
        } header: {
            Text("Library from Mac")
        } footer: {
            Text("Your Mac’s music copies here automatically whenever Stream is open on the same Wi-Fi.")
        }
    }
    #endif

    #if os(macOS)
    private var musicFolders: some View {
        Section("Music folders") {
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
}
