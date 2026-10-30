import SwiftUI

extension Color {
    /// Apple Music's key color: #FA2D48 in light mode, #FA586A in dark.
    static let appleMusicRed: Color = {
        #if os(iOS)
        Color(UIColor { $0.userInterfaceStyle == .dark
            ? UIColor(red: 0.980, green: 0.345, blue: 0.416, alpha: 1)
            : UIColor(red: 0.980, green: 0.176, blue: 0.282, alpha: 1) })
        #else
        Color(NSColor(name: nil) { $0.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(srgbRed: 0.980, green: 0.345, blue: 0.416, alpha: 1)
            : NSColor(srgbRed: 0.980, green: 0.176, blue: 0.282, alpha: 1) })
        #endif
    }()
}
