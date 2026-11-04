import UITestSupport
import XCTest

/// Sign in with aza on first launch, and the account in Profile. Its own launch, at the sign-in screen.
final class AccountTests: XCTestCase {
    private let app = XCUIApplication()

    override func setUpWithError() throws {
        continueAfterFailure = false
        app.launchArguments = ["-uiTesting", "YES"]
        app.launchAtSignIn()
    }

    /// Signing in asks the aza app: it opens if it's installed, and otherwise Stream says to get it.
    func testSignInAsksTheAzaApp() {
        wait(app.buttons["signIn"]).tap()
        // The aza app can't be inspected from here, so watch Stream go to the background instead.
        let alert = app.alerts["Get the aza app"]
        XCTAssertTrue(poll(10) { alert.exists || self.app.state != .runningForeground }, "neither the aza app nor the alert")
    }

    func testSignedInAccountSignsOut() {
        app.terminate()
        app.launchArguments = ["-uiTesting", "YES"]
        app.launchPastSignIn(signedIn: true)
        wait(app.buttons["profile"]).tap()
        wait(app.staticTexts["Sample account"])
        app.buttons["account"].tap()
        wait(app.buttons["signOut"]).tap()
        wait(app.buttons["Sign out"]).tap()
        wait(app.staticTexts["Not signed in"])
    }

    func testSkipLeavesYouSignedOut() {
        wait(app.buttons["skip"]).tap()
        wait(app.buttons["profile"]).tap()
        wait(app.staticTexts["Not signed in"])
    }
}
