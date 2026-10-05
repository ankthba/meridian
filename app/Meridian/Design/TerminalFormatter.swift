import Foundation
import MeridianCore

/// Formats numbers per the display hints Rust sends. Pure; no computation
/// beyond presentation (ARCHITECTURE §1, rule 1).
enum TerminalFormatter {
    nonisolated(unsafe) private static var cache: [String: NumberFormatter] = [:]

    private static func grouped(_ decimals: Int) -> NumberFormatter {
        let key = "g\(decimals)"
        if let f = cache[key] { return f }
        let f = NumberFormatter()
        f.numberStyle = .decimal
        f.usesGroupingSeparator = true
        f.minimumFractionDigits = decimals
        f.maximumFractionDigits = decimals
        f.locale = Locale(identifier: "en_US_POSIX")
        cache[key] = f
        return f
    }

    static func fixed(_ v: Double, _ decimals: Int, grouping: Bool = true) -> String {
        if grouping { return grouped(decimals).string(from: NSNumber(value: v)) ?? "" }
        return String(format: "%.\(decimals)f", v)
    }

    static func large(_ v: Double, _ decimals: Int) -> String {
        let a = abs(v)
        let (div, suffix): (Double, String) = a >= 1e12 ? (1e12, "T") : a >= 1e9 ? (1e9, "B") : a >= 1e6 ? (1e6, "M") : a >= 1e3 ? (1e3, "K") : (1, "")
        if suffix.isEmpty { return fixed(v, 0) }
        return String(format: "%.\(decimals)f%@", v / div, suffix)
    }

    private static let dateFmt: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "MM/dd/yy"
        f.locale = Locale(identifier: "en_US_POSIX")
        return f
    }()

    private static let dateTimeFmt: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "MM/dd HH:mm"
        f.locale = Locale(identifier: "en_US_POSIX")
        return f
    }()

    private static let timeFmt: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        f.locale = Locale(identifier: "en_US_POSIX")
        return f
    }()

    static func date(fromNanos ns: Double) -> Date { Date(timeIntervalSince1970: ns / 1e9) }

    /// Formats `value` for `format`. Signed formats get an explicit `+`.
    static func string(_ value: Double?, _ format: FormatFfi) -> String {
        guard let v = value, v.isFinite else { return "" }
        switch format {
        case .number(let d), .price(let d): return fixed(v, Int(d))
        case .percent(let d): return fixed(v, Int(d)) + "%"
        case .change(let d): return (v > 0 ? "+" : "") + fixed(v, Int(d))
        case .changePercent(let d): return (v > 0 ? "+" : "") + fixed(v, Int(d)) + "%"
        case .large(let d): return large(v, Int(d))
        case .integer: return fixed(v, 0)
        case .date: return dateFmt.string(from: date(fromNanos: v))
        case .dateTime: return dateTimeFmt.string(from: date(fromNanos: v))
        case .time: return v > 0 ? timeFmt.string(from: date(fromNanos: v)) : ""
        case .text: return fixed(v, 2)
        }
    }

    /// Up/down styling for signed formats.
    static func signedStyle(_ value: Double?, _ format: FormatFfi) -> StyleFfi? {
        guard let v = value, v.isFinite else { return nil }
        switch format {
        case .change, .changePercent: return v > 0 ? .up : v < 0 ? .down : nil
        default: return nil
        }
    }
}
