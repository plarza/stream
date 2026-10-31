import XCTest

/// Spotify sign-in from Settings, end to end against FakeSpotify: the browser sheet,
/// the app's loopback listener, the token exchange and both approvals; then a real
/// login to Spotify's streaming servers, which turn the stand-in's token away.
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
        wait(el("account")).tap()
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
    }
}
