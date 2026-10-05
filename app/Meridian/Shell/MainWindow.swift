import MeridianCore
import SwiftUI

/// The default four-panel window.
struct MainWindow: View {
    @Bindable var app = AppModel.shared
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(spacing: 0) {
            StatusBar()
            if let err = app.startupError {
                Text(err)
                    .font(Theme.swiftFont())
                    .foregroundStyle(Theme.down.swiftUI)
                    .padding()
                Spacer()
            } else {
                panelGrid
            }
        }
        .background(Color.black)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $app.showKeyboardOverlay) { KeyboardOverlay() }
        .onReceive(NotificationCenter.default.publisher(for: .openLaunchpad)) { _ in
            openWindow(id: "launchpad", value: "1")
        }
    }

    private var panelGrid: some View {
        let ws = app.workspace
        return GeometryReader { geo in
            let w = (geo.size.width - 1) / 2
            let h = (geo.size.height - 1) / 2
            VStack(spacing: 1) {
                HStack(spacing: 1) {
                    cell(ws, 0).frame(width: w, height: h)
                    cell(ws, 1).frame(width: w, height: h)
                }
                HStack(spacing: 1) {
                    cell(ws, 2).frame(width: w, height: h)
                    cell(ws, 3).frame(width: w, height: h)
                }
            }
            .background(Theme.grid.swiftUI)
        }
    }

    private func cell(_ ws: Workspace, _ i: Int) -> some View {
        PanelView(panel: ws.panels[i], focused: ws.focused == i, focusToken: ws.focusToken) {
            if ws.focused != i { ws.focused = i } else { ws.requestFocus() }
        }
        .clipped()
    }
}

struct StatusBar: View {
    @Bindable var app = AppModel.shared
    @State private var now = Date()
    private let timer = Timer.publish(every: 1, on: .main, in: .common).autoconnect()
    private static let clock: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "EEE MMM dd HH:mm:ss"
        return f
    }()

    var body: some View {
        HStack(spacing: 14) {
            Text("MERIDIAN").font(Theme.swiftFont(weight: .bold)).foregroundStyle(Theme.white.swiftUI)
            if app.mode == .mock {
                Text("MOCK DATA").font(Theme.swiftFont(weight: .bold)).foregroundStyle(.white)
                    .padding(.horizontal, 6).background(Theme.mockBadge.swiftUI)
                    .help("All data is synthetic. Switch to LIVE in Settings (⌘,).")
            } else {
                Text("LIVE").font(Theme.swiftFont(weight: .bold)).foregroundStyle(.black)
                    .padding(.horizontal, 6).background(Theme.up.swiftUI)
            }
            ForEach(app.feeds) { f in
                HStack(spacing: 4) {
                    Circle().fill((f.connected ? Theme.up : Theme.down).swiftUI).frame(width: 7, height: 7)
                    Text(f.provider).font(Theme.swiftFont(11)).foregroundStyle(Theme.muted.swiftUI)
                }
                .help(f.message)
            }
            if app.instrumentsLoaded > 0 {
                Text("\(app.instrumentsLoaded) securities").font(Theme.swiftFont(11)).foregroundStyle(Theme.muted.swiftUI)
            }
            if let a = app.lastAlert {
                Text("ALERT: \(a)").font(Theme.swiftFont(11)).foregroundStyle(Theme.warning.swiftUI).lineLimit(1)
            }
            Spacer()
            Text("⌘/ keys").font(Theme.swiftFont(11)).foregroundStyle(Theme.muted.swiftUI)
            Text(Self.clock.string(from: now)).font(Theme.swiftFont(11)).foregroundStyle(Theme.amber.swiftUI)
        }
        .padding(.horizontal, 8)
        .frame(height: 20)
        .background(Color.black)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.grid.swiftUI).frame(height: 1) }
        .onReceive(timer) { now = $0 }
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
        VStack(alignment: .leading, spacing: 4) {
            Text("KEYBOARD").font(Theme.swiftFont(weight: .bold)).foregroundStyle(Theme.white.swiftUI)
            Text("Mac F-keys send media keys unless fn is held or “Use F1, F2… as standard function keys” is on; ⌥1–⌥0 always work.")
                .font(Theme.swiftFont(11)).foregroundStyle(Theme.muted.swiftUI)
            ForEach(Self.rows, id: \.0) { r in
                HStack {
                    Text(r.0).font(Theme.swiftFont(weight: .medium)).foregroundStyle(Theme.amber.swiftUI).frame(width: 180, alignment: .leading)
                    Text(r.1).font(Theme.swiftFont()).foregroundStyle(Theme.white.swiftUI)
                }
            }
            HStack { Spacer(); Button("Close") { dismiss() }.keyboardShortcut(.cancelAction) }
        }
        .padding(16)
        .frame(width: 560)
        .background(Color.black)
    }
}
