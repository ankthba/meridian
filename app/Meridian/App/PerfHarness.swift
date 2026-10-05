import AppKit
import Darwin
import MeridianCore
import SwiftUI

/// Measures the ARCHITECTURE §12 budgets inside the real app process.
/// Enabled with MERIDIAN_PERF=1 (use MERIDIAN_MODE=mock,
/// MERIDIAN_MOCK_EXTRA=2000, MERIDIAN_MOCK_RATE=3). Writes JSON to
/// MERIDIAN_PERF_OUT (default stdout) and exits.
@MainActor
enum PerfHarness {
    static var enabled: Bool { ProcessInfo.processInfo.environment["MERIDIAN_PERF"] == "1" }

    /// Seconds since this process started (kernel start time).
    static func secondsSinceProcessStart() -> Double {
        var kinfo = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
        guard sysctl(&mib, 4, &kinfo, &size, nil, 0) == 0 else { return .nan }
        let start = kinfo.kp_proc.p_un.__p_starttime
        let startSec = Double(start.tv_sec) + Double(start.tv_usec) / 1e6
        return Date().timeIntervalSince1970 - startSec
    }

    static func cpuSeconds() -> Double {
        var u = rusage()
        getrusage(RUSAGE_SELF, &u)
        return Double(u.ru_utime.tv_sec) + Double(u.ru_utime.tv_usec) / 1e6 + Double(u.ru_stime.tv_sec) + Double(u.ru_stime.tv_usec) / 1e6
    }

    static func percentile(_ xs: [Double], _ p: Double) -> Double {
        guard !xs.isEmpty else { return .nan }
        let s = xs.sorted()
        return s[min(s.count - 1, Int(Double(s.count - 1) * p))]
    }

    static func run() {
        var results: [String: Any] = [:]
        Task { @MainActor in
            let ws = AppModel.shared.workspace
            // 1. Cold launch: process start → first panel screen loaded.
            var waited = 0
            while ws.panels[0].screen == nil && waited < 400 {
                try? await Task.sleep(nanoseconds: 5_000_000)
                waited += 1
            }
            results["cold_launch_s"] = secondsSinceProcessStart()

            // 2. Command line: suggestions latency and GO → screen loaded.
            let p = ws.panels[0]
            var suggestMs: [Double] = []
            for text in ["A", "AA", "AAP", "AAPL", "APPLE", "MICRO", "DE", "GP"] {
                p.commandText = text
                let t = CACurrentMediaTime()
                p.updateSuggestions()
                suggestMs.append((CACurrentMediaTime() - t) * 1000)
            }
            results["suggest_ms_p50"] = percentile(suggestMs, 0.5)
            results["suggest_ms_max"] = suggestMs.max() ?? .nan
            var goMs: [Double] = []
            for cmd in ["MSFT US <EQUITY> DES", "AAPL US <EQUITY> HP", "NVDA US <EQUITY> FA", "IBM US <EQUITY> DES"] {
                p.commandText = cmd
                let t = CACurrentMediaTime()
                p.go()
                while p.loading { try? await Task.sleep(nanoseconds: 500_000) }
                goMs.append((CACurrentMediaTime() - t) * 1000)
            }
            results["go_to_screen_ms"] = goMs

            // 3. Streaming: 2,000 symbols in a live grid for 20 s.
            if let core = AppModel.shared.core {
                let n = Int(ProcessInfo.processInfo.environment["MERIDIAN_MOCK_EXTRA"] ?? "2000") ?? 2000
                let secs = (1...n).map { String(format: "ZQ%04d US Equity", $0) }
                let table = TableFfi(
                    title: nil,
                    columns: [
                        ColumnFfi(title: "Security", format: .text, align: .left, width: 18, live: nil),
                        ColumnFfi(title: "Last", format: .price(decimals: 2), align: .right, width: 10, live: .last),
                        ColumnFfi(title: "Chg", format: .change(decimals: 2), align: .right, width: 9, live: .netChange),
                        ColumnFfi(title: "%Chg", format: .changePercent(decimals: 2), align: .right, width: 8, live: .pctChange),
                        ColumnFfi(title: "Bid", format: .price(decimals: 2), align: .right, width: 10, live: .bid),
                        ColumnFfi(title: "Ask", format: .price(decimals: 2), align: .right, width: 10, live: .ask),
                        ColumnFfi(title: "Volume", format: .large(decimals: 2), align: .right, width: 9, live: .volume),
                    ],
                    rows: secs.map { RowFfi(cells: [CellFfi(value: nil, text: $0, style: .normal)] + Array(repeating: CellFfi(value: nil, text: nil, style: .normal), count: 6), depth: 0, security: $0, action: nil, emphasis: false) },
                    pageSize: nil,
                    numbered: false
                )
                // Baseline: subscription polling + mock tick generation only.
                let base = QuoteFeed(core: core, securities: secs)
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                let bc0 = cpuSeconds(), bt0 = CACurrentMediaTime()
                try? await Task.sleep(nanoseconds: 10_000_000_000)
                results["stream_cpu_percent_feed_only"] = (cpuSeconds() - bc0) / (CACurrentMediaTime() - bt0) * 100
                base?.stop()
                let grid = TerminalGridView()
                grid.table = table
                let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1600, height: 1000), styleMask: [.titled], backing: .buffered, defer: false)
                window.title = "Perf"
                let scroll = NSScrollView(frame: window.contentView!.bounds)
                grid.frame = NSRect(x: 0, y: 0, width: 1600, height: grid.contentHeight)
                scroll.documentView = grid
                window.contentView = scroll
                window.orderFrontRegardless()
                let feed = QuoteFeed(core: core, securities: secs)
                grid.feed = feed
                let self_counter = NSObject()
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                let clock = DisplayClock.shared
                let f0 = clock.frameCount
                let rows0 = TerminalGridView.rowsDrawn, calls0 = TerminalGridView.drawCalls
                let cpu0 = cpuSeconds()
                let t0 = CACurrentMediaTime()
                var intervals: [Double] = []
                var last = CACurrentMediaTime()
                var updates = 0
                feed?.addListener(self_counter) { ids in updates += ids.count }
                // Sample frame-to-frame time from the display clock's timestamps.
                while CACurrentMediaTime() - t0 < 20 {
                    try? await Task.sleep(nanoseconds: 16_000_000)
                    let now = CACurrentMediaTime()
                    intervals.append((now - last) * 1000)
                    last = now
                }
                let wall = CACurrentMediaTime() - t0
                results["grid_rows_drawn_per_s"] = Double(TerminalGridView.rowsDrawn - rows0) / wall
                results["grid_draw_calls_per_s"] = Double(TerminalGridView.drawCalls - calls0) / wall
                let cpu = cpuSeconds() - cpu0
                let frames = clock.frameCount - f0
                results["stream_symbols"] = n
                results["stream_row_updates_per_s"] = Double(updates) / wall
                results["stream_fps"] = Double(frames) / wall
                results["stream_main_loop_interval_ms_p50"] = percentile(intervals, 0.5)
                results["stream_main_loop_interval_ms_p99"] = percentile(intervals, 0.99)
                results["stream_cpu_percent_one_core"] = cpu / wall * 100
                feed?.stop()
                window.orderOut(nil)
            }

            // 4. Charts: 10y daily and 1M bars, pan/zoom frame times.
            for (label, n) in [("chart_10y_daily", 2_520), ("chart_1m_bars", 1_000_000)] {
                var s = ChartSeries()
                var px: Float = 0
                s.origin = 100
                s.ts = (0..<n).map { Int64($0) * 60_000_000_000 }
                var rng = SystemRandomNumberGenerator()
                for _ in 0..<n {
                    let o = px
                    px += Float.random(in: -0.5...0.5, using: &rng)
                    s.open.append(o); s.close.append(px)
                    s.high.append(max(o, px) + 0.2); s.low.append(min(o, px) - 0.2)
                    s.volume.append(Float.random(in: 1000...5000, using: &rng))
                }
                let view = PriceChartNSView(frame: NSRect(x: 0, y: 0, width: 1600, height: 900))
                let window = NSWindow(contentRect: view.frame, styleMask: [.titled], backing: .buffered, defer: false)
                window.contentView = view
                window.orderFrontRegardless()
                view.series = s
                view.style = .candles
                let times = view.benchmarkFrames(240).map { $0 * 1000 }
                results["\(label)_frame_ms_p50"] = percentile(times, 0.5)
                results["\(label)_frame_ms_p99"] = percentile(times, 0.99)
                window.orderOut(nil)
            }

            let data = try? JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted, .sortedKeys])
            if let out = ProcessInfo.processInfo.environment["MERIDIAN_PERF_OUT"], let data {
                try? data.write(to: URL(fileURLWithPath: out))
            }
            if let data, let s = String(data: data, encoding: .utf8) { print(s) }
            NSApp.terminate(nil)
        }
    }
}
