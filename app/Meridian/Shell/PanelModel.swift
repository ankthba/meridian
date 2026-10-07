import Foundation
import MeridianCore
import Observation

/// One terminal panel: command line, loaded security, function stack,
/// current screen. All data logic lives in Rust; this type only routes
/// commands and keeps navigation state.
@MainActor
@Observable
final class PanelModel: Identifiable {
    let id: String
    let index: Int
    var commandText = ""
    private(set) var security: String?
    private(set) var screen: ScreenFfi?
    private(set) var current: ActionFfi?
    private(set) var history: [ActionFfi] = []
    private(set) var loading = false
    var message: String?
    var suggestions: [SuggestionFfi] = []
    var highlighted: Int?
    /// Current page per table block index.
    var pages: [Int: Int] = [:]
    var linkGroup: String?
    /// Edits to input cells not yet applied.
    var pendingInputs: [String: String] = [:]
    /// Views implemented in Swift rather than as Rust screen models.
    enum Special: Equatable { case none, ask }
    /// A question typed as "ask …" in the command bar, sent when ASK opens.
    var pendingQuestion: String?
    private(set) var special: Special = .none
    /// Live quotes for the current screen's bound rows and chart.
    private(set) var feed: QuoteFeed?
    private var feedSecurities: [String] = []
    private var refreshTask: Task<Void, Never>?
    private var loadTask: Task<Void, Never>?
    private var generation = 0

    init(index: Int) {
        self.index = index
        self.id = "panel\(index + 1)"
    }

    var core: Core? { AppModel.shared.core }

    // MARK: Command line

    /// Recomputes autocomplete for the current command text.
    func updateSuggestions() {
        message = nil
        highlighted = nil
        let text = commandText.trimmingCharacters(in: .whitespaces)
        guard let core, !text.isEmpty, Int(text) == nil else {
            suggestions = []
            return
        }
        suggestions = (try? core.suggest(input: commandText, loaded: security, limit: 12)) ?? []
        // The row GO would run is highlighted, so Return does what was typed.
        highlighted = suggestions.firstIndex(where: \.best)
    }

    func moveHighlight(_ delta: Int) {
        guard !suggestions.isEmpty else { return }
        let n = suggestions.count
        highlighted = ((highlighted ?? (delta > 0 ? -1 : n)) + delta + n) % n
    }

    /// Accepts the highlighted suggestion into the command line.
    func acceptSuggestion() -> Bool {
        guard let i = highlighted, suggestions.indices.contains(i) else { return false }
        commandText = suggestions[i].completion
        suggestions = []
        highlighted = nil
        return true
    }

    /// GO.
    func go() {
        // A highlighted suggestion with a resolved action runs directly.
        if let i = highlighted, suggestions.indices.contains(i), let action = suggestions[i].action {
            let typed = commandText.trimmingCharacters(in: .whitespaces)
            if !typed.isEmpty { try? core?.pushHistory(panel: id, command: typed) }
            commandText = ""
            suggestions = []
            highlighted = nil
            run(action)
            return
        }
        if acceptSuggestion() {
            // A completion that already names a function runs immediately.
            if case .security(_, .some, _)? = try? core?.parseCommand(input: commandText, loaded: security) {} else { updateSuggestions(); return }
        }
        let input = commandText.trimmingCharacters(in: .whitespaces)
        suggestions = []
        guard let core else { return }
        if input.isEmpty {
            if let current { run(current, push: false) }
            return
        }
        try? core.pushHistory(panel: id, command: input)
        commandText = ""
        let parsed = (try? core.parseCommand(input: input, loaded: security)) ?? .empty
        switch parsed {
        case .empty:
            break
        case let .security(sec, fn, args):
            setSecurity(sec, broadcast: true)
            run(ActionFfi(function: fn ?? "MENU", security: sec, args: Self.namedArgs(fn ?? "MENU", args)))
        case let .function(fn, args):
            run(ActionFfi(function: fn, security: security, args: Self.namedArgs(fn, args)))
        case let .menuItem(n):
            select(number: Int(n))
        case let .search(text):
            run(ActionFfi(function: "SECF", security: nil, args: [KeyValue(key: "q", value: text)]))
        case let .run(action):
            run(action)
        }
    }

    /// Positional command-line arguments → named screen arguments.
    static func namedArgs(_ function: String, _ args: [String]) -> [KeyValue] {
        guard !args.isEmpty else { return [] }
        switch function {
        case "GP", "GIP": return [KeyValue(key: "range", value: args[0])]
        case "N", "CN", "TOP", "SECF": return [KeyValue(key: "q", value: args.joined(separator: " "))]
        case "FA": return [KeyValue(key: "stmt", value: args[0])]
        case "HP": return [KeyValue(key: "period", value: args[0].capitalized)]
        case "HELP": return [KeyValue(key: "topic", value: args[0].uppercased())]
        default: return []
        }
    }

    /// CANCEL.
    func cancel() {
        if !commandText.isEmpty || !suggestions.isEmpty {
            commandText = ""
            suggestions = []
            message = nil
            return
        }
        loadTask?.cancel()
        loading = false
    }

    /// MENU: back one level.
    func menuBack() {
        guard let prev = history.popLast() else {
            if let sec = security, current?.function != "MENU" {
                run(ActionFfi(function: "MENU", security: sec, args: []), push: false)
            }
            return
        }
        if let s = prev.security { setSecurity(s, broadcast: false) }
        run(prev, push: false)
    }

    // MARK: Navigation

    func setSecurity(_ s: String, broadcast: Bool) {
        security = s
        if broadcast, let g = linkGroup {
            AppModel.shared.links.publish(group: g, security: s, from: id)
        }
    }

    /// Called when another member of our link group loads a security.
    func linkedSecurityChanged(_ s: String) {
        guard s != security else { return }
        security = s
        if let cur = current, cur.security != nil {
            run(ActionFfi(function: cur.function, security: s, args: []), push: true)
        }
    }

    /// Re-runs the current screen (data sources changed).
    func reload() {
        if let current { run(current, push: false) }
    }

    func run(_ action: ActionFfi, push: Bool = true) {
        guard let core else { return }
        // App-level actions from plain commands ("import", "settings").
        switch action.function {
        case "BLP":
            NotificationCenter.default.post(name: .openLaunchpad, object: nil)
            return
        case "IMPORT":
            let pid = action.args.first { $0.key == "portfolio" }.flatMap { Int64($0.value) }
            NotificationCenter.default.post(name: .openImport, object: pid)
            return
        case "SETTINGS":
            NotificationCenter.default.post(name: .openSettingsRequest, object: nil)
            return
        default:
            break
        }
        if push, let cur = current, cur != action { history.append(cur) }
        if history.count > 100 { history.removeFirst(history.count - 100) }
        current = action
        if let s = action.security, s != security { setSecurity(s, broadcast: true) }
        pendingInputs = [:]
        pages = [:]
        if action.function == "ASK" {
            pendingQuestion = action.args.first { $0.key == "q" }?.value
            special = .ask
            screen = nil
            loading = false
            return
        }
        special = .none
        loading = true
        message = nil
        generation += 1
        let gen = generation
        refreshTask?.cancel()
        loadTask?.cancel()
        loadTask = Task { [weak self] in
            do {
                let s = try await core.screen(function: action.function, security: action.security, args: action.args)
                guard let self, gen == self.generation, !Task.isCancelled else { return }
                self.screen = s
                self.loading = false
                self.updateFeed(for: s)
                self.scheduleRefresh(s)
            } catch {
                guard let self, gen == self.generation else { return }
                self.loading = false
                self.message = error.userMessage
            }
        }
    }

    /// Securities whose live values the screen displays.
    static func liveSecurities(_ s: ScreenFfi) -> [String] {
        var out: [String] = []
        for b in s.blocks {
            switch b {
            case let .table(t) where t.columns.contains(where: { $0.live != nil }):
                out.append(contentsOf: t.rows.compactMap(\.security))
            case let .chart(spec):
                out.append(spec.security)
            default:
                break
            }
        }
        if let sec = s.security { out.append(sec) }
        var seen = Set<String>()
        return out.filter { seen.insert($0).inserted }
    }

    private func updateFeed(for s: ScreenFfi) {
        let secs = Self.liveSecurities(s)
        guard secs != feedSecurities else { return }
        feed?.stop()
        feedSecurities = secs
        feed = core.flatMap { QuoteFeed(core: $0, securities: secs) }
    }

    private func scheduleRefresh(_ s: ScreenFfi) {
        guard let ms = s.refreshMs, ms > 0, let action = current else { return }
        refreshTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(ms) * 1_000_000)
                guard let self, !Task.isCancelled, let core = self.core, self.current == action else { return }
                if let s = try? await core.screen(function: action.function, security: action.security, args: action.args), self.current == action {
                    self.screen = s
                }
            }
        }
    }

    /// Re-runs the current function with one argument changed (input cells).
    func applyInput(id: String, value: String) {
        guard var a = current else { return }
        a.args.removeAll { $0.key == id }
        a.args.append(KeyValue(key: id, value: value))
        run(a, push: false)
    }

    /// Runs the current function with every pending input applied.
    func applyPendingInputs(extra: [KeyValue] = []) {
        guard var a = current else { return }
        for (k, v) in pendingInputs {
            a.args.removeAll { $0.key == k }
            a.args.append(KeyValue(key: k, value: v))
        }
        for kv in extra {
            a.args.removeAll { $0.key == kv.key }
            a.args.append(kv)
        }
        run(a, push: false)
    }

    // MARK: Numbered selection & paging

    /// First paged table in the screen, with its block index.
    private var pagedTable: (Int, TableFfi)? {
        guard let blocks = screen?.blocks else { return nil }
        for (i, b) in blocks.enumerated() {
            if case let .table(t) = b, t.numbered || t.pageSize != nil { return (i, t) }
        }
        return nil
    }

    /// `<n> <GO>`: numbered table row on the current page, else menu item.
    func select(number n: Int) {
        if let (i, t) = pagedTable, t.numbered, n >= 1, n <= t.rows.count {
            if let a = t.rows[n - 1].action {
                if a.function == current?.function, a.security == current?.security {
                    // In-screen action (e.g. open story): merge with current args.
                    var merged = current!
                    for kv in a.args {
                        merged.args.removeAll { $0.key == kv.key }
                        merged.args.append(kv)
                    }
                    run(merged)
                } else {
                    run(a)
                }
                _ = i
                return
            }
        }
        if let item = screen?.menu.first(where: { Int($0.number) == n }) {
            runRowAction(item.action)
            return
        }
        message = "No item \(n)"
    }

    func pageForward() { page(by: 1) }
    func pageBack() { page(by: -1) }

    private func page(by d: Int) {
        guard let (i, t) = pagedTable, let size = t.pageSize, size > 0 else { return }
        let pagesCount = max(1, (t.rows.count + Int(size) - 1) / Int(size))
        pages[i] = min(max(0, (pages[i] ?? 0) + d), pagesCount - 1)
    }

    func runRowAction(_ a: ActionFfi) {
        if a.function == current?.function, a.security == current?.security, let cur = current {
            var merged = cur
            // Typed-but-unapplied input cells travel with in-screen actions
            // (e.g. PORT "Add Transaction", ALRT "Create Alert").
            for (k, v) in pendingInputs {
                merged.args.removeAll { $0.key == k }
                merged.args.append(KeyValue(key: k, value: v))
            }
            for kv in a.args {
                merged.args.removeAll { $0.key == kv.key }
                merged.args.append(kv)
            }
            run(merged)
        } else {
            run(a)
        }
    }

    // MARK: Persistence

    struct Snapshot: Codable {
        var function: String?
        var security: String?
        var args: [[String]]
        var linkGroup: String?
    }

    var snapshot: Snapshot {
        Snapshot(function: current?.function, security: security, args: current?.args.map { [$0.key, $0.value] } ?? [], linkGroup: linkGroup)
    }

    func restore(_ s: Snapshot) {
        linkGroup = s.linkGroup
        if let sec = s.security { security = sec }
        if let f = s.function {
            run(ActionFfi(function: f, security: s.security, args: s.args.compactMap { $0.count == 2 ? KeyValue(key: $0[0], value: $0[1]) : nil }), push: false)
        }
    }
}
