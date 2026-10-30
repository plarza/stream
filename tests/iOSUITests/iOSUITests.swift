import XCTest

final class iOSUITests: MusicUITestCase {
    private func openAlbum(_ title: String) {
        app.tabBars.buttons["Library"].tap()
        wait(el("library.Albums")).tap()
        wait(el("row.\(title)")).tap()
        wait(el("collection.play"))
    }

    func testTabsAndLibrarySections() {
        app.tabBars.buttons["Library"].tap()
        for s in ["Playlists", "Artists", "Albums", "Songs", "Liked Songs", "Downloaded"] {
            wait(el("library.\(s)"))
        }
        el("library.Artists").tap()
        wait(el("row.Ada Field"))
        XCTAssertFalse(el("row.Ada Field feat. Juno").exists)
    }

    func testPlayAlbumFromItsPage() {
        openAlbum("Night Drive")
        el("collection.play").tap()
        waitLabel(el("mini.title"), "Neon Lights")
        waitLabel(el("mini.playPause"), "Pause")
        el("mini.next").tap()
        waitLabel(el("mini.title"), "Tunnel")
        el("mini.playPause").tap()
        waitLabel(el("mini.playPause"), "Play")
    }

    func testNowPlayingTransportLyricsAndQueue() {
        openAlbum("Singles")
        wait(el("track.Loose Song")).tap()
        waitLabel(el("mini.title"), "Loose Song")
        el("miniPlayer").tap()
        waitLabel(wait(el("nowPlaying.title")), "Loose Song")
        el("nowPlaying.playPause").tap()
        waitLabel(el("nowPlaying.playPause"), "Play")
        let shuffle = el("nowPlaying.shuffle")
        shuffle.tap()
        waitValue(shuffle, "on")
        shuffle.tap()
        waitValue(shuffle, "off")
        let loop = el("nowPlaying.repeat")
        loop.tap()
        waitValue(loop, "on")
        loop.tap()
        waitValue(loop, "off")
        el("nowPlaying.lyrics").tap()
        wait(text("Second line comes in"))
        el("nowPlaying.queue").tap()
        wait(app.staticTexts["Nothing up next"])
        el("nowPlaying.queue").tap()
        wait(el("nowPlaying.title"))
        // Pull down from the grabber, like a person would.
        let top = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.08))
        top.press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.9)), withVelocity: .fast, thenHoldForDuration: 0)
        waitGone(el("nowPlaying.title"))
    }

    func testPinShowsOnHome() {
        openAlbum("Morning Light")
        el("collection.pin").tap()
        app.tabBars.buttons["Home"].tap()
        wait(el("pin.Morning Light")).tap()
        wait(el("track.Sunrise"))
    }

    func testSearchFindsSongs() {
        app.tabBars.buttons["Search"].tap()
        let field = wait(app.searchFields.firstMatch)
        XCTAssertEqual(field.placeholderValue, "Search your library")
        field.tap()
        field.typeText("tunnel")
        wait(el("result.Tunnel")).tap()
        waitLabel(el("mini.title"), "Tunnel")
    }

    func testSettingsOpen() {
        wait(el("account")).tap()
        wait(text("Normalize loudness"))
    }
}
