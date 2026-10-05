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

    private static let pow10: [Double] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000]

    /// Fast fixed-decimal formatting with thousands grouping (hot path: no
    /// NumberFormatter, no varargs). Falls back for huge magnitudes.
    static func fixed(_ v: Double, _ decimals: Int, grouping: Bool = true) -> String {
        guard v.isFinite else { return "" }
        let d = max(0, min(decimals, 8))
        let scaled = (abs(v) * pow10[d]).rounded()
        guard scaled < 9.0e15 else { return grouped(d).string(from: NSNumber(value: v)) ?? "" }
        let n = UInt64(scaled)
        let unit = UInt64(pow10[d])
        let intPart = n / unit
        let frac = n % unit
        var digits = Array(String(intPart).utf8)
        if grouping && digits.count > 3 {
            var out: [UInt8] = []
            out.reserveCapacity(digits.count + digits.count / 3)
            for (i, c) in digits.enumerated() {
                if i > 0 && (digits.count - i) % 3 == 0 { out.append(UInt8(ascii: ",")) }
                out.append(c)
            }
            digits = out
        }
        if d > 0 {
            digits.append(UInt8(ascii: "."))
            let f = Array(String(frac).utf8)
            for _ in 0..<(d - f.count) { digits.append(UInt8(ascii: "0")) }
            digits.append(contentsOf: f)
        }
        let negative = v < 0 && n != 0
        return (negative ? "-" : "") + String(decoding: digits, as: UTF8.self)
    }

    static func large(_ v: Double, _ decimals: Int) -> String {
        let a = abs(v)
        let (div, suffix): (Double, String) = a >= 1e12 ? (1e12, "T") : a >= 1e9 ? (1e9, "B") : a >= 1e6 ? (1e6, "M") : a >= 1e3 ? (1e3, "K") : (1, "")
        if suffix.isEmpty { return fixed(v, 0) }
        return fixed(v / div, decimals, grouping: false) + suffix
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

    nonisolated(unsafe) private static var tzOffset: Int = TimeZone.current.secondsFromGMT()

    /// HH:mm:ss in local time without DateFormatter (hot path for live grids).
    static func fastTime(_ ns: Double) -> String {
        var secs = (Int(ns / 1e9) + tzOffset) % 86_400
        if secs < 0 { secs += 86_400 }
        let h = secs / 3600, m = (secs % 3600) / 60, s = secs % 60
        func two(_ x: Int) -> String { x < 10 ? "0\(x)" : "\(x)" }
        return "\(two(h)):\(two(m)):\(two(s))"
    }

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
        case .time: return v > 0 ? fastTime(v) : ""
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
