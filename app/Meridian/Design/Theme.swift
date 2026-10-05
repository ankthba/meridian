import AppKit
import CoreText
import MeridianCore
import SwiftUI

/// Visual tokens. Values are PROVISIONAL: chosen from public descriptions
/// of the terminal convention (amber on black, white emphasis, amber input
/// cells, red function bar, green/red up/down) until reference screenshots
/// exist; `scripts/palette` will replace them with measured values.
enum Theme {
    // Colors
    static let background = NSColor(srgbRed: 0, green: 0, blue: 0, alpha: 1)
    static let amber = NSColor(srgbRed: 1.0, green: 0.627, blue: 0.157, alpha: 1) // #FFA028
    static let white = NSColor(srgbRed: 0.949, green: 0.949, blue: 0.949, alpha: 1) // #F2F2F2
    static let yellow = NSColor(srgbRed: 1.0, green: 0.878, blue: 0.0, alpha: 1) // #FFE000
    static let muted = NSColor(srgbRed: 0.49, green: 0.49, blue: 0.49, alpha: 1) // #7D7D7D
    static let up = NSColor(srgbRed: 0.184, green: 0.827, blue: 0.420, alpha: 1) // #2FD36B
    static let down = NSColor(srgbRed: 1.0, green: 0.302, blue: 0.302, alpha: 1) // #FF4D4D
    static let warning = NSColor(srgbRed: 1.0, green: 0.78, blue: 0.2, alpha: 1)
    static let link = NSColor(srgbRed: 0.949, green: 0.949, blue: 0.949, alpha: 1)
    static let functionBar = NSColor(srgbRed: 0.478, green: 0.059, blue: 0.059, alpha: 1) // #7A0F0F
    static let grid = NSColor(srgbRed: 0.149, green: 0.149, blue: 0.149, alpha: 1) // #262626
    static let commandBackground = NSColor(srgbRed: 0.043, green: 0.043, blue: 0.043, alpha: 1)
    static let inputFill = amber
    static let inputText = NSColor.black
    static let focus = NSColor(srgbRed: 0.176, green: 0.420, blue: 1.0, alpha: 1) // #2D6BFF
    static let mockBadge = NSColor(srgbRed: 0.753, green: 0.224, blue: 0.169, alpha: 1)
    static let flashUp = NSColor(srgbRed: 0.05, green: 0.24, blue: 0.12, alpha: 1)
    static let flashDown = NSColor(srgbRed: 0.28, green: 0.06, blue: 0.06, alpha: 1)
    static let selection = NSColor(srgbRed: 0.12, green: 0.18, blue: 0.36, alpha: 1)
    /// Distinct series colors for charts (amber first).
    static let series: [NSColor] = [
        amber, yellow, NSColor(srgbRed: 0.22, green: 0.74, blue: 0.97, alpha: 1), white,
        NSColor(srgbRed: 0.75, green: 0.52, blue: 0.99, alpha: 1), up, down,
    ]

    // Typography
    static let fontFamily = "Iosevka Fixed SS08"
    static let baseSize: CGFloat = 13
    nonisolated(unsafe) private static var registered = false

    /// Registers bundled fonts once; falls back to Menlo if missing.
    static func registerFonts() {
        guard !registered else { return }
        registered = true
        let urls = Bundle.main.urls(forResourcesWithExtension: "ttf", subdirectory: nil) ?? []
        for url in urls {
            CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
    }

    static func font(_ size: CGFloat = baseSize, weight: NSFont.Weight = .regular) -> NSFont {
        let name: String
        switch weight {
        case .bold, .heavy, .black, .semibold: name = "IosevkaFixedSS08-Bold"
        case .medium: name = "IosevkaFixedSS08-Medium"
        default: name = "IosevkaFixedSS08-Regular"
        }
        return NSFont(name: name, size: size) ?? NSFont.monospacedSystemFont(ofSize: size, weight: weight)
    }

    static func swiftFont(_ size: CGFloat = baseSize, weight: NSFont.Weight = .regular) -> Font {
        Font(font(size, weight: weight))
    }

    /// Width of one character cell at the base size.
    static var charWidth: CGFloat {
        let f = font()
        return ("0" as NSString).size(withAttributes: [.font: f]).width
    }

    static let rowHeight: CGFloat = 17
}

extension NSColor {
    var swiftUI: Color { Color(nsColor: self) }
}

extension StyleFfi {
    var color: NSColor {
        switch self {
        case .normal: Theme.amber
        case .emphasis: Theme.white
        case .up: Theme.up
        case .down: Theme.down
        case .muted: Theme.muted
        case .input: Theme.inputText
        case .warning: Theme.warning
        case .link: Theme.link
        }
    }
}
