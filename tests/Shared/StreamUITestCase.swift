import XCTest

/// Launches Stream against the fixture library in test mode: isolated data,
/// silent output, nothing persisted, no network services.
class StreamUITestCase: XCTestCase {
    var app: XCUIApplication!

    /// The small generated library from tests/make-fixtures.sh, or the one in
    /// STREAM_LIBRARY (scripts/demo.py passes its screenshot library this way).
    static var library: String {
        if let path = ProcessInfo.processInfo.environment["STREAM_LIBRARY"] { return path }
        return URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("library/Music")
            .path
    }

    /// One launch serves every test; between tests the app is reset in place.
    private static var running: XCUIApplication?

    override func setUpWithError() throws {
        continueAfterFailure = false
        if let app = Self.running, app.state == .runningForeground {
            self.app = app
            #if os(iOS)
            XCUIDevice.shared.system.open(URL(string: "network-aza-stream://reset")!)
            let home = app.tabBars.buttons["Home"].firstMatch
            XCTAssertTrue(poll(5) { home.isSelected && home.isHittable }, "reset didn't land on Home")
            #endif
            return
        }
        app = XCUIApplication()
        // Pairs only: AppKit reads `-key value`, and a bare flag before a pair hides the window.
        let spotify = FakeSpotify.shared
        app.launchArguments = ["-musicFolder", Self.library, "-uiTesting", "YES",
                               "-spotifyAccounts", spotify.accountsURL, "-spotifyApi", spotify.apiURL]
        app.launch()
        Self.running = app
    }

    func el(_ id: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: id).firstMatch
    }

    func text(_ label: String) -> XCUIElement {
        app.staticTexts.matching(NSPredicate(format: "label == %@ OR value == %@", label, label)).firstMatch
    }

    /// Checks every 0.1s. XCTest's own waits check about once a second, which made every step cost a second.
    func poll(_ timeout: TimeInterval, _ done: () -> Bool) -> Bool {
        let end = Date().addingTimeInterval(timeout)
        while !done() {
            if Date() > end { return false }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
        return true
    }

    @discardableResult
    func wait(_ e: XCUIElement, _ timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        XCTAssertTrue(poll(timeout) { e.exists }, "\(e) never appeared", file: file, line: line)
        return e
    }

    func waitGone(_ e: XCUIElement, _ timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(poll(timeout) { !e.exists }, "\(e) still visible", file: file, line: line)
    }

    /// Waits until an element's label (or value) equals `expected`.
    func waitLabel(_ e: XCUIElement, _ expected: String, _ timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) {
        let ok = poll(timeout) { e.exists && (e.label == expected || e.value as? String == expected) }
        XCTAssertTrue(ok, "\(e.identifier) is “\(e.label)” / “\(String(describing: e.value))”, wanted “\(expected)”", file: file, line: line)
    }

    func waitValue(_ e: XCUIElement, _ expected: String, _ timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(poll(timeout) { e.exists && e.value as? String == expected }, "\(e.identifier) value is \(String(describing: e.value)), wanted \(expected)", file: file, line: line)
    }
}
