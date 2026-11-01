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

    func testSignInShowsTheAccountThenSignOut() {
        wait(app.buttons["signIn"]).tap()
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
