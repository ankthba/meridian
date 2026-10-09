import MeridianCore
import SwiftUI

/// The main window: top bar, the command bar (runs in the focused pane),
/// and four panes. Layout per `docs/DESIGN.md`.
struct MainWindow: View {
    @Bindable var app = AppModel.shared
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @State private var importRequest: ImportRequest?

    var body: some View {
        let ws = app.workspace
        VStack(spacing: 0) {
            TopBar()
            if let err = app.startupError {
                Text(err)
                    .font(Theme.ui(13))
                    .foregroundStyle(Theme.down.swiftUI)
                    .padding()
                Spacer()
            } else {
                ZStack(alignment: .topLeading) {
                    VStack(spacing: 0) {
                        CommandBar(panel: ws.focusedPanel, focusToken: ws.focusToken)
                        panelGrid(ws)
                    }
                    if !ws.focusedPanel.suggestions.isEmpty {
                        CompletionPopover(panel: ws.focusedPanel)
                            .padding(.top, 44)
                            .padding(.leading, 14)
                            .zIndex(10)
                    }
                }
            }
        }
        .background(Theme.bg.swiftUI)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $app.showKeyboardOverlay) { KeyboardOverlay() }
        .onReceive(NotificationCenter.default.publisher(for: .openLaunchpad)) { _ in
            openWindow(id: "launchpad", value: "1")
        }
        .onReceive(NotificationCenter.default.publisher(for: .openSettingsRequest)) { _ in
            openSettings()
        }
        .onReceive(NotificationCenter.default.publisher(for: .openImport)) { n in
            importRequest = ImportRequest(portfolioId: n.object as? Int64)
        }
        .sheet(item: $importRequest) { req in
            ImportSheet(model: ImportModel(portfolioId: req.portfolioId))
        }
        .task {
            // Without stock data or EDGAR most screens are empty: open Settings.
            let wanted = UserDefaults.standard.object(forKey: Preference.openSetupAtLaunch) as? Bool ?? true
            if wanted, app.mode == .live, !SnapshotMode.enabled, !PerfHarness.enabled, DataSource.essentialMissing {
                openSettings()
            }
        }
    }

    /// 2 × 2 panes, columns 1.45 : 1 and rows 1.2 : 1, hairline borders.
    private func panelGrid(_ ws: Workspace) -> some View {
        GeometryReader { geo in
            let w0 = ((geo.size.width - 1) * 1.45 / 2.45).rounded()
            let w1 = geo.size.width - 1 - w0
            let h0 = ((geo.size.height - 1) * 1.2 / 2.2).rounded()
            let h1 = geo.size.height - 1 - h0
            VStack(spacing: 1) {
                HStack(spacing: 1) {
                    cell(ws, 0).frame(width: w0, height: h0)
                    cell(ws, 1).frame(width: w1, height: h0)
                }
                HStack(spacing: 1) {
                    cell(ws, 2).frame(width: w0, height: h1)
                    cell(ws, 3).frame(width: w1, height: h1)
                }
            }
            .background(Theme.line.swiftUI)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Panes")
    }

    private func cell(_ ws: Workspace, _ i: Int) -> some View {
        PanelView(panel: ws.panels[i], focused: ws.focused == i, focusToken: ws.focusToken) {
            if ws.focused != i { ws.focused = i } else { ws.requestFocus() }
        }
        .clipped()
    }
}

/// Wordmark, data status and the clock.
struct TopBar: View {
    @Bindable var app = AppModel.shared
    @Environment(\.openSettings) private var openSettings
    @State private var now = Date()
    private let timer = Timer.publish(every: 1, on: .main, in: .common).autoconnect()
    private static let clock: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f
    }()

    var body: some View {
        HStack(spacing: 14) {
            Text("MERIDIAN")
                .font(Theme.ui(11, weight: .bold))
                .tracking(3)
                .foregroundStyle(Theme.text.swiftUI)
                .padding(.leading, 74) // clear the window controls
                .accessibilityLabel("Meridian")
            if let a = app.lastAlert {
                Text("Alert · \(a)").font(Theme.ui(12)).foregroundStyle(Theme.warn.swiftUI).lineLimit(1)
            }
            Spacer()
            if app.mode == .mock {
                Text("Mock data · tests only")
                    .font(Theme.ui(12, weight: .semibold))
                    .foregroundStyle(Theme.warn.swiftUI)
                    .help("Synthetic data for automated tests (MERIDIAN_MODE=mock).")
            } else if app.missingSetup > 0 {
                Button { openSettings() } label: {
                    Text("\(app.missingSetup) source\(app.missingSetup == 1 ? "" : "s") not set up")
                        .font(Theme.ui(12))
                        .foregroundStyle(Theme.warn.swiftUI)
                }
                .buttonStyle(.plain)
                .help("Open Settings → Data Sources (⌘,)")
            }
            HStack(spacing: 6) {
                Circle().fill(liveColor.swiftUI).frame(width: 6, height: 6)
                Text(app.mode == .mock ? "Mock" : "Live").font(Theme.ui(12)).foregroundStyle(Theme.text2.swiftUI)
            }
            .help(app.feeds.map { "\($0.provider): \($0.message)" }.joined(separator: "\n"))
            // The dot's color is the feed health; say it in words.
            .accessibilityElement(children: .ignore)
            .accessibilityAddTraits(.isStaticText)
            .accessibilityLabel(app.mode == .mock ? "Mock data" : "Live")
            .accessibilityValue(app.mode == .mock ? "" : feedFailed ? "a data feed has failed" : "feeds healthy")
            Text(app.feeds.map(\.provider).joined(separator: " · "))
                .font(Theme.ui(12))
                .foregroundStyle(Theme.muted.swiftUI)
                .lineLimit(1)
                .accessibilityLabel(app.feeds.isEmpty ? "" : "Feeds: " + app.feeds.map(\.provider).joined(separator: ", "))
            Text(Self.clock.string(from: now))
                .font(Theme.swiftFont(12))
                .foregroundStyle(Theme.text2.swiftUI)
                .accessibilityLabel("Time")
                .accessibilityValue(Self.clock.string(from: now))
        }
        .padding(.trailing, 14)
        .frame(height: 40)
        .background(Theme.bg.swiftUI)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.line.swiftUI).frame(height: 1) }
        .onReceive(timer) { now = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Status")
    }

    private var feedFailed: Bool {
        app.feeds.contains { !$0.connected && !$0.message.hasPrefix("idle") && !$0.message.hasPrefix("reconnecting with") }
    }

    /// Green when every connected-on-demand feed is healthy; red if one failed.
    private var liveColor: NSColor {
        if app.mode == .mock { return Theme.warn }
        return feedFailed ? Theme.down : Theme.up
    }
}

/// ⌘/ — key mapping reference.
struct KeyboardOverlay: View {
    @Environment(\.dismiss) private var dismiss
    static let rows: [(String, String)] = [
        ("GO", "Return"),
        ("CANCEL", "Esc"),
        ("MENU (back)", "⌘[  ·  End  ·  Delete on empty line"),
        ("HELP", "F1  ·  ⌘?   (twice: function directory)"),
        ("PAGE FWD / BACK", "PgDn / PgUp  ·  ⌘↓ / ⌘↑"),
        ("PANEL (next / prev)", "⌃Tab / ⌃⇧Tab  ·  ⌘1–⌘4"),
        ("Autocomplete", "↑ ↓ to choose · Tab to accept"),
        ("Numbered item", "<n> Return"),
        ("GOVT", "F2 · ⌥1"), ("CORP", "F3 · ⌥2"), ("MTGE", "F4 · ⌥3"), ("M-MKT", "F5 · ⌥4"), ("MUNI", "F6 · ⌥5"),
        ("PFD", "F7 · ⌥6"), ("EQUITY", "F8 · ⌥7"), ("CMDTY", "F9 · ⌥8"), ("INDEX", "F10 · ⌥9"), ("CRNCY", "F11 · ⌥0"),
        ("Launchpad", "BLP <GO>"),
        ("Settings / API keys", "⌘,"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("KEYBOARD").font(Theme.label()).tracking(1.4).foregroundStyle(Theme.muted.swiftUI)
                .accessibilityLabel("Keyboard")
                .accessibilityAddTraits(.isHeader)
            Text("Type plain commands like aapl 5y or aapl filings; the keys below are shortcuts. Mac F-keys send media keys unless fn is held; ⌥1–⌥0 always work.")
                .font(Theme.ui(12)).foregroundStyle(Theme.text2.swiftUI)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.vertical, 8)
            ForEach(Self.rows, id: \.0) { r in
                HStack {
                    Text(r.0).font(Theme.ui(12.5)).foregroundStyle(Theme.text2.swiftUI).frame(width: 180, alignment: .leading)
                    Text(r.1).font(Theme.swiftFont(12)).foregroundStyle(Theme.text.swiftUI)
                    Spacer()
                }
                .padding(.vertical, 4)
                .overlay(alignment: .bottom) { Rectangle().fill(Theme.hairline.swiftUI).frame(height: 1) }
                .accessibilityElement(children: .combine)
            }
            HStack { Spacer(); Button("Close") { dismiss() }.keyboardShortcut(.cancelAction) }.padding(.top, 12)
        }
        .padding(20)
        .frame(width: 600)
        .background(Theme.raised.swiftUI)
    }
}
