import AppKit
import MeridianCore
import SwiftUI

/// The panel's command line. Keys with terminal meaning (GO, CANCEL, MENU,
/// sector keys, paging) are intercepted by `KeyRouter` before they reach
/// this field; the field itself only edits text.
final class CommandField: NSTextField {
    weak var panel: PanelModel?

    override var acceptsFirstResponder: Bool { true }
}

struct CommandLineView: NSViewRepresentable {
    @Bindable var panel: PanelModel
    let focused: Bool
    let focusToken: Int

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var panel: PanelModel
        var lastToken = -1
        init(_ p: PanelModel) { panel = p }

        func controlTextDidChange(_ obj: Notification) {
            guard let f = obj.object as? NSTextField else { return }
            MainActor.assumeIsolated {
                panel.commandText = f.stringValue.uppercased() == f.stringValue ? f.stringValue : f.stringValue.uppercased()
                if f.stringValue != panel.commandText { f.stringValue = panel.commandText }
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
        f.drawsBackground = true
        f.backgroundColor = Theme.commandBackground
        f.textColor = Theme.yellow
        f.font = Theme.font()
        f.focusRingType = .none
        f.delegate = context.coordinator
        f.cell?.usesSingleLineMode = true
        f.cell?.isScrollable = true
        f.placeholderAttributedString = NSAttributedString(
            string: "Type a security and function, e.g. AAPL US <EQUITY> DES, then Return (GO)",
            attributes: [.foregroundColor: Theme.muted, .font: Theme.font(12)]
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
        if focused, context.coordinator.lastToken != focusToken {
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

/// Autocomplete list under the command line.
struct SuggestionList: View {
    @Bindable var panel: PanelModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(panel.suggestions.enumerated()), id: \.offset) { i, s in
                HStack(spacing: 10) {
                    Text(s.kind == .function ? "FN" : "SEC")
                        .font(Theme.swiftFont(10))
                        .foregroundStyle(Theme.muted.swiftUI)
                        .frame(width: 26, alignment: .leading)
                    Text(s.display)
                        .font(Theme.swiftFont(weight: .medium))
                        .foregroundStyle(Theme.white.swiftUI)
                        .frame(minWidth: 150, alignment: .leading)
                    Text(s.detail)
                        .font(Theme.swiftFont())
                        .foregroundStyle(Theme.amber.swiftUI)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 6)
                .frame(height: Theme.rowHeight)
                .background(panel.highlighted == i ? Theme.selection.swiftUI : Color.black)
                .contentShape(Rectangle())
                .onTapGesture {
                    panel.highlighted = i
                    _ = panel.acceptSuggestion()
                    panel.updateSuggestions()
                    AppModel.shared.workspace.requestFocus()
                }
            }
        }
        .background(Color.black)
        .overlay(Rectangle().stroke(Theme.grid.swiftUI, lineWidth: 1))
    }
}
