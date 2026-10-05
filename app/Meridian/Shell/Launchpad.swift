import MeridianCore
import Observation
import SwiftUI

/// Something that owns panels and keyboard focus (main window, Launchpad).
@MainActor
protocol FocusHost: AnyObject {
    var focusedPanel: PanelModel { get }
    func focusNext()
    func focusPrevious()
    func focus(_ i: Int)
    func requestFocus()
}

extension Workspace: FocusHost {
    func focus(_ i: Int) { if panels.indices.contains(i) { focused = i } }
}

/// Launchpad: multiple pages of tiled components, each a full panel with
/// its own command line and link group. Saved in SQLite as layout "launchpad".
@MainActor
@Observable
final class LaunchpadModel: FocusHost {
    static let shared = LaunchpadModel()

    struct Page: Identifiable {
        let id = UUID()
        var name: String
        var columns: Int
        var components: [PanelModel]
    }

    var pages: [Page] = []
    var selected = 0
    var focused = 0 { didSet { focusToken += 1 } }
    private(set) var focusToken = 0
    private var nextIndex = 100

    var page: Page? { pages.indices.contains(selected) ? pages[selected] : nil }
    var focusedPanel: PanelModel { page?.components[safe: focused] ?? AppModel.shared.workspace.focusedPanel }

    func focusNext() { if let n = page?.components.count, n > 0 { focused = (focused + 1) % n } }
    func focusPrevious() { if let n = page?.components.count, n > 0 { focused = (focused + n - 1) % n } }
    func focus(_ i: Int) { if let n = page?.components.count, i < n { focused = i } }
    func requestFocus() { focusToken += 1 }

    private func makeComponent(_ function: String, _ security: String?, group: String?) -> PanelModel {
        nextIndex += 1
        let p = PanelModel(index: nextIndex)
        p.linkGroup = group
        AppModel.shared.links.listen(id: p.id) { [weak p] g, s in
            guard let p, p.linkGroup == g else { return }
            p.linkedSecurityChanged(s)
        }
        p.run(ActionFfi(function: function, security: security, args: []), push: false)
        return p
    }

    func ensureLoaded() {
        guard pages.isEmpty else { return }
        if restore() { return }
        pages = [
            Page(name: "Markets", columns: 3, components: [
                makeComponent("W", nil, group: "A"),
                makeComponent("GP", "SPY US Equity", group: "A"),
                makeComponent("TOP", nil, group: nil),
                makeComponent("WEI", nil, group: "B"),
                makeComponent("FXC", nil, group: nil),
                makeComponent("CRYP", nil, group: "B"),
            ]),
            Page(name: "Research", columns: 2, components: [
                makeComponent("DES", "MSFT US Equity", group: "C"),
                makeComponent("FA", "MSFT US Equity", group: "C"),
                makeComponent("CN", "MSFT US Equity", group: "C"),
                makeComponent("ANR", "MSFT US Equity", group: "C"),
            ]),
        ]
    }

    func addComponent(_ function: String) {
        guard pages.indices.contains(selected) else { return }
        let sec = focusedPanel.security
        pages[selected].components.append(makeComponent(function, sec, group: focusedPanel.linkGroup))
        focused = pages[selected].components.count - 1
        save()
    }

    func removeFocused() {
        guard pages.indices.contains(selected), pages[selected].components.indices.contains(focused) else { return }
        let p = pages[selected].components.remove(at: focused)
        AppModel.shared.links.stopListening(id: p.id)
        focused = max(0, focused - 1)
        save()
    }

    func addPage() {
        pages.append(Page(name: "Page \(pages.count + 1)", columns: 2, components: [makeComponent("W", nil, group: nil)]))
        selected = pages.count - 1
        focused = 0
        save()
    }

    // MARK: Persistence

    private struct Saved: Codable {
        struct P: Codable { var name: String; var columns: Int; var components: [PanelModel.Snapshot] }
        var pages: [P]
    }

    func save() {
        guard let core = AppModel.shared.core else { return }
        let s = Saved(pages: pages.map { .init(name: $0.name, columns: $0.columns, components: $0.components.map(\.snapshot)) })
        if let d = try? JSONEncoder().encode(s), let j = String(data: d, encoding: .utf8) {
            try? core.saveWorkspace(id: "launchpad", name: "Launchpad", json: j)
        }
    }

    private func restore() -> Bool {
        guard let core = AppModel.shared.core,
              let j = try? core.loadWorkspace(id: "launchpad"),
              let d = j.data(using: .utf8),
              let s = try? JSONDecoder().decode(Saved.self, from: d), !s.pages.isEmpty else { return false }
        pages = s.pages.map { sp in
            Page(name: sp.name, columns: sp.columns, components: sp.components.map { snap in
                let p = makeComponent(snap.function ?? "W", snap.security, group: snap.linkGroup)
                p.restore(snap)
                return p
            })
        }
        return true
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}

/// Key handling for Launchpad windows (same terminal keys as the main window).
@MainActor
enum LaunchpadKeys {
    static func handle(_ e: NSEvent) -> Bool {
        guard NSApp.keyWindow?.title.hasPrefix("Launchpad") == true else { return false }
        let lp = LaunchpadModel.shared
        let panel = lp.focusedPanel
        guard let key = KeyRouter.map(e, suggestionsOpen: !panel.suggestions.isEmpty, commandEmpty: panel.commandText.isEmpty) else {
            return false
        }
        switch key {
        case .nextPanel: lp.focusNext()
        case .previousPanel: lp.focusPrevious()
        case let .focusPanel(i): lp.focus(i)
        default: return KeyRouter.apply(key, panel: panel, host: lp)
        }
        return true
    }
}

struct LaunchpadView: View {
    @Bindable var model = LaunchpadModel.shared

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            if let page = model.page {
                GeometryReader { geo in
                    let cols = max(1, page.columns)
                    let rows = max(1, (page.components.count + cols - 1) / cols)
                    let w = (geo.size.width - CGFloat(cols - 1)) / CGFloat(cols)
                    let h = (geo.size.height - CGFloat(rows - 1)) / CGFloat(rows)
                    VStack(spacing: 1) {
                        ForEach(0..<rows, id: \.self) { r in
                            HStack(spacing: 1) {
                                ForEach(0..<cols, id: \.self) { c in
                                    let i = r * cols + c
                                    if i < page.components.count {
                                        PanelView(panel: page.components[i], focused: model.focused == i, focusToken: model.focusToken) {
                                            if model.focused != i { model.focused = i } else { model.requestFocus() }
                                        }
                                        .frame(width: w, height: h)
                                        .clipped()
                                    } else {
                                        Color.black.frame(width: w, height: h)
                                    }
                                }
                            }
                        }
                    }
                    .background(Theme.grid.swiftUI)
                }
            }
        }
        .background(Color.black)
        .preferredColorScheme(.dark)
        .onAppear { model.ensureLoaded() }
        .onDisappear { model.save() }
    }

    private var toolbar: some View {
        HStack(spacing: 12) {
            ForEach(Array(model.pages.enumerated()), id: \.element.id) { i, p in
                Text(p.name)
                    .font(Theme.swiftFont(weight: .bold))
                    .foregroundStyle(model.selected == i ? Color.black : Theme.amber.swiftUI)
                    .padding(.horizontal, 6)
                    .background(model.selected == i ? Theme.amber.swiftUI : Color.clear)
                    .onTapGesture { model.selected = i; model.focused = 0 }
            }
            Text("+ Page").font(Theme.swiftFont()).foregroundStyle(Theme.muted.swiftUI).onTapGesture { model.addPage() }
            Spacer()
            Text("Add:").font(Theme.swiftFont(11)).foregroundStyle(Theme.muted.swiftUI)
            ForEach(["W", "GP", "GIP", "TOP", "CN", "WEI", "CRYP", "FXC", "ECO", "OMON"], id: \.self) { f in
                Text(f).font(Theme.swiftFont(11, weight: .bold)).foregroundStyle(Theme.white.swiftUI)
                    .onTapGesture { model.addComponent(f) }
            }
            Text("Remove").font(Theme.swiftFont(11)).foregroundStyle(Theme.down.swiftUI).onTapGesture { model.removeFocused() }
            Stepper("Cols \(model.page?.columns ?? 1)", value: Binding(
                get: { model.page?.columns ?? 1 },
                set: { if model.pages.indices.contains(model.selected) { model.pages[model.selected].columns = min(max($0, 1), 4); model.save() } }
            ), in: 1...4)
            .font(Theme.swiftFont(11))
            .fixedSize()
        }
        .padding(.horizontal, 8)
        .frame(height: 24)
        .background(Color.black)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.grid.swiftUI).frame(height: 1) }
    }
}
