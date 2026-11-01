import XCTest

/// Taps through the app for the README screenshots (scripts/demo.py --screenshots
/// builds the library, then captures each SHOT_ mark). Not a test.
final class DemoFlow: StreamUITestCase {
    /// demo.json next to the library: which albums and artists go where, written by scripts/demo.py.
    struct Demo: Decodable {
        struct Item: Decodable { let id: String; let title: String }
        let pins: [Item]
        let recents: [Item]
        let album: Item
        let playing: Item
        let lyrics: Item
        let lyricsTrack: String
        let search: String
        let result: String
    }

    private var demo: Demo {
        let url = URL(fileURLWithPath: Self.library).deletingLastPathComponent().appendingPathComponent("demo.json")
        return try! JSONDecoder().decode(Demo.self, from: Data(contentsOf: url))
    }

    private func shot(_ name: String) {
        XCTContext.runActivity(named: "SHOT_\(name)") { _ in }
        Thread.sleep(forTimeInterval: 2.0)
    }

    private func link(_ action: String, _ item: Demo.Item) {
        XCUIDevice.shared.system.open(URL(string: "network-aza-stream://\(action)?id=\(item.id)")!)
    }

    func testScreenshots() {
        let demo = demo
        for item in demo.pins {
            link("pin", item)
            Thread.sleep(forTimeInterval: 0.5)
        }
        // Oldest first: each play goes to the top of Recents.
        for album in demo.recents {
            link("play", album)
            Thread.sleep(forTimeInterval: 1)
        }
        app.tabBars.buttons["Library"].tap()
        wait(el("library.Albums"))
        shot("library")

        link("open", demo.album)
        wait(el("collection.play")).tap()
        Thread.sleep(forTimeInterval: 1)
        shot("album")
        el("collection.cover").tap()
        wait(el("cover.close"))
        shot("cover")
        el("cover.close").tap()
        waitGone(el("cover.close"))

        link("play", demo.playing)
        app.tabBars.buttons["Home"].tap()
        wait(el("pin.\(demo.pins[0].title)"))
        Thread.sleep(forTimeInterval: 1)
        shot("home")

        wait(el("mini.title")).tap()
        wait(el("nowPlaying.title"))
        shot("now-playing")
        el("nowPlaying.queue").tap()
        shot("queue")
        el("nowPlaying.queue").tap()
        app.swipeDown(velocity: .fast)

        link("open", demo.lyrics)
        wait(el("track.\(demo.lyricsTrack)")).tap()
        wait(el("mini.title")).tap()
        el("nowPlaying.lyrics").tap()
        Thread.sleep(forTimeInterval: 1.5)
        shot("lyrics")
        el("nowPlaying.lyrics").tap()
        app.swipeDown(velocity: .fast)

        app.tabBars.buttons["Search"].tap()
        shot("search-open")
        let field = wait(app.searchFields.firstMatch)
        field.tap()
        field.typeText(demo.search)
        wait(el("result.\(demo.result)"))
        shot("search")
    }
}
