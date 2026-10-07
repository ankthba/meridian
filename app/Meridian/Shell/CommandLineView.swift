import AppKit
import MeridianCore
import SwiftUI

/// The command field. Keys with special meaning (GO, CANCEL, MENU, paging,
/// suggestion navigation) are intercepted by `KeyRouter` before they reach
/// it; the field itself only edits text.
final class CommandField: NSTextField {
    weak var panel: PanelModel?

    override var acceptsFirstResponder: Bool { true }
}

/// The text field inside the command bar, bound to whichever panel is
/// focused: commands always run in the focused pane.
struct CommandFieldView: NSViewRepresentable {
    @Bindable var panel: PanelModel
    let focusToken: Int

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var panel: PanelModel
        var lastToken = -1
        init(_ p: PanelModel) { panel = p }

        func controlTextDidChange(_ obj: Notification) {
            guard let f = obj.object as? NSTextField else { return }
            MainActor.assumeIsolated {
                panel.commandText = f.stringValue
                panel.updateSuggestions()
            }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(panel) }

    func makeNSView(context: Context) -> CommandField {
        let f = CommandField()
        f.panel = panel
        f.isBordered = false
        f.isBezeled = false
        f.drawsBackground = false
        f.textColor = Theme.text
        f.font = Theme.uiFont(15)
        f.focusRingType = .none
        f.delegate = context.coordinator
        f.cell?.usesSingleLineMode = true
        f.cell?.isScrollable = true
        f.placeholderAttributedString = NSAttributedString(
            string: "Type a ticker or a command: aapl · aapl 5y · aapl filings · earnings this week",
            attributes: [.foregroundColor: Theme.muted, .font: Theme.uiFont(15)]
        )
        return f
    }

    func updateNSView(_ f: CommandField, context: Context) {
        context.coordinator.panel = panel
        f.panel = panel
        if f.stringValue != panel.commandText {
            f.stringValue = panel.commandText
            f.currentEditor()?.selectedRange = NSRange(location: panel.commandText.utf16.count, length: 0)
        }
        if context.coordinator.lastToken != focusToken {
            context.coordinator.lastToken = focusToken
            DispatchQueue.main.async {
                guard let w = f.window else { return }
                if w.firstResponder !== f.currentEditor() {
                    w.makeFirstResponder(f)
                    f.currentEditor()?.selectedRange = NSRange(location: f.stringValue.utf16.count, length: 0)
                }
            }
        }
    }
}

/// The command bar: prompt, field, and where the command will run.
struct CommandBar: View {
    @Bindable var panel: PanelModel
    let focusToken: Int

    var body: some View {
        HStack(spacing: 10) {
            Text("›").font(Theme.ui(17)).foregroundStyle(Theme.muted.swiftUI)
            CommandFieldView(panel: panel, focusToken: focusToken)
                .frame(height: 22)
            Text("runs in pane \(panel.index + 1)")
                .font(Theme.ui(11))
                .foregroundStyle(Theme.muted.swiftUI)
        }
        .padding(.horizontal, 14)
        .frame(height: 44)
        .background(Theme.header.swiftUI)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.line.swiftUI).frame(height: 1) }
    }
}

/// Grouped completions under the command bar (`docs/DESIGN.md` → Command bar).
struct CompletionPopover: View {
    @Bindable var panel: PanelModel

    private var groups: [(String, [(Int, SuggestionFfi)])] {
        var order: [String] = []
        var byGroup: [String: [(Int, SuggestionFfi)]] = [:]
        for (i, s) in panel.suggestions.enumerated() {
            let g = s.groupTitle
            if byGroup[g] == nil { order.append(g) }
            byGroup[g, default: []].append((i, s))
        }
        return order.map { ($0, byGroup[$0] ?? []) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(groups, id: \.0) { group, items in
                Text(group.uppercased())
                    .font(Theme.ui(10.5, weight: .semibold))
                    .tracking(0.8)
                    .foregroundStyle(Theme.muted.swiftUI)
                    .padding(.horizontal, 14)
                    .padding(.top, 9)
                    .padding(.bottom, 3)
                ForEach(items, id: \.0) { i, s in
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        Text(s.titleText)
                            .font(Theme.ui(13, weight: .semibold))
                            .foregroundStyle(Theme.text.swiftUI)
                            .frame(minWidth: 64, alignment: .leading)
                        Text(s.detail)
                            .font(Theme.ui(13))
                            .foregroundStyle(Theme.text2.swiftUI)
                            .lineLimit(1)
                        Spacer(minLength: 8)
                        Text(s.hintText)
                            .font(Theme.swiftFont(11.5))
                            .foregroundStyle(Theme.muted.swiftUI)
                    }
                    .padding(.horizontal, 14)
                    .frame(height: 30)
                    .background((panel.highlighted == i ? Theme.hover : Theme.raised).swiftUI)
                    .contentShape(Rectangle())
                    .onTapGesture {
                        panel.highlighted = i
                        _ = panel.acceptSuggestion()
                        panel.updateSuggestions()
                        AppModel.shared.workspace.requestFocus()
                    }
                }
            }
        }
        .padding(.bottom, 6)
        .frame(width: 560, alignment: .leading)
        .background(Theme.raised.swiftUI)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color(white: 0.2), lineWidth: 1))
        .shadow(color: .black.opacity(0.55), radius: 25, y: 18)
    }
}

extension SuggestionFfi {
    /// Group heading in the completion popover.
    var groupTitle: String { kind == .function ? "Functions" : "Securities" }

    /// Securities show their ticker; functions their plain name.
    var titleText: String {
        switch kind {
        case .security: display.split(separator: " ").first.map(String.init) ?? display
        case .function: FunctionLabel.short(display)
        }
    }

    /// Right-hand hint: the mnemonic for functions, the market for securities.
    var hintText: String {
        switch kind {
        case .function: display
        case .security: display.split(separator: " ").dropFirst().joined(separator: " ")
        }
    }
}

/// Plain-word names for function mnemonics, used in pane labels and menus.
enum FunctionLabel {
    static let names: [String: String] = [
        "DES": "Overview", "GP": "Chart", "GIP": "Intraday", "HP": "Prices", "W": "Watchlist", "MOST": "Most active",
        "N": "News", "CN": "News", "TOP": "News", "FA": "Financials", "EE": "Estimates", "ERN": "Earnings",
        "ANR": "Analysts", "HDS": "Holders", "DVD": "Dividends", "CF": "Filing", "OMON": "Options",
        "OVDV": "Volatility", "OVME": "Valuation", "EQS": "Screener", "RV": "Peers", "CORR": "Correlation",
        "PORT": "Portfolio", "BTST": "Backtest", "ALRT": "Alerts", "WEI": "World indices", "ECO": "Economy",
        "FXC": "Currencies", "CRYP": "Crypto", "ASK": "Ask", "HELP": "Help", "MENU": "Menu", "SECF": "Search",
        "TODAY": "Today", "CALENDAR": "Calendar", "FILINGS": "Filings", "COMPARE": "Compare", "BLP": "Launchpad",
    ]

    static func short(_ mnemonic: String) -> String { names[mnemonic] ?? mnemonic.capitalized }
}
