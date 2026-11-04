import StreamBindings
import AppKit
import AzaUI
import SwiftUI

@main
struct StreamMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel.shared
    @State private var palette = false
    @AppStorage(Zoom.key) private var zoom = Zoom.standard

    init() {
        // "Pure black in dark mode" was its own switch; it's the OLED Dark theme now.
        let d = UserDefaults.standard
        if d.bool(forKey: "pureBlack") { d.set("oledDark", forKey: Appearance.key) }
        d.removeObject(forKey: "pureBlack")
    }

    var body: some Scene {
        Window("Stream", id: "main") {
            Zoomed(zoom: zoom) {
                MacRootView(showPalette: $palette)
                    .environment(model)
                    .azaUI(name: "Stream", accent: "FF375F", appID: StreamSync.appID, scopes: ["sign-in", "read", "write"])
            }
            .frame(minWidth: 900 * zoom, minHeight: 560 * zoom)
                .onContinueUserActivity("network.aza.stream.playing") { model.continueActivity($0) }
                .onOpenURL { model.handle(url: $0) }
                .onAppear { NSApp.setActivationPolicy(.regular) }
                .onDisappear {
                    // Closing the window keeps music playing from the menu bar.
                    NSApp.setActivationPolicy(.accessory)
                }
        }
        // A toolbar row (title hidden, background clear) puts the traffic lights
        // where macOS puts them in its own apps, inside the floating sidebar.
        .windowToolbarStyle(.unified(showsTitle: false))
        .defaultSize(width: 1240, height: 800)
        // Always open the window at launch, even if it was closed last time.
        .defaultLaunchBehavior(.presented)
        .restorationBehavior(.disabled)
        .commands {
            CommandGroup(after: .toolbar) {
                Button("Zoom In") { zoom = Zoom.next(zoom, 1) }.keyboardShortcut("+", modifiers: .command)
                Button("Zoom Out") { zoom = Zoom.next(zoom, -1) }.keyboardShortcut("-", modifiers: .command)
                Button("Actual Size") { zoom = Zoom.standard }.keyboardShortcut("0", modifiers: .command)
                Divider()
            }
            CommandMenu("Playback") {
                Button(model.player.status == .playing ? "Pause" : "Play") { model.toggle() }
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
            ProfileView().environment(model).frame(width: 520, height: 640)
                .azaUI(name: "Stream", accent: "FF375F", appID: StreamSync.appID, scopes: ["sign-in", "read", "write"], showsSignIn: false)
        }

        MenuBarExtra {
            MenuBarCard().environment(model)
                .azaUI(name: "Stream", accent: "FF375F", appID: StreamSync.appID, scopes: ["sign-in", "read", "write"], showsSignIn: false)
        } label: {
            MenuBarLabel()
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

/// The menu bar icon. It exists from launch, so it also makes sure the main
/// window is up: SwiftUI sometimes launches with no window at all.
struct MenuBarLabel: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Image(systemName: "music.note")
            .task {
                try? await Task.sleep(for: .milliseconds(600))
                let visible = NSApp.windows.contains { $0.isVisible && $0.identifier?.rawValue.hasPrefix("main") == true }
                if !visible {
                    openWindow(id: "main")
                    NSApp.activate()
                }
            }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationDidFinishLaunching(_ notification: Notification) {
        MainActor.assumeIsolated {
            NSApp.setActivationPolicy(.regular)
            let model = AppModel.shared
            // Space plays/pauses anywhere except while typing; ⌘= zooms in like ⌘+.
            NSEvent.addLocalMonitorForEvents(matching: .keyDown) { e in
                if e.charactersIgnoringModifiers == "=", e.modifierFlags.intersection([.command, .option, .control, .shift]) == .command {
                    let d = UserDefaults.standard
                    d.set(Zoom.next(d.object(forKey: Zoom.key) as? Double ?? Zoom.standard, 1), forKey: Zoom.key)
                    return nil
                }
                let typing = NSApp.keyWindow?.firstResponder is NSText
                guard e.keyCode == 49, e.modifierFlags.intersection([.command, .option, .control, .shift]).isEmpty, !typing else { return e }
                MainActor.assumeIsolated { model.toggle() }
                return nil
            }
            guard !AppModel.isTesting else { return }
            FolderWatcher.shared.watch(model.core.musicFolders())
            StreamSync.shared.start()
        }
    }

    /// Coming back to the Mac picks up what another device played meanwhile.
    func applicationDidBecomeActive(_ notification: Notification) {
        MainActor.assumeIsolated { StreamSync.shared.run() }
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
