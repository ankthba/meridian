import AppKit
import MeridianCore

/// Terminal keys, independent of the physical keyboard.
enum TerminalKey: Equatable {
    case go, cancel, menu, help, pageForward, pageBack, nextPanel, previousPanel
    case focusPanel(Int)
    case sector(String)
    case suggestionUp, suggestionDown, acceptSuggestion
    case keyboardOverlay
}

/// Translates key events into terminal keys before views see them, so the
/// mapping is configurable and testable (ARCHITECTURE §9.3).
@MainActor
enum KeyRouter {
    nonisolated(unsafe) private static var monitor: Any?
    private static var lastHelp: CFTimeInterval = 0

    /// F2…F11 key codes in sector order.
    static let sectorKeyCodes: [UInt16: String] = [
        120: "GOVT", 99: "CORP", 118: "MTGE", 96: "M-MKT", 97: "MUNI",
        98: "PFD", 100: "EQUITY", 101: "CMDTY", 109: "INDEX", 103: "CRNCY",
    ]
    static let sectorOrder = ["GOVT", "CORP", "MTGE", "M-MKT", "MUNI", "PFD", "EQUITY", "CMDTY", "INDEX", "CRNCY"]

    static func install() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            nonisolated(unsafe) let ev = event
            let consumed = MainActor.assumeIsolated { handle(ev) }
            return consumed ? nil : event
        }
    }

    /// Whether the key window's first responder is a panel command line.
    private static func commandLineFocused() -> Bool {
        guard let editor = NSApp.keyWindow?.firstResponder as? NSTextView else { return false }
        return editor.delegate is CommandField || (editor.delegate as? NSTextField) is CommandField
    }

    private static func textEditingElsewhere() -> Bool {
        guard NSApp.keyWindow?.firstResponder is NSTextView else { return false }
        return !commandLineFocused()
    }

    /// Maps an event to a terminal key, given whether suggestions are open.
    static func map(_ e: NSEvent, suggestionsOpen: Bool, commandEmpty: Bool) -> TerminalKey? {
        let mods = e.modifierFlags.intersection([.command, .control, .option, .shift])
        let code = e.keyCode
        if let s = sectorKeyCodes[code] { return .sector(s) }
        if mods == .option, let ch = e.charactersIgnoringModifiers, let d = Int(ch) {
            let idx = d == 0 ? 9 : d - 1
            if idx < sectorOrder.count { return .sector(sectorOrder[idx]) }
        }
        switch code {
        case 36, 76: return mods.isEmpty || mods == .shift ? .go : nil
        case 53: return .cancel
        case 122: return .help // F1
        case 121: return .pageForward // PgDn
        case 116: return .pageBack // PgUp
        case 119: return .menu // End
        case 125 where mods == .command: return .pageForward
        case 126 where mods == .command: return .pageBack
        case 125 where suggestionsOpen: return .suggestionDown
        case 126 where suggestionsOpen: return .suggestionUp
        case 48 where mods == .control: return .nextPanel
        case 48 where mods == [.control, .shift]: return .previousPanel
        case 48 where suggestionsOpen && mods.isEmpty: return .acceptSuggestion
        case 51 where commandEmpty && mods.isEmpty: return .menu // Delete on empty line
        default: break
        }
        if mods == .command, let ch = e.charactersIgnoringModifiers {
            switch ch {
            case "[": return .menu
            case "/": return .keyboardOverlay
            case "?": return .help
            case "1", "2", "3", "4": return .focusPanel(Int(ch)! - 1)
            default: break
            }
        }
        if mods == [.command, .shift], e.charactersIgnoringModifiers == "/" || e.charactersIgnoringModifiers == "?" { return .help }
        return nil
    }

    /// Returns true when the event was consumed.
    static func handle(_ e: NSEvent) -> Bool {
        let app = AppModel.shared
        guard NSApp.keyWindow?.identifier?.rawValue.hasPrefix("main") == true || NSApp.keyWindow?.title == "Meridian" else {
            return LaunchpadKeys.handle(e)
        }
        let ws: Workspace = app.workspace
        let panel = ws.focusedPanel
        // While editing an input cell, only panel/sector navigation applies.
        if textEditingElsewhere() {
            if case .some(let k) = map(e, suggestionsOpen: false, commandEmpty: false),
               [.nextPanel, .previousPanel, .keyboardOverlay].contains(k) || { if case .focusPanel = k { return true }; return false }() {
                return apply(k, panel: panel, host: ws)
            }
            if e.keyCode == 53 { ws.requestFocus(); return true }
            return false
        }
        guard let key = map(e, suggestionsOpen: !panel.suggestions.isEmpty, commandEmpty: panel.commandText.isEmpty) else {
            // Typing anywhere goes to the focused command line.
            if !commandLineFocused(), e.modifierFlags.intersection([.command, .control]).isEmpty, let ch = e.characters, !ch.isEmpty,
               ch.unicodeScalars.allSatisfy({ CharacterSet.alphanumerics.union(.punctuationCharacters).union(.whitespaces).contains($0) }) {
                panel.commandText += ch
                panel.updateSuggestions()
                ws.requestFocus()
                return true
            }
            return false
        }
        return apply(key, panel: panel, host: ws)
    }

    static func apply(_ key: TerminalKey, panel: PanelModel, host ws: FocusHost) -> Bool {
        switch key {
        case .go:
            if !panel.pendingInputs.isEmpty && panel.commandText.isEmpty {
                panel.applyPendingInputs()
            } else {
                panel.go()
            }
            ws.requestFocus()
        case .cancel: panel.cancel()
        case .menu: panel.menuBack()
        case .help:
            // Once: help for the screen in the focused panel. Twice: the
            // directory of all functions.
            let now = CACurrentMediaTime()
            let fn = panel.current?.function
            if now - lastHelp < 1.2 || fn == nil || fn == "HELP" || fn == "MENU" {
                panel.run(ActionFfi(function: "HELP", security: nil, args: []))
            } else if let fn {
                panel.run(ActionFfi(function: "HELP", security: nil, args: [KeyValue(key: "topic", value: fn)]))
            }
            lastHelp = now
        case .pageForward: panel.pageForward()
        case .pageBack: panel.pageBack()
        case .nextPanel: ws.focusNext()
        case .previousPanel: ws.focusPrevious()
        case let .focusPanel(i): ws.focus(i)
        case let .sector(s):
            let t = panel.commandText.trimmingCharacters(in: .whitespaces)
            panel.commandText = (t.isEmpty ? "" : t + " ") + "<\(s)> "
            panel.updateSuggestions()
            ws.requestFocus()
        case .suggestionUp: panel.moveHighlight(-1)
        case .suggestionDown: panel.moveHighlight(1)
        case .acceptSuggestion:
            if panel.highlighted == nil { panel.highlighted = 0 }
            _ = panel.acceptSuggestion()
            panel.updateSuggestions()
            ws.requestFocus()
        case .keyboardOverlay: AppModel.shared.showKeyboardOverlay.toggle()
        }
        return true
    }
}
