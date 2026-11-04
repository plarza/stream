import SwiftUI

/// The Mac window's zoom, as in a browser: ⌘+ / ⌘- / ⌘0. Everything is laid out
/// at the window's size divided by the zoom, then scaled up to fill it.
enum Zoom {
    static let key = "zoom"
    static let standard = 1.1
    static let steps = [0.8, 0.9, 1.0, 1.1, 1.2, 1.35, 1.5]

    static func next(_ z: Double, _ direction: Int) -> Double {
        let i = steps.firstIndex { $0 >= z - 0.001 } ?? steps.count - 1
        return steps[min(max(i + direction, 0), steps.count - 1)]
    }
}

struct Zoomed<Content: View>: View {
    var zoom: Double
    @ViewBuilder var content: Content

    var body: some View {
        GeometryReader { geo in
            content
                .frame(width: geo.size.width / zoom, height: geo.size.height / zoom)
                .scaleEffect(zoom, anchor: .topLeading)
        }
        .ignoresSafeArea()
        .animation(.smooth(duration: 0.25), value: zoom)
    }
}
