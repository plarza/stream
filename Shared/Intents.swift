import StreamBindings
import AppIntents

/// Siri and Shortcuts.

struct PlayPauseIntent: AppIntent {
    static let title: LocalizedStringResource = "Play or Pause"
    @MainActor func perform() async throws -> some IntentResult {
        AppModel.shared.core.toggle()
        return .result()
    }
}

struct NextTrackIntent: AppIntent {
    static let title: LocalizedStringResource = "Next Song"
    @MainActor func perform() async throws -> some IntentResult {
        AppModel.shared.core.next()
        return .result()
    }
}

struct PreviousTrackIntent: AppIntent {
    static let title: LocalizedStringResource = "Previous Song"
    @MainActor func perform() async throws -> some IntentResult {
        AppModel.shared.core.previous()
        return .result()
    }
}

/// Pinned albums, artists, playlists and songs, for "Play <pin>".
struct PinEntity: AppEntity {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Pinned Item"
    static let defaultQuery = PinQuery()
    let id: String
    let title: String
    var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(title)") }
}

struct PinQuery: EntityStringQuery {
    @MainActor func entities(for identifiers: [String]) async throws -> [PinEntity] {
        AppModel.shared.core.home().pins.filter { identifiers.contains($0.id) }.map { PinEntity(id: $0.id, title: $0.title) }
    }

    @MainActor func entities(matching string: String) async throws -> [PinEntity] {
        AppModel.shared.core.home().pins
            .filter { $0.title.localizedCaseInsensitiveContains(string) }
            .map { PinEntity(id: $0.id, title: $0.title) }
    }

    @MainActor func suggestedEntities() async throws -> [PinEntity] {
        AppModel.shared.core.home().pins.map { PinEntity(id: $0.id, title: $0.title) }
    }
}

struct PlayPinIntent: AppIntent {
    static let title: LocalizedStringResource = "Play Pinned Item"
    @Parameter(title: "Item") var pin: PinEntity

    @MainActor func perform() async throws -> some IntentResult {
        let model = AppModel.shared
        if let item = model.core.home().pins.first(where: { $0.id == pin.id }) {
            try await model.core.playItem(item: item, shuffle: false)
        }
        return .result()
    }
}

struct StreamShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(intent: PlayPinIntent(), phrases: ["Play \(\.$pin) in \(.applicationName)"], shortTitle: "Play Pinned", systemImageName: "pin.fill")
        AppShortcut(intent: PlayPauseIntent(), phrases: ["Pause \(.applicationName)", "Resume \(.applicationName)"], shortTitle: "Play/Pause", systemImageName: "playpause.fill")
        AppShortcut(intent: NextTrackIntent(), phrases: ["Skip song in \(.applicationName)"], shortTitle: "Next Song", systemImageName: "forward.fill")
    }
}
