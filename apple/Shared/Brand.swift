import SwiftUI

/// Quick Desktop's colours, from its icon, teal and cream, which the
/// iPhone and iPad app accents the system's own surfaces with. The accent
/// colour, teal in light mode and cream in dark, as the icon is on black, is
/// the iPhone and iPad app's `AccentColor`.
enum Brand {
    /// The icon's teal.
    static let teal = Color(hex: 0x025559)
    /// The icon's cream.
    static let cream = Color(hex: 0xFCF5E3)

    /// Text on the accent colour: cream on teal, teal on cream.
    static func onAccent(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? teal : cream
    }

    /// A switch that is on: teal, lighter in dark mode, where the accent's
    /// cream would hide the switch's white knob.
    static func switchOn(_ scheme: ColorScheme) -> Color {
        scheme == .dark ? Color(hex: 0x2A9D98) : teal
    }
}

private extension Color {
    /// The sRGB colour `0xRRGGBB`.
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255
        )
    }
}

/// A computer's icon in the lists: a tile of glass, as the system's own
/// icons are. Teal with a cream glyph in light mode, and cream with a teal
/// one in dark mode, for a computer online; grey glass for one offline;
/// frosted for one to pair with; amber for one to beware of.
struct HostIcon: View {
    /// How the tile looks.
    enum Look {
        case online, offline, pairable, warning
    }

    let look: Look
    /// The SF Symbol on the tile.
    let symbol: String

    @Environment(\.colorScheme) private var scheme

    /// The tile's side, in points.
    private static let side: CGFloat = 36
    /// Its corners' radius, in points.
    private static let cornerRadius: CGFloat = 10
    /// The glyph's size, in points.
    private static let glyphSize: CGFloat = 17
    /// The width of the rim the glass catches light on, in points.
    private static let rimWidth: CGFloat = 0.75

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Self.cornerRadius, style: .continuous)
        shape
            .fill(LinearGradient(colors: fill, startPoint: .top, endPoint: .bottom))
            .overlay {
                // The light the glass catches at its top.
                shape.fill(
                    RadialGradient(
                        colors: [.white.opacity(0.5), .white.opacity(0.1), .clear],
                        center: UnitPoint(x: 0.28, y: 0),
                        startRadius: 0,
                        endRadius: Self.side * 0.85
                    )
                )
            }
            .overlay {
                shape.strokeBorder(
                    LinearGradient(colors: rim, startPoint: .top, endPoint: .bottom),
                    lineWidth: Self.rimWidth
                )
            }
            .overlay {
                Image(systemName: symbol)
                    .font(.system(size: Self.glyphSize, weight: .semibold))
                    .foregroundStyle(glyph)
            }
            .frame(width: Self.side, height: Self.side)
            .shadow(color: .black.opacity(scheme == .dark ? 0.3 : 0.15), radius: 4, y: 2)
            .accessibilityHidden(true)
    }

    /// The glass's colours, top to bottom.
    private var fill: [Color] {
        switch (look, scheme == .dark) {
        case (.online, false): [Color(hex: 0x13878C), Color(hex: 0x036267), Color(hex: 0x014246)]
        case (.online, true): [Color(hex: 0xFFFAF0), Color(hex: 0xF3E9D2), Color(hex: 0xE2D4B4)]
        case (.offline, false): [Color(hex: 0xC9C6BF), Color(hex: 0xA9A59C), Color(hex: 0x8F8B82)]
        case (.offline, true): [Brand.cream.opacity(0.3), Brand.cream.opacity(0.14)]
        case (.pairable, false): [Color.white.opacity(0.95), Color(hex: 0xECE7DB)]
        case (.pairable, true): [Brand.cream.opacity(0.16), Brand.cream.opacity(0.05)]
        case (.warning, _): [Color(hex: 0xF0A35B), Color(hex: 0xD07A2A)]
        }
    }

    /// The rim's colours, top to bottom.
    private var rim: [Color] {
        switch (look, scheme == .dark) {
        case (.pairable, false): [Brand.teal.opacity(0.2), Brand.teal.opacity(0.12)]
        case (.pairable, true): [Brand.cream.opacity(0.3), Brand.cream.opacity(0.15)]
        default: [Color.white.opacity(0.6), Color.white.opacity(0.08)]
        }
    }

    /// The glyph's colour.
    private var glyph: Color {
        switch (look, scheme == .dark) {
        case (.online, false): Brand.cream
        case (.online, true): Brand.teal
        case (.offline, false), (.warning, _): .white
        case (.offline, true): Brand.cream.opacity(0.7)
        case (.pairable, false): Brand.teal
        case (.pairable, true): Brand.cream
        }
    }
}

extension View {
    /// Colours a form's switches, on iPhone and iPad, so that they show in
    /// dark mode too.
    func brandSwitches() -> some View {
        modifier(BrandSwitches())
    }
}

/// [`View.brandSwitches`].
private struct BrandSwitches: ViewModifier {
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        #if os(iOS)
        content.tint(Brand.switchOn(scheme))
        #else
        content
        #endif
    }
}
