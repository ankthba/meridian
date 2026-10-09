import AppKit
import MeridianCore

/// Speech forms of what the screen shows. VoiceOver reads the strings the
/// views draw (from `TerminalFormatter`); these helpers only turn symbols
/// into words ("+1.74%" → "up 1.74 percent", "3.39T" → "3.39 trillion") and
/// join a row's cells into one sentence. Pure, so tests cover them.
enum SpokenText {
    /// One cell or field. `text` is exactly what the view draws; `formatted`
    /// is true when that text was formatted from a number in `format`, false
    /// when it came from the data as is (a ticker, a name, a headline).
    struct Cell: Equatable {
        var header: String
        var text: String
        var format: FormatFfi
        var formatted: Bool
    }

    /// A drawn value as words.
    static func value(_ text: String, format: FormatFfi, formatted: Bool = true) -> String {
        let t = text.trimmingCharacters(in: .whitespaces)
        if t == "--" || t == "—" { return "no data" }
        guard formatted, !t.isEmpty else { return t }
        var body = Substring(t)
        var direction = ""
        switch format {
        case .change, .changePercent:
            // The formatter signs changes: "+" up, "-" down, none for zero.
            if let sign = body.first, "+-−".contains(sign) {
                direction = sign == "+" ? "up " : "down "
                body = body.dropFirst()
            }
        default:
            break
        }
        switch format {
        case .percent, .changePercent:
            if body.hasSuffix("%") { return direction + body.dropLast() + " percent" }
        case .large:
            if let suffix = body.last, let word = largeSuffixes[suffix] { return direction + body.dropLast() + " " + word }
        default:
            break
        }
        return direction + body
    }

    private static let largeSuffixes: [Character: String] = ["K": "thousand", "M": "million", "B": "billion", "T": "trillion"]

    /// Column and field headers with their abbreviations spelled out
    /// ("% Chg" → "percent change", "1D %" → "1 day percent").
    static func header(_ title: String) -> String {
        title.split(separator: " ").map { token in
            let s = String(token)
            if let word = abbreviations[s] { return word }
            if let r = rangeWords(s) { return r }
            return s
        }
        .joined(separator: " ")
    }

    private static let abbreviations: [String: String] = [
        "%": "percent", "Chg": "change", "Mkt": "market", "Val": "value", "Cap": "cap", "Yld": "yield",
        "Div": "dividend", "Gr": "growth", "Mgn": "margin", "Ret": "return", "Surp": "surprise",
        "Qty": "quantity", "Ctry": "country", "Ccy": "currency", "Imp": "importance", "Avg": "average",
        "Px": "price", "Exp": "expiry", "Wt": "weight", "Pct": "percent", "–": "to", "—": "to",
    ]

    /// A row as one sentence: "AAPL, Apple Inc., Last 289.44, change up
    /// 1.74 percent". Numbers carry their column name; words and dates read
    /// alone. Empty cells are skipped.
    static func row(_ cells: [Cell]) -> String {
        cells.compactMap { c -> String? in
            let v = value(c.text, format: c.format, formatted: c.formatted)
            guard !v.isEmpty else { return nil }
            let h = header(c.header)
            return isQuantity(c) && !h.isEmpty ? "\(h) \(v)" : v
        }
        .joined(separator: ", ")
    }

    /// Whether a cell is a number that needs its column name to mean anything.
    static func isQuantity(_ c: Cell) -> Bool {
        guard c.formatted else { return false }
        switch c.format {
        case .date, .dateTime, .time, .text: return false
        default: return true
        }
    }

    /// Chart ranges and periods: "1Y" → "1 year", "5D" → "5 days",
    /// "YTD" → "year to date". Anything else reads as is.
    static func range(_ r: String) -> String { rangeWords(r) ?? r }

    private static func rangeWords(_ r: String) -> String? {
        let u = r.uppercased()
        switch u {
        case "YTD": return "year to date"
        case "MAX", "ALL": return "all history"
        default: break
        }
        guard u.count >= 2, let unit = u.last, let word = rangeUnits[unit], let n = Int(u.dropLast()) else { return nil }
        return "\(n) \(word)\(n == 1 ? "" : "s")"
    }

    private static let rangeUnits: [Character: String] = ["D": "day", "W": "week", "M": "month", "Y": "year", "H": "hour"]

    /// "iex · rt · alpaca" → "iex, rt, alpaca".
    static func list(_ s: String) -> String {
        s.replacingOccurrences(of: "   ", with: "; ").replacingOccurrences(of: " · ", with: ", ")
    }
}

/// Reduce Motion, read once and refreshed when the system setting changes,
/// so hot paths can check it for the cost of a stored property.
@MainActor
enum MotionPreference {
    private static var cached: Bool?
    private static var observer: NSObjectProtocol?

    static var reduceMotion: Bool {
        if let cached { return cached }
        let ws = NSWorkspace.shared
        let v = ws.accessibilityDisplayShouldReduceMotion
        cached = v
        observer = ws.notificationCenter.addObserver(forName: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: ws, queue: .main) { _ in
            MainActor.assumeIsolated { cached = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
        }
        return v
    }
}

/// Spoken announcements for changes VoiceOver can't see on its own (the
/// highlighted suggestion while focus stays in the command field). Does
/// nothing unless VoiceOver is running.
@MainActor
enum Announcer {
    /// `make` runs only when VoiceOver is on. High priority interrupts what
    /// VoiceOver is saying (the previous row); medium waits its turn.
    static func announce(_ make: @autoclosure () -> String, priority: NSAccessibilityPriorityLevel = .high) {
        guard NSWorkspace.shared.isVoiceOverEnabled, let app = NSApp else { return }
        let text = make()
        guard !text.isEmpty else { return }
        let element: Any = app.keyWindow ?? app
        NSAccessibility.post(element: element, notification: .announcementRequested, userInfo: [
            .announcement: text,
            .priority: priority.rawValue,
        ])
    }
}
