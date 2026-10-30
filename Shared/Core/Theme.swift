import SwiftUI

/// The accent color choice in Settings.
enum AccentChoice: String, CaseIterable, Identifiable {
    case red, pink, orange, yellow, green, teal, blue, purple, albumArt

    var id: String { rawValue }

    var title: String {
        switch self {
        case .red: "Red"
        case .pink: "Pink"
        case .orange: "Orange"
        case .yellow: "Yellow"
        case .green: "Green"
        case .teal: "Teal"
        case .blue: "Blue"
        case .purple: "Purple"
        case .albumArt: "Album Art"
        }
    }

    var color: Color {
        switch self {
        // Apple Music's red.
        case .red: Color(red: 0.98, green: 0.14, blue: 0.24)
        case .pink: Color(red: 1.0, green: 0.27, blue: 0.55)
        case .orange: Color(red: 1.0, green: 0.55, blue: 0.2)
        case .yellow: Color(red: 1.0, green: 0.8, blue: 0.2)
        case .green: Color(red: 0.2, green: 0.8, blue: 0.4)
        case .teal: Color(red: 0.2, green: 0.75, blue: 0.8)
        case .blue: Color(red: 0.2, green: 0.5, blue: 1.0)
        case .purple: Color(red: 0.6, green: 0.35, blue: 1.0)
        case .albumArt: .white
        }
    }

    /// Black on light accents, white on dark ones (relative luminance).
    var onColor: Color {
        switch self {
        case .yellow, .albumArt: return .black
        default:
            #if os(iOS)
            let c = UIColor(color)
            #else
            let c = NSColor(color).usingColorSpace(.sRGB) ?? .white
            #endif
            var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
            c.getRed(&r, green: &g, blue: &b, alpha: &a)
            return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.55 ? Color.black : Color.white
        }
    }

    static var saved: AccentChoice {
        UserDefaults.standard.string(forKey: "accent").flatMap(AccentChoice.init) ?? .red
    }

    func save() { UserDefaults.standard.set(rawValue, forKey: "accent") }
}

/// Row of color swatches; album art shows as a gradient swatch.
struct AccentPicker: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 8) {
            ForEach(AccentChoice.allCases) { c in
                Button {
                    withAnimation(.snappy) { model.theme = c }
                } label: {
                    Circle()
                        .fill(c == .albumArt ? AnyShapeStyle(AngularGradient(colors: [.red, .orange, .yellow, .green, .blue, .purple, .red], center: .center)) : AnyShapeStyle(c.color))
                        .frame(width: 26, height: 26)
                        .frame(maxWidth: .infinity)
                        .overlay {
                            if model.theme == c {
                                Circle().stroke(.primary, lineWidth: 2).padding(-4)
                            }
                        }
                }
                .buttonStyle(.plain)
                .help(c.title)
                .accessibilityLabel(c.title)
                .accessibilityIdentifier("accent.\(c.rawValue)")
                .accessibilityAddTraits(model.theme == c ? .isSelected : [])
            }
        }
        .padding(.vertical, 4)
    }
}
