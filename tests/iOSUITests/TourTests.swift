import XCTest

/// Walks every screen and saves a screenshot of each to $TOUR_DIR (or /tmp/music-tour),
/// for reviewing layout. Doubles as a smoke test that every screen opens.
final class TourTests: MusicUITestCase {
    private var dir: URL {
        let d = URL(fileURLWithPath: ProcessInfo.processInfo.environment["TOUR_DIR"] ?? "/tmp/music-tour")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func shot(_ name: String) {
        sleep(1)
        let png = XCUIScreen.main.screenshot().pngRepresentation
        try? png.write(to: dir.appendingPathComponent("\(name).png"))
    }

    func testTour() {
        let library = app.tabBars.buttons["Library"]
        wait(library).tap()
        wait(el("library.Albums"))
        shot("01-library")
        el("library.Albums").tap()
        wait(el("row.Night Drive"))
        shot("02-albums")
        el("row.Night Drive").tap()
        wait(el("collection.play")).tap()
        shot("03-album")
        app.navigationBars.buttons.firstMatch.tap()
        app.navigationBars.buttons.firstMatch.tap()
        wait(el("library.Songs")).tap()
        wait(el("row.Tunnel"))
        shot("04-songs")
        app.navigationBars.buttons.firstMatch.tap()
        wait(el("library.Artists")).tap()
        wait(el("row.Neon Coast"))
        shot("05-artists")

        app.tabBars.buttons["Home"].tap()
        wait(el("home.settings")).tap()
        wait(el("accent.blue"))
        shot("06-settings")
        app.buttons["Done"].tap()

        app.tabBars.buttons["Search"].tap()
        shot("07-search-empty")
        let field = wait(app.searchFields.firstMatch)
        field.tap()
        field.typeText("ne")
        wait(el("result.Night Drive"))
        shot("08-search")
        el("result.Night Drive").tap()
        wait(el("collection.play")).tap()
        shot("08b-search-album")
        wait(el("mini.title")).tap()
        wait(el("nowPlaying.title"))
        shot("09-now-playing")
        el("nowPlaying.lyrics").tap()
        shot("10-lyrics")
        el("nowPlaying.queue").tap()
        shot("11-queue")
    }
}
