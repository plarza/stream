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

    /// The same on iPhone and Mac: everything shown comes from the core's sign-in step.
    private var spotify: some View {
        let a = model.account
        let done = a.spotifyWeb && a.spotifyPlayback
        return Section {
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
                Button("Refresh library") {
                    Task { try? await model.core.refreshSpotify(force: true) }
                }
                Button("Sign out", role: .destructive) { model.core.spotifySignOut() }
                    .accessibilityIdentifier("settings.spotify.signOut")
            }
        } header: {
            Text("Spotify")
        } footer: {
            if !a.spotifyWeb {
                Text("Create an app at developer.spotify.com with redirect URI http://127.0.0.1:8989/login and the Web API enabled, then paste its Client ID. Signing in takes two approvals: your account, then playback. Playback needs Premium.")
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
