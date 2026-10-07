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
///   MERIDIAN_SNAPSHOT_SETUP=1        also render Settings → Setup
///   MERIDIAN_SNAPSHOT_WAIT=10        seconds to wait for each screen
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
                // MERIDIAN_SNAPSHOT_WAIT: seconds to wait for a screen (default 10).
                let limit = Int((Double(env["MERIDIAN_SNAPSHOT_WAIT"] ?? "") ?? 10) * 20)
                var waited = 0
                let started = Date()
                while (panel.loading || (panel.screen == nil && panel.special == .none)) && waited < limit {
                    try? await Task.sleep(nanoseconds: 50_000_000)
                    waited += 1
                }
                print("\(spec.function): \(panel.loading ? "still loading" : "loaded") after \(String(format: "%.1f", Date().timeIntervalSince(started))) s")
                // Charts load asynchronously inside the view.
                try? await Task.sleep(nanoseconds: 700_000_000)
                let view = PanelView(panel: panel, focused: false, focusToken: 0)
                    .frame(width: size.width, height: size.height)
                    .background(Color.black)
                    .environment(\.colorScheme, .dark)
                await render(view, size: size, to: out.appendingPathComponent(spec.fileName))
            }
            if env["MERIDIAN_SNAPSHOT_SETUP"] == "1" {
                let panes: [(String, AnyView)] = [
                    ("sources", AnyView(DataSourcesPane())), ("general", AnyView(GeneralPane())),
                    ("storage", AnyView(StoragePane())), ("keyboard", AnyView(KeyboardPane())), ("about", AnyView(AboutPane())),
                ]
                for (name, pane) in panes {
                    let v = pane.frame(width: 860, height: 600).background(Color(nsColor: .windowBackgroundColor))
                    await render(v, size: CGSize(width: 860, height: 600), to: out.appendingPathComponent("settings-\(name).png"))
                }
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

    /// Waits by suspending (never by spinning the run loop inside this
    /// main-actor task): chart loads resume on the main actor, and a blocked
    /// actor would leave them in flight until the timeout.
    static func render<V: View>(_ view: V, size: CGSize, to url: URL) async {
        let host = NSHostingView(rootView: view)
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        // SwiftUI only draws its layers for ordered-in windows.
        window.orderFrontRegardless()
        host.layoutSubtreeIfNeeded()
        host.displayIfNeeded()
        try? await Task.sleep(nanoseconds: 400_000_000)
        // Wait for async chart data (network in LIVE mode), up to 15 s.
        var waited = 0.0
        while ChartBlockView.inFlight > 0 && waited < 15 {
            try? await Task.sleep(nanoseconds: 100_000_000)
            waited += 0.1
        }
        try? await Task.sleep(nanoseconds: 500_000_000)
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
