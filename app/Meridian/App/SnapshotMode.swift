import AppKit
import MeridianCore
import SwiftUI

/// Offscreen rendering of function screens to PNG, for snapshot tests,
/// `scripts/capture`, and fidelity comparisons. Driven by environment:
///
///   MERIDIAN_SNAPSHOT_DIR=/path      output directory (enables the mode)
///   MERIDIAN_SNAPSHOT_SPECS="DES|AAPL US Equity;GP|AAPL US Equity|range=1Y"
///   MERIDIAN_SNAPSHOT_SIZE=960x600   panel size (default 960x600)
///   MERIDIAN_SNAPSHOT_MAIN=1         also render the whole main window
///
/// Use with MERIDIAN_MODE=mock, MERIDIAN_IN_MEMORY=1 and
/// MERIDIAN_FIXED_CLOCK_NS for deterministic output.
@MainActor
enum SnapshotMode {
    static var enabled: Bool { ProcessInfo.processInfo.environment["MERIDIAN_SNAPSHOT_DIR"] != nil }

    struct Spec {
        var function: String
        var security: String?
        var args: [KeyValue]

        var fileName: String {
            let sec = security.map { "_" + $0.replacingOccurrences(of: " ", with: "-") } ?? ""
            let a = args.map { "_\($0.key)-\($0.value)" }.joined()
            return "\(function)\(sec)\(a)".replacingOccurrences(of: "/", with: "-") + ".png"
        }
    }

    static func parse(_ s: String) -> [Spec] {
        s.split(separator: ";").compactMap { item in
            let parts = item.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
            guard let f = parts.first, !f.isEmpty else { return nil }
            let sec = parts.count > 1 && !parts[1].isEmpty ? parts[1] : nil
            let args = parts.dropFirst(2).compactMap { kv -> KeyValue? in
                let p = kv.split(separator: "=", maxSplits: 1).map(String.init)
                return p.count == 2 ? KeyValue(key: p[0], value: p[1]) : nil
            }
            return Spec(function: f.uppercased(), security: sec, args: Array(args))
        }
    }

    static func run() {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["MERIDIAN_SNAPSHOT_DIR"] else { return }
        let out = URL(fileURLWithPath: dir, isDirectory: true)
        try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let size: CGSize = {
            let p = (env["MERIDIAN_SNAPSHOT_SIZE"] ?? "960x600").split(separator: "x").compactMap { Double($0) }
            return p.count == 2 ? CGSize(width: p[0], height: p[1]) : CGSize(width: 960, height: 600)
        }()
        let specs = parse(env["MERIDIAN_SNAPSHOT_SPECS"] ?? "")
        Task { @MainActor in
            // Let the universe load and streams tick so live columns fill.
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            for (i, spec) in specs.enumerated() {
                let panel = PanelModel(index: 900 + i)
                panel.run(ActionFfi(function: spec.function, security: spec.security, args: spec.args), push: false)
                var waited = 0
                while (panel.loading || (panel.screen == nil && panel.special == .none)) && waited < 200 {
                    try? await Task.sleep(nanoseconds: 50_000_000)
                    waited += 1
                }
                // Charts load asynchronously inside the view.
                try? await Task.sleep(nanoseconds: 700_000_000)
                let view = PanelView(panel: panel, focused: false, focusToken: 0)
                    .frame(width: size.width, height: size.height)
                    .background(Color.black)
                    .environment(\.colorScheme, .dark)
                render(view, size: size, to: out.appendingPathComponent(spec.fileName))
            }
            if env["MERIDIAN_SNAPSHOT_MAIN"] == "1" {
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                if let w = NSApp.windows.first(where: { $0.title == "Meridian" }), let v = w.contentView {
                    writePNG(v, to: out.appendingPathComponent("main-window.png"))
                }
            }
            print("snapshots written to \(out.path)")
            NSApp.terminate(nil)
        }
    }

    static func render<V: View>(_ view: V, size: CGSize, to url: URL) {
        let host = NSHostingView(rootView: view)
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        host.displayIfNeeded()
        // One more pass so AppKit-backed subviews (grids, charts) draw.
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        host.layoutSubtreeIfNeeded()
        writePNG(host, to: url)
        window.orderOut(nil)
    }

    static func writePNG(_ view: NSView, to url: URL) {
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        if let data = rep.representation(using: .png, properties: [:]) {
            try? data.write(to: url)
        }
    }
}
