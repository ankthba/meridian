import MeridianCore
import SwiftUI

/// One panel: header strip, command line, red function bar, screen.
struct PanelView: View {
    @Bindable var panel: PanelModel
    let focused: Bool
    let focusToken: Int
    var onFocus: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            ZStack(alignment: .topLeading) {
                VStack(spacing: 0) {
                    CommandLineView(panel: panel, focused: focused, focusToken: focusToken)
                        .frame(height: 20)
                    functionBar
                    content
                }
                if !panel.suggestions.isEmpty && focused {
                    SuggestionList(panel: panel)
                        .padding(.top, 20)
                        .zIndex(10)
                }
            }
        }
        .background(Color.black)
        .overlay(Rectangle().stroke(focused ? Theme.focus.swiftUI : Color.clear, lineWidth: 1))
        .contentShape(Rectangle())
        .simultaneousGesture(TapGesture().onEnded { onFocus() })
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text("\(panel.index + 1)")
                .font(Theme.swiftFont(11, weight: .bold))
                .foregroundStyle(focused ? Theme.focus.swiftUI : Theme.muted.swiftUI)
            Text(panel.security ?? "—")
                .font(Theme.swiftFont(11))
                .foregroundStyle(Theme.muted.swiftUI)
                .lineLimit(1)
            if panel.loading {
                Text("LOADING…").font(Theme.swiftFont(11)).foregroundStyle(Theme.warning.swiftUI)
            }
            Spacer()
            Menu {
                Button("No link") { panel.linkGroup = nil }
                ForEach(LinkBus.groups, id: \.self) { g in
                    Button("Group \(g)") { panel.linkGroup = g }
                }
            } label: {
                Text(panel.linkGroup ?? "·")
                    .font(Theme.swiftFont(11, weight: .bold))
                    .foregroundStyle(panel.linkGroup == nil ? Theme.muted.swiftUI : Color.black)
                    .padding(.horizontal, 4)
                    .background(panel.linkGroup == nil ? Color.clear : Theme.amber.swiftUI)
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .fixedSize()
            .help("Link group: panels in the same group follow each other's security")
        }
        .padding(.horizontal, 6)
        .frame(height: 16)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.grid.swiftUI).frame(height: 1) }
    }

    private var functionBar: some View {
        HStack(spacing: 14) {
            if let s = panel.screen, !s.menu.isEmpty {
                ScrollView(.horizontal) {
                    HStack(spacing: 14) {
                        ForEach(s.menu, id: \.number) { m in
                            HStack(spacing: 3) {
                                Text("\(m.number))").font(Theme.swiftFont(weight: .bold))
                                Text(m.label).font(Theme.swiftFont())
                            }
                            .foregroundStyle(m.selected ? Theme.functionBar.swiftUI : Color.white)
                            .padding(.horizontal, m.selected ? 3 : 0)
                            .background(m.selected ? Color.white : Color.clear)
                            .onTapGesture { panel.runRowAction(m.action) }
                        }
                    }
                }
                .scrollIndicators(.never)
            } else if let s = panel.screen {
                Text(s.function).font(Theme.swiftFont(weight: .bold)).foregroundStyle(.white)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 6)
        .frame(height: 18)
        .background(Theme.functionBar.swiftUI)
    }

    @ViewBuilder
    private var content: some View {
        if let m = panel.message {
            Text(m)
                .font(Theme.swiftFont())
                .foregroundStyle(Theme.down.swiftUI)
                .padding(6)
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
