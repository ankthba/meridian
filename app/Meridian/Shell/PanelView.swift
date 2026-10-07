import MeridianCore
import SwiftUI

/// One pane: a header (label, security, the screen's sections as tabs,
/// sources, shortcut) over the screen. Commands come from the window's
/// command bar, which always targets the focused pane.
struct PanelView: View {
    @Bindable var panel: PanelModel
    let focused: Bool
    let focusToken: Int
    var onFocus: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            content
        }
        .background(Theme.bg.swiftUI)
        .contentShape(Rectangle())
        .simultaneousGesture(TapGesture().onEnded { onFocus() })
    }

    private var label: String {
        if panel.special == .ask { return "ASK" }
        guard let f = panel.screen?.function ?? panel.current?.function else { return "EMPTY" }
        return FunctionLabel.short(f).uppercased()
    }

    private var ticker: String? {
        guard let s = panel.screen?.security ?? panel.security else { return nil }
        return s.split(separator: " ").first.map(String.init)
    }

    private var header: some View {
        HStack(spacing: 14) {
            Text(label)
                .font(Theme.label())
                .tracking(1.4)
                .foregroundStyle((focused ? Theme.text : Theme.muted).swiftUI)
            if let ticker {
                Text(ticker)
                    .font(Theme.ui(12, weight: .semibold))
                    .foregroundStyle(Theme.text.swiftUI)
            }
            if let s = panel.screen, !s.menu.isEmpty {
                ScrollView(.horizontal) {
                    HStack(spacing: 14) {
                        ForEach(s.menu, id: \.number) { m in
                            Text(m.label)
                                .font(Theme.ui(12))
                                .foregroundStyle((m.selected ? Theme.text : Theme.muted).swiftUI)
                                .padding(.vertical, 3)
                                .overlay(alignment: .bottom) {
                                    if m.selected { Rectangle().fill(Theme.text.swiftUI).frame(height: 1.5) }
                                }
                                .contentShape(Rectangle())
                                .onTapGesture { panel.runRowAction(m.action) }
                        }
                    }
                }
                .scrollIndicators(.never)
            }
            Spacer(minLength: 6)
            if panel.loading {
                ProgressView().controlSize(.mini)
            }
            if let s = panel.screen {
                Text(sourceText(s))
                    .font(Theme.ui(11))
                    .foregroundStyle(Theme.muted.swiftUI)
                    .lineLimit(1)
                    .help(s.sources.compactMap(\.attribution).joined(separator: "\n"))
            }
            linkMenu
            Text("⌘\(panel.index + 1)")
                .font(Theme.swiftFont(11))
                .foregroundStyle(Theme.muted.swiftUI)
        }
        .padding(.horizontal, 12)
        .frame(height: 34)
        .background((focused ? Theme.selected : Theme.header).swiftUI)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.line.swiftUI).frame(height: 1) }
    }

    /// "iex · rt · alpaca", one source after another; MOCK when synthetic.
    private func sourceText(_ s: ScreenFfi) -> String {
        s.sources.map { b in
            b.synthetic ? "mock" : SourceText.describe(b)
        }
        .joined(separator: "   ")
    }

    private var linkMenu: some View {
        Menu {
            Button("Not linked") { panel.linkGroup = nil }
            ForEach(LinkBus.groups, id: \.self) { g in
                Button("Link group \(g)") { panel.linkGroup = g }
            }
        } label: {
            Text(panel.linkGroup.map { "link \($0)" } ?? "link")
                .font(Theme.ui(11))
                .foregroundStyle((panel.linkGroup == nil ? Theme.muted : Theme.text2).swiftUI)
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .fixedSize()
        .help("Panes in the same link group follow each other's security")
    }

    @ViewBuilder
    private var content: some View {
        if let m = panel.message {
            Text(m)
                .font(Theme.ui(13))
                .foregroundStyle(Theme.warn.swiftUI)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        if panel.special == .ask {
            AskView(panel: panel)
        } else if let s = panel.screen {
            ScreenView(screen: s, panel: panel, feed: panel.feed)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else {
            Spacer()
        }
    }
}
