import XCTest

/// Spotify sign-in from Profile, end to end against FakeSpotify: the browser sheet,
/// the app's loopback listener, the token exchange and both approvals; then a real
/// login to Spotify's streaming servers, which turn the stand-in's token away. Then the
/// synced library: a playlist's sort menu, a song by two artists opening either one's
/// page, and an artist's releases filtered by kind.
final class SignInTests: StreamUITestCase {
    /// iOS asks before every sign-in sheet ("“Stream” Wants to Use … to Sign In").
    private func allowSignInSheet(file: StaticString = #filePath, line: UInt = #line) {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let allow = springboard.buttons["Continue"]
        XCTAssertTrue(poll(10) { allow.exists }, "the sign-in sheet never asked", file: file, line: line)
        allow.tap()
    }

    func testSpotifySignIn() {
        let approvals = FakeSpotify.shared.approvals
        wait(el("profile")).tap()
        let clientId = wait(el("settings.spotify.clientId"))
        clientId.tap()
        clientId.typeText("test-client-id")
        el("settings.spotify.signIn").tap()

        // Approval 1 of 2: the account.
        allowSignInSheet()
        waitLabel(wait(el("settings.spotify.account"), 15), "Account, \(FakeSpotify.listener)")
        // Approval 2 of 2: playback, straight after.
        allowSignInSheet()
        XCTAssertTrue(poll(10) { FakeSpotify.shared.approvals == approvals + 2 }, "both approvals should reach Spotify")

        // The streaming servers are Spotify's own and refuse the stand-in's token; the
        // app says so in words and offers to try again.
        let error = wait(el("settings.spotify.error"), 45)
        XCTAssertTrue(error.label.contains("didn't accept the playback sign-in"), "unexpected error: \(error.label)")
        waitLabel(el("settings.spotify.signIn"), "Try again")
        XCTAssertEqual(el("settings.spotify.step.Spotify account").value as? String, "done")

        // The Web API side still works: the library from FakeSpotify.
        checkPlaylistSortArtistLinksAndReleases()
    }

    private func order(_ titles: [String], file: StaticString = #filePath, line: UInt = #line) {
        let ok = poll(5) {
            let ys = titles.map { el("track.\($0)").frame.minY }
            return titles.allSatisfy { el("track.\($0)").exists } && ys == ys.sorted()
        }
        XCTAssertTrue(ok, "songs aren't in the order \(titles)", file: file, line: line)
    }

    /// Runs signed in from testSpotifySignIn: one sign-in per launch, as a second
    /// one in the same run doesn't always get iOS to open its playback sheet.
    func checkPlaylistSortArtistLinksAndReleases() {
        app.buttons["Done"].firstMatch.tap()
        app.tabBars.buttons["Library"].tap()

        // A playlist sorted by title, then back in playlist order.
        wait(el("library.Playlists")).tap()
        // Pulled down to sync now, rather than waiting on the sync after sign-in.
        let mix = el("row.Test Mix")
        XCTAssertTrue(poll(45) {
            if !mix.exists { app.swipeDown() }
            return mix.exists
        }, "the playlist never synced")
        mix.tap()
        order(["Zebra", "Apple", "Mango"])
        wait(el("playlist.sort")).tap()
        wait(app.buttons["Title"]).tap()
        order(["Apple", "Mango", "Zebra"])
        el("playlist.sort").tap()
        wait(app.buttons["Playlist Order"]).tap()
        order(["Zebra", "Apple", "Mango"])
        app.navigationBars.buttons.firstMatch.tap()
        app.navigationBars.buttons.firstMatch.tap()

        // A song by two artists offers both; the second opens their page.
        wait(el("library.Liked Songs")).tap()
        wait(el("row.Duet"), 15).press(forDuration: 1)
        wait(app.buttons["Go to Artist"]).tap()
        wait(app.buttons["Mira Vale"])
        app.buttons["Otto Rhee"].tap()

        // Every release, newest first, filtered by the pills.
        wait(el("release.One Song"), 15)
        for r in ["Big Album", "Short Set"] { wait(el("release.\(r)")) }
        XCTAssertLessThan(el("release.One Song").frame.minX + el("release.One Song").frame.minY * 10,
                          el("release.Big Album").frame.minX + el("release.Big Album").frame.minY * 10, "newest first")
        el("pill.EPs").tap()
        wait(el("release.Short Set"))
        waitGone(el("release.Big Album"))
        XCTAssertFalse(el("release.One Song").exists)
        el("pill.EPs").tap()
        wait(el("release.Big Album"))
    }
}

