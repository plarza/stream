import XCTest

/// Launches Music against the fixture library in test mode: isolated data,
/// silent output, nothing persisted, no network services.
class MusicUITestCase: XCTestCase {
    var app: XCUIApplication!

    static var fixtures: String {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("fixtures/Music")
            .path
    }

    override func setUpWithError() throws {
        continueAfterFailure = false
        app = XCUIApplication()
        // Pairs only: AppKit reads `-key value`, and a bare flag before a pair hides the window.
        app.launchArguments = ["-musicFolder", Self.fixtures, "-uiTesting", "YES"]
        app.launch()
    }

    override func tearDownWithError() throws {
        app.terminate()
    }

    func el(_ id: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: id).firstMatch
    }

    func text(_ label: String) -> XCUIElement {
        app.staticTexts.matching(NSPredicate(format: "label == %@ OR value == %@", label, label)).firstMatch
    }

    @discardableResult
    func wait(_ e: XCUIElement, _ timeout: TimeInterval = 10, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        XCTAssertTrue(e.waitForExistence(timeout: timeout), "\(e) never appeared", file: file, line: line)
        return e
    }

    func waitGone(_ e: XCUIElement, _ timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) {
        let p = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: e)
        XCTAssertEqual(XCTWaiter.wait(for: [p], timeout: timeout), .completed, "\(e) still visible", file: file, line: line)
    }

    /// Waits until an element's label (or value) equals `expected`.
    func waitLabel(_ e: XCUIElement, _ expected: String, _ timeout: TimeInterval = 8, file: StaticString = #filePath, line: UInt = #line) {
        let p = expectation(for: NSPredicate(format: "label == %@ OR value == %@", expected, expected), evaluatedWith: e)
        let r = XCTWaiter.wait(for: [p], timeout: timeout)
        XCTAssertEqual(r, .completed, "\(e.identifier) is “\(e.label)” / “\(String(describing: e.value))”, wanted “\(expected)”", file: file, line: line)
    }

    func waitValue(_ e: XCUIElement, _ expected: String, _ timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) {
        let p = expectation(for: NSPredicate(format: "value == %@", expected), evaluatedWith: e)
        XCTAssertEqual(XCTWaiter.wait(for: [p], timeout: timeout), .completed, "\(e.identifier) value is \(String(describing: e.value)), wanted \(expected)", file: file, line: line)
    }
}
