import AppKit
import MeridianCore
import SwiftUI

/// Visual tokens for the Instrument design system (`docs/DESIGN.md`):
/// graphite surfaces, bone text, no accent hue; green and red only for up
/// and down. SF Pro for interface text, SF Mono for data.
enum Theme {
    private static func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> NSColor {
        NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: a)
    }

    // Surfaces
    static let bg = rgb(0x121212)
    static let header = rgb(0x181818)
    static let raised = rgb(0x1B1B1B)
    static let selected = rgb(0x1E1E1E)
    static let hover = rgb(0x2A2A2A)
    static let line = rgb(0x2A2A2A)
    static let hairline = rgb(0x1F1F1F)
    static let volume = rgb(0x232323)

    // Text
    static let text = rgb(0xE8E6E1)
    static let text2 = rgb(0xBEBBB3)
    static let muted = rgb(0x8A877F)

    // Signal
    static let up = rgb(0x5BC98A)
    static let down = rgb(0xEF6F66)
    static let warn = rgb(0xE0B25C)
    /// `up`/`down` at ~10% over `bg`, precomputed for the hot grid path.
    static let upTint = rgb(0x1A2A20)
    static let downTint = rgb(0x2B1C1A)
    static let diffAddText = rgb(0x9BE0B6)
    static let diffDelText = rgb(0xF2A29C)

    /// Distinguishable but restrained series colors for multi-line charts.
    static let series: [NSColor] = [text, rgb(0x9CB4CC), rgb(0xC9A26B), rgb(0xA3C9A8), rgb(0xC4A2C9), text2, up, down]

    // Roles used across the app (kept as names so call sites read clearly).
    static let background = bg
    static let white = text
    static let amber = text2
    static let yellow = text
    static let warning = warn
    static let link = text
    static let grid = line
    static let functionBar = header
    static let commandBackground = header
    static let inputText = text
    static let focus = text
    static let mockBadge = warn
    static let flashUp = upTint
    static let flashDown = downTint
    static let selection = hover

    // Typography
    static let baseSize: CGFloat = 12.5

    /// Kept for call sites from the bundled-font era; SF needs no registration.
    static func registerFonts() {}

    /// Data font: SF Mono, whose digits are tabular.
    static func font(_ size: CGFloat = baseSize, weight: NSFont.Weight = .regular) -> NSFont {
        NSFont.monospacedSystemFont(ofSize: size, weight: weight)
    }

    static func swiftFont(_ size: CGFloat = baseSize, weight: NSFont.Weight = .regular) -> Font {
        Font(font(size, weight: weight))
    }

    /// Interface font: SF Pro.
    static func uiFont(_ size: CGFloat = 13, weight: NSFont.Weight = .regular) -> NSFont {
        NSFont.systemFont(ofSize: size, weight: weight)
    }

    static func ui(_ size: CGFloat = 13, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }

    /// Pane label: SF Pro 10.5 bold, uppercase, tracked.
    static func label() -> Font { .system(size: 10.5, weight: .bold) }

    /// Width of one character cell at the base size.
    static var charWidth: CGFloat {
        ("0" as NSString).size(withAttributes: [.font: font()]).width
    }

    static let rowHeight: CGFloat = 22
}

extension NSColor {
    var swiftUI: Color { Color(nsColor: self) }
}

extension StyleFfi {
    var color: NSColor {
        switch self {
        case .normal: Theme.text
        case .emphasis: Theme.text
        case .up: Theme.up
        case .down: Theme.down
        case .muted: Theme.muted
        case .input: Theme.text
        case .warning: Theme.warn
        case .link: Theme.text
        }
    }
}
