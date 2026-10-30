import XCTest

/// Taps through the app for the README screenshots (scripts/demo.py --screenshots
/// captures each SHOT_ mark). Not a test.
final class DemoFlow: MusicUITestCase {
    private func shot(_ name: String) {
        XCTContext.runActivity(named: "SHOT_\(name)") { _ in }
        Thread.sleep(forTimeInterval: 2.0)
    }

    private func pin(_ album: String) {
        app.tabBars.buttons["Library"].tap()
        wait(el("library.Albums")).tap()
        wait(el("row.\(album)")).tap()
        wait(el("collection.pin")).tap()
        app.navigationBars.buttons.firstMatch.tap()
        app.navigationBars.buttons.firstMatch.tap()
    }

    func testScreenshots() {
        pin("Night Drive")
        pin("Morning Light")
        pin("Singles")
        app.tabBars.buttons["Library"].tap()
        wait(el("library.Albums")).tap()
        wait(el("row.Morning Light")).tap()
        wait(el("collection.play")).tap()
        Thread.sleep(forTimeInterval: 1)
        shot("album")

        app.navigationBars.buttons.firstMatch.tap()
        wait(el("row.Night Drive")).tap()
        wait(el("collection.play")).tap()
        app.tabBars.buttons["Home"].tap()
        wait(el("pin.Night Drive"))
        shot("home")

        wait(el("mini.title")).tap()
        wait(el("nowPlaying.title"))
        shot("now-playing")
        el("nowPlaying.queue").tap()
        shot("queue")
        el("nowPlaying.queue").tap()
        app.swipeDown(velocity: .fast)

        app.tabBars.buttons["Home"].tap()
        wait(el("pin.Singles")).tap()
        wait(el("track.Loose Song")).tap()
        wait(el("mini.title")).tap()
        el("nowPlaying.lyrics").tap()
        Thread.sleep(forTimeInterval: 1.5)
        shot("lyrics")
        el("nowPlaying.lyrics").tap()
        app.swipeDown(velocity: .fast)

        app.tabBars.buttons["Search"].tap()
        let field = wait(app.searchFields.firstMatch)
        field.tap()
        field.typeText("ne")
        wait(el("result.Night Drive"))
        shot("search")
    }
}
