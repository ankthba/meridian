import Foundation
import MeridianCore

extension Notification.Name {
    static let openLaunchpad = Notification.Name("meridian.openLaunchpad")
}
import Observation

/// Security linking between panels and Launchpad components. Groups are
/// letters (A, B, C, D), as in the incumbent's Group Manager.
@MainActor
@Observable
final class LinkBus {
    static let groups = ["A", "B", "C", "D"]
    private(set) var latest: [String: String] = [:]
    private var listeners: [String: (String, String) -> Void] = [:]

    func publish(group: String, security: String, from source: String) {
        latest[group] = security
        for (id, l) in listeners where id != source {
            l(group, security)
        }
    }

    func listen(id: String, _ f: @escaping (_ group: String, _ security: String) -> Void) {
        listeners[id] = f
    }

    func stopListening(id: String) {
        listeners[id] = nil
    }
}

/// The main window's four panels plus focus, persisted in SQLite.
@MainActor
@Observable
final class Workspace {
    let panels: [PanelModel] = (0..<4).map { PanelModel(index: $0) }
    var focused = 0 { didSet { focusToken += 1 } }
    /// Bumped to ask the focused panel's command line to take focus.
    private(set) var focusToken = 0
    private weak var core: Core?

    func requestFocus() { focusToken += 1 }

    var focusedPanel: PanelModel { panels[focused] }

    func focusNext() { focused = (focused + 1) % panels.count }
    func focusPrevious() { focused = (focused + panels.count - 1) % panels.count }

    /// Restores the saved layout, or opens the default four screens.
    func restore(core: Core) {
        self.core = core
        for p in panels {
            AppModel.shared.links.listen(id: p.id) { [weak p] group, security in
                guard let p, p.linkGroup == group else { return }
                p.linkedSecurityChanged(security)
            }
        }
        let restoreSaved = UserDefaults.standard.object(forKey: Preference.restoreWorkspace) as? Bool ?? true
        if restoreSaved,
           let json = try? core.loadWorkspace(id: "main"),
           let data = json.data(using: .utf8),
           let snaps = try? JSONDecoder().decode([PanelModel.Snapshot].self, from: data),
           snaps.count == panels.count,
           ProcessInfo.processInfo.environment["MERIDIAN_RESET_LAYOUT"] != "1" {
            for (p, s) in zip(panels, snaps) { p.restore(s) }
            return
        }
        resetToDefaults()
    }

    /// HELP, a worksheet, a price graph and top news.
    func resetToDefaults() {
        let defaults: [(String, String?, String?)] = [
            ("HELP", nil, nil),
            ("W", nil, "A"),
            ("GP", "AAPL US Equity", "A"),
            ("TOP", nil, "B"),
        ]
        for (p, d) in zip(panels, defaults) {
            p.linkGroup = d.2
            p.run(ActionFfi(function: d.0, security: d.1, args: []), push: false)
        }
        focused = 0
    }

    func save() {
        guard let core else { return }
        let snaps = panels.map(\.snapshot)
        if let data = try? JSONEncoder().encode(snaps), let json = String(data: data, encoding: .utf8) {
            try? core.saveWorkspace(id: "main", name: "Main", json: json)
        }
    }
}
