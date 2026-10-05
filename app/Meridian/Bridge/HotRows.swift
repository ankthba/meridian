import Foundation
import MeridianCore
import QuartzCore

/// One decoded streaming quote row. Layout: `core/crates/stream/src/row.rs`.
struct HotRow: Equatable {
    var instrument: UInt32
    var changed: UInt32
    var tsEvent: Int64
    var bid, ask, last, open, high, low, prevClose, volume: Double
    var bidSize, askSize, lastSize, netChange, pctChange: Double
    var flags: UInt32

    static let stale: UInt32 = 1 << 0
    static let synthetic: UInt32 = 1 << 3
    static let tickUp: UInt32 = 1 << 4
    static let tickDown: UInt32 = 1 << 5

    func value(_ f: LiveFieldFfi) -> Double? {
        let v: Double
        switch f {
        case .last: v = last
        case .bid: v = bid
        case .ask: v = ask
        case .netChange: v = netChange
        case .pctChange: v = pctChange
        case .volume: v = volume
        case .open: v = open
        case .high: v = high
        case .low: v = low
        case .prevClose: v = prevClose
        case .time: v = Double(tsEvent)
        case .bidSize: v = bidSize
        case .askSize: v = askSize
        }
        return v.isFinite && (f != .time || tsEvent > 0) ? v : nil
    }

    /// Bit in `changed` for a live field (for flash-on-change).
    static func changedBit(_ f: LiveFieldFfi) -> UInt32 {
        switch f {
        case .bid: 1 << 0
        case .ask: 1 << 1
        case .last, .netChange, .pctChange, .time: 1 << 2
        case .bidSize: 1 << 3
        case .askSize: 1 << 4
        case .volume: 1 << 5
        case .open, .high, .low, .prevClose: 1 << 6
        }
    }
}

/// Decodes packed rows. Offsets are checked against the Rust layout once at
/// startup so the two sides cannot drift silently.
enum HotRowDecoder {
    static let rowSize = 128
    static let expected: [String: Int] = [
        "row_size": 128, "instrument": 0, "changed": 4, "ts_event": 8, "bid": 16, "ask": 24, "last": 32,
        "open": 40, "high": 48, "low": 56, "prev_close": 64, "volume": 72, "bid_size": 80, "ask_size": 88,
        "last_size": 96, "net_change": 104, "pct_change": 112, "flags": 120,
    ]

    /// Returns a description of any mismatch, or nil when layouts agree.
    static func verify(_ layout: [LayoutFieldFfi]) -> String? {
        for f in layout where expected[f.name] != Int(f.offset) {
            return "hot row layout mismatch for \(f.name): Rust \(f.offset), Swift \(expected[f.name].map(String.init) ?? "missing")"
        }
        return layout.count == expected.count ? nil : "hot row layout field count differs"
    }

    static func decode(_ data: Data, into out: inout [UInt32: HotRow]) -> [UInt32] {
        var ids: [UInt32] = []
        data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            let n = raw.count / rowSize
            ids.reserveCapacity(n)
            for i in 0..<n {
                let b = i * rowSize
                func u32(_ o: Int) -> UInt32 { raw.loadUnaligned(fromByteOffset: b + o, as: UInt32.self).littleEndian }
                func i64(_ o: Int) -> Int64 { raw.loadUnaligned(fromByteOffset: b + o, as: Int64.self).littleEndian }
                func f64(_ o: Int) -> Double { Double(bitPattern: raw.loadUnaligned(fromByteOffset: b + o, as: UInt64.self).littleEndian) }
                let row = HotRow(
                    instrument: u32(0), changed: u32(4), tsEvent: i64(8),
                    bid: f64(16), ask: f64(24), last: f64(32), open: f64(40), high: f64(48), low: f64(56),
                    prevClose: f64(64), volume: f64(72), bidSize: f64(80), askSize: f64(88), lastSize: f64(96),
                    netChange: f64(104), pctChange: f64(112), flags: u32(120)
                )
                out[row.instrument] = row
                ids.append(row.instrument)
            }
        }
        return ids
    }
}

/// A view's live quotes: owns a Rust subscription and polls it each frame.
@MainActor
final class QuoteFeed {
    private let subscription: QuoteSubscriptionFfi
    private var since: UInt64 = 0
    private(set) var rows: [UInt32: HotRow] = [:]
    private(set) var idBySecurity: [String: UInt32] = [:]
    /// Instruments updated in the last poll, with the frame time.
    private(set) var lastChanged: [UInt32: CFTimeInterval] = [:]
    private var listeners: [ObjectIdentifier: ([UInt32]) -> Void] = [:]

    func addListener(_ owner: AnyObject, _ f: @escaping ([UInt32]) -> Void) {
        listeners[ObjectIdentifier(owner)] = f
    }

    func removeListener(_ owner: AnyObject) {
        listeners[ObjectIdentifier(owner)] = nil
    }

    init?(core: Core, securities: [String]) {
        guard !securities.isEmpty, let sub = try? core.subscribe(securities: securities) else { return nil }
        subscription = sub
        if let ids = try? sub.instrumentIds() {
            for (s, id) in zip(securities, ids) { idBySecurity[s] = id }
        }
        DisplayClock.shared.add(self)
    }

    func row(for security: String) -> HotRow? {
        idBySecurity[security].flatMap { rows[$0] }
    }

    /// Called by the display clock. Cheap when nothing changed.
    func tick(_ now: CFTimeInterval) {
        guard let r = try? subscription.poll(since: since) else { return }
        since = r.seq
        guard !r.rows.isEmpty else { return }
        let ids = HotRowDecoder.decode(r.rows, into: &rows)
        for id in ids { lastChanged[id] = now }
        for l in listeners.values { l(ids) }
    }

    func stop() {
        DisplayClock.shared.remove(self)
    }
}

/// Drives `QuoteFeed` polls once per display frame.
@MainActor
final class DisplayClock {
    static let shared = DisplayClock()
    private var feeds: [ObjectIdentifier: QuoteFeedBox] = [:]
    private var timer: Timer?
    private(set) var frameCount: UInt64 = 0
    private(set) var lastFrameTimes: [CFTimeInterval] = []

    private struct QuoteFeedBox { weak var feed: QuoteFeed? }

    func add(_ f: QuoteFeed) {
        feeds[ObjectIdentifier(f)] = QuoteFeedBox(feed: f)
        startIfNeeded()
    }

    func remove(_ f: QuoteFeed) {
        feeds[ObjectIdentifier(f)] = nil
    }

    private func startIfNeeded() {
        guard timer == nil else { return }
        // 60 Hz on the main run loop in common modes (keeps ticking during
        // scrolling/tracking). Polls are microseconds when idle.
        let t = Timer(timeInterval: 1.0 / 60.0, repeats: true) { _ in
            MainActor.assumeIsolated { DisplayClock.shared.fire() }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    private func fire() {
        let now = CACurrentMediaTime()
        frameCount += 1
        lastFrameTimes.append(now)
        if lastFrameTimes.count > 240 { lastFrameTimes.removeFirst(lastFrameTimes.count - 240) }
        for (k, box) in feeds {
            if let f = box.feed { f.tick(now) } else { feeds[k] = nil }
        }
    }
}
