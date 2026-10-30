import AppKit
import SwiftUI

@main
struct MusicMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel.shared
    @State private var palette = false

    var body: some Scene {
        Window("Music", id: "main") {
            MacRootView(showPalette: $palette)
                .environment(model)
                .frame(minWidth: 820, minHeight: 560)
                .onContinueUserActivity("network.aza.music.playing") { model.continueActivity($0) }
                .onOpenURL { model.handle(url: $0) }
                .onAppear { NSApp.setActivationPolicy(.regular) }
                .onDisappear {
                    // Closing the window keeps music playing from the menu bar.
                    NSApp.setActivationPolicy(.accessory)
                }
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandMenu("Playback") {
                Button(model.player.status == .playing ? "Pause" : "Play") { model.toggle() }
                    .keyboardShortcut(.space, modifiers: [])
                Button("Next") { model.next() }.keyboardShortcut(.rightArrow, modifiers: .command)
                Button("Previous") { model.previous() }.keyboardShortcut(.leftArrow, modifiers: .command)
                Divider()
                Button("Now Playing") { withAnimation(.player) { model.showNowPlaying.toggle() } }
                    .keyboardShortcut("p", modifiers: [.command, .shift])
                Button("Lyrics") { showMode(.lyrics) }.keyboardShortcut("l", modifiers: .command)
                Button("Queue") { showMode(.queue) }.keyboardShortcut("u", modifiers: .command)
                Divider()
                Button("Go to…") { palette = true }.keyboardShortcut("k", modifiers: .command)
            }
        }

        Settings {
            SettingsView().environment(model).frame(width: 520, height: 560)
        }

        MenuBarExtra {
            MenuBarCard().environment(model)
        } label: {
            Image(systemName: model.player.status == .playing ? "music.note" : "music.note")
        }
        .menuBarExtraStyle(.window)
    }

    private func showMode(_ m: AppModel.NowPlayingMode) {
        withAnimation(.player) {
            if model.showNowPlaying && model.nowPlayingMode == m {
                model.nowPlayingMode = .artwork
            } else {
                model.nowPlayingMode = m
                model.showNowPlaying = true
            }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationDidFinishLaunching(_ notification: Notification) {
        MainActor.assumeIsolated {
            let model = AppModel.shared
            FolderWatcher.shared.watch(model.core.musicFolders())
            SyncAdvertiser.shared.start(port: model.core.startSyncServer())
        }
    }
}

/// Publishes the library server so iPhones on the network find it.
@MainActor
final class SyncAdvertiser: NSObject, NetServiceDelegate {
    static let shared = SyncAdvertiser()
    private var service: NetService?

    func start(port: UInt16) {
        let name = Host.current().localizedName ?? "Mac"
        let s = NetService(domain: "local.", type: "_musicsync._tcp.", name: name, port: Int32(port))
        s.publish()
        service = s
    }
}

/// Rescans when files change under the music folders (FSEvents, debounced).
final class FolderWatcher: @unchecked Sendable {
    static let shared = FolderWatcher()
    private var stream: FSEventStreamRef?
    private var pending: DispatchWorkItem?

    func watch(_ folders: [String]) {
        if let s = stream {
            FSEventStreamStop(s)
            FSEventStreamInvalidate(s)
            FSEventStreamRelease(s)
            stream = nil
        }
        guard !folders.isEmpty else { return }
        var ctx = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(), retain: nil, release: nil, copyDescription: nil)
        let cb: FSEventStreamCallback = { _, info, _, _, _, _ in
            guard let info else { return }
            Unmanaged<FolderWatcher>.fromOpaque(info).takeUnretainedValue().changed()
        }
        guard let s = FSEventStreamCreate(nil, cb, &ctx, folders as CFArray, FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 2.0,
                                          FSEventStreamCreateFlags(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer)) else { return }
        FSEventStreamSetDispatchQueue(s, .main)
        FSEventStreamStart(s)
        stream = s
    }

    private func changed() {
        pending?.cancel()
        let work = DispatchWorkItem { MainActor.assumeIsolated { AppModel.shared.core.rescan() } }
        pending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: work)
    }
}
