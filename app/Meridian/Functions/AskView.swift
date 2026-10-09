import MeridianCore
import Observation
import SwiftUI

/// Bridges streamed ASK callbacks (Rust threads) to the main actor.
nonisolated final class AskStream: AskObserverFfi, @unchecked Sendable {
    let onText: @Sendable (String) -> Void
    let onCall: @Sendable (String, String, String) -> Void
    let onResult: @Sendable (AskToolFfi) -> Void

    init(onText: @escaping @Sendable (String) -> Void, onCall: @escaping @Sendable (String, String, String) -> Void, onResult: @escaping @Sendable (AskToolFfi) -> Void) {
        self.onText = onText
        self.onCall = onCall
        self.onResult = onResult
    }

    func onText(delta: String) { onText(delta) }
    func onToolCall(id: String, name: String, inputJson: String) { onCall(id, name, inputJson) }
    func onToolResult(tool: AskToolFfi) { onResult(tool) }
}

@MainActor
@Observable
final class AskModel {
    struct Turn: Identifiable {
        let id = UUID()
        var question: String
        var answer = ""
        var calls: [(id: String, name: String, input: String)] = []
        var tools: [AskToolFfi] = []
        var final: AskTurnFfi?
        var error: String?
        var running = true
    }

    var turns: [Turn] = []
    var draft = ""
    let sessionId = UUID().uuidString
    var available: Bool { (try? AppModel.shared.core?.askAvailable()) ?? false }

    func send(security: String?) {
        let q = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty, let core = AppModel.shared.core, !(turns.last?.running ?? false) else { return }
        draft = ""
        turns.append(Turn(question: q))
        let idx = turns.count - 1
        let stream = AskStream(
            onText: { d in Task { @MainActor in self.turns[idx].answer += d } },
            onCall: { id, name, input in Task { @MainActor in self.turns[idx].calls.append((id, name, input)) } },
            onResult: { t in Task { @MainActor in self.turns[idx].tools.append(t) } }
        )
        Task {
            do {
                let turn = try await core.ask(sessionId: sessionId, question: q, security: security, observer: stream)
                turns[idx].final = turn
                turns[idx].answer = turn.answer
                turns[idx].tools = turn.tools
                turns[idx].error = turn.error
            } catch {
                turns[idx].error = error.userMessage
            }
            turns[idx].running = false
        }
    }

    func cancel() {
        try? AppModel.shared.core?.askCancel(sessionId: sessionId)
    }
}

/// ASK — answers with tool use over local data, shows every source, and
/// flags any number not found in a tool result.
struct AskView: View {
    @Bindable var panel: PanelModel
    @State private var model = AskModel()

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(AppModel.shared.mode == .mock ? "Answers use mock test data." : "Answers come from the terminal's own data, where each source's terms allow; every number is checked against it.")
                .font(Theme.ui(12)).foregroundStyle(Theme.muted.swiftUI)
                .padding(.horizontal, 12).padding(.top, 10)
            if !model.available {
                VStack(alignment: .leading, spacing: 3) {
                    Text("Not available").font(Theme.ui(13, weight: .semibold)).foregroundStyle(Theme.warn.swiftUI)
                    Text("Add an Anthropic API key in Settings → Data Sources (⌘,).").font(Theme.ui(13)).foregroundStyle(Theme.text2.swiftUI)
                }
                .padding(.horizontal, 12).padding(.vertical, 6)
            }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(model.turns) { t in
                            TurnView(turn: t).id(t.id)
                        }
                    }
                    .padding(12)
                }
                .onChange(of: model.turns.last?.answer) { _, _ in
                    if let id = model.turns.last?.id { proxy.scrollTo(id, anchor: .bottom) }
                }
            }
            HStack(spacing: 8) {
                Text("Ask").font(Theme.ui(12.5, weight: .semibold)).foregroundStyle(Theme.muted.swiftUI)
                    .accessibilityHidden(true) // the field is labeled "Question"
                TextField("Ask about \(panel.security ?? "markets"), e.g. how has it done against SPY this year?", text: $model.draft)
                    .textFieldStyle(.plain)
                    .font(Theme.ui(13.5))
                    .foregroundStyle(Theme.text.swiftUI)
                    .onSubmit { model.send(security: panel.security) }
                    .accessibilityLabel("Question")
                if model.turns.last?.running == true {
                    Text("Stop").font(Theme.ui(12.5)).foregroundStyle(Theme.down.swiftUI).onTapGesture { model.cancel() }
                        .accessibilityAddTraits(.isButton)
                        .accessibilityAction { model.cancel() }
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 38)
            .background(Theme.header.swiftUI)
            .overlay(alignment: .top) { Rectangle().fill(Theme.line.swiftUI).frame(height: 1) }
        }
        // "ask …" typed in the command bar arrives here and is sent once.
        .task(id: panel.pendingQuestion) {
            guard let q = panel.pendingQuestion, !q.isEmpty else { return }
            panel.pendingQuestion = nil
            model.draft = q
            if model.available { model.send(security: panel.security) }
        }
    }
}

private struct TurnView: View {
    let turn: AskModel.Turn

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(turn.question).font(Theme.ui(13.5, weight: .semibold)).foregroundStyle(Theme.text.swiftUI)
            if turn.running && turn.answer.isEmpty {
                Text(turn.calls.isEmpty ? "Thinking…" : "Running \(turn.calls.last!.name)…")
                    .font(Theme.ui(12.5)).foregroundStyle(Theme.muted.swiftUI)
            }
            Text(attributed)
                .font(Theme.ui(13))
                .lineSpacing(3)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            if let f = turn.final {
                let unverified = f.checks.filter { !$0.verified }.count
                HStack(spacing: 12) {
                    Text(unverified == 0 ? "All \(f.checks.count) numbers match the data retrieved" : "\(unverified) number\(unverified == 1 ? "" : "s") not found in the data, marked in red")
                        .foregroundStyle((unverified == 0 ? Theme.up : Theme.down).swiftUI)
                    Text("\(f.model)\(f.servedByFallback ? " (fallback)" : "") · in \(f.inputTokens) / out \(f.outputTokens) · cache \(f.cacheReadTokens)")
                        .foregroundStyle(Theme.muted.swiftUI)
                }
                .font(Theme.ui(11.5))
            }
            if let e = turn.error {
                Text(e).font(Theme.ui(12.5)).foregroundStyle(Theme.down.swiftUI)
            }
            if !turn.tools.isEmpty {
                Text("SOURCES").font(Theme.label()).tracking(1.4).foregroundStyle(Theme.muted.swiftUI).padding(.top, 4)
                ForEach(Array(turn.tools.enumerated()), id: \.offset) { i, t in
                    VStack(alignment: .leading, spacing: 0) {
                        Text("[\(i + 1)] \(t.tool)  \(t.inputJson)")
                            .foregroundStyle((t.isError ? Theme.down : Theme.text2).swiftUI)
                            .lineLimit(2)
                        HStack(spacing: 10) {
                            if !t.sources.isEmpty { Text(t.sources.joined(separator: ", ")) }
                            if let r = t.rows { Text("\(r) rows") }
                            Text("\(t.durationMs) ms")
                        }
                        .foregroundStyle(Theme.muted.swiftUI)
                        if let sql = t.sql {
                            Text(sql).foregroundStyle(Theme.text.swiftUI).textSelection(.enabled)
                        }
                    }
                    .font(Theme.swiftFont(11))
                }
            }
        }
    }

    /// Answer text with unverified numbers marked red-on-dark.
    private var attributed: AttributedString {
        var a = AttributedString(turn.answer)
        a.foregroundColor = Theme.text2.swiftUI
        guard let f = turn.final else { return a }
        let ns = turn.answer as NSString
        for c in f.checks where !c.verified {
            let r = NSRange(location: Int(c.start), length: Int(c.end) - Int(c.start))
            guard r.location + r.length <= ns.length, let range = Range(r, in: turn.answer), let ar = Range(range, in: a) else { continue }
            a[ar].foregroundColor = Theme.down.swiftUI
            a[ar].backgroundColor = Theme.flashDown.swiftUI
            a[ar].underlineStyle = .single
        }
        return a
    }
}
