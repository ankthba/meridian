import Charts
import MeridianCore
import SwiftUI

/// Renders any function screen from its Rust screen model.
struct ScreenView: View {
    let screen: ScreenFfi
    @Bindable var panel: PanelModel
    let feed: QuoteFeed?

    private var hasFillChart: Bool {
        screen.blocks.contains { if case let .chart(spec) = $0 { return spec.heightRows == 0 } else { return false } }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            titleRow
            switch screen.status {
            case .ok:
                if hasFillChart {
                    VStack(alignment: .leading, spacing: 4) { blocks }
                        .frame(maxHeight: .infinity, alignment: .top)
                } else {
                    ScrollView(.vertical) {
                        VStack(alignment: .leading, spacing: 6) { blocks }
                            .padding(.bottom, 8)
                    }
                    .scrollIndicators(.never)
                }
            case let .notAvailable(reason):
                notAvailable(reason)
                ScrollView { VStack(alignment: .leading, spacing: 6) { blocks } }
            case let .error(message):
                Text("ERROR — \(message)")
                    .font(Theme.swiftFont())
                    .foregroundStyle(Theme.down.swiftUI)
                    .padding(6)
                Spacer()
            }
        }
    }

    private var titleRow: some View {
        HStack(spacing: 8) {
            Text(screen.title)
                .font(Theme.swiftFont(weight: .bold))
                .foregroundStyle(Theme.white.swiftUI)
                .lineLimit(1)
            Spacer(minLength: 4)
            ForEach(Array(screen.sources.enumerated()), id: \.offset) { _, b in
                SourceBadgeView(badge: b)
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
    }

    private func notAvailable(_ reason: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("NOT AVAILABLE")
                .font(Theme.swiftFont(weight: .bold))
                .foregroundStyle(Theme.down.swiftUI)
            Text(reason)
                .font(Theme.swiftFont())
                .foregroundStyle(Theme.amber.swiftUI)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(6)
    }

    @ViewBuilder
    private var blocks: some View {
        ForEach(Array(screen.blocks.enumerated()), id: \.offset) { i, b in
            BlockView(block: b, index: i, panel: panel, feed: feed)
        }
    }
}

struct SourceBadgeView: View {
    let badge: SourceBadgeFfi
    var body: some View {
        HStack(spacing: 4) {
            if badge.synthetic {
                Text("MOCK")
                    .font(Theme.swiftFont(11, weight: .bold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 4)
                    .background(Theme.mockBadge.swiftUI)
            } else {
                Text(badge.delay)
                    .font(Theme.swiftFont(11, weight: .bold))
                    .foregroundStyle(badge.delay == "RT" ? Theme.up.swiftUI : Theme.warning.swiftUI)
                Text(badge.source)
                    .font(Theme.swiftFont(11))
                    .foregroundStyle(Theme.muted.swiftUI)
            }
            Text(badge.provider)
                .font(Theme.swiftFont(11))
                .foregroundStyle(Theme.muted.swiftUI)
        }
        .help(badge.attribution ?? "\(badge.provider) · \(badge.delay) · \(badge.source)")
    }
}

struct BlockView: View {
    let block: BlockFfi
    let index: Int
    @Bindable var panel: PanelModel
    let feed: QuoteFeed?

    var body: some View {
        switch block {
        case let .fields(title, columns, fields):
            FieldsView(title: title, columns: Int(columns), fields: fields)
        case let .table(table):
            VStack(alignment: .leading, spacing: 2) {
                if let t = table.title {
                    SectionTitle(text: t)
                }
                TableBlockView(table: table, page: panel.pages[index] ?? 0, feed: feed) { row in
                    if let a = table.rows[row].action { panel.runRowAction(a) }
                }
            }
        case let .text(title, body):
            VStack(alignment: .leading, spacing: 2) {
                if let title { SectionTitle(text: title) }
                Text(body)
                    .font(Theme.swiftFont())
                    .foregroundStyle(Theme.amber.swiftUI)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 6)
            }
        case let .inputs(title, inputs):
            InputsView(title: title, inputs: inputs, panel: panel)
        case let .notice(level, text):
            Text(text)
                .font(Theme.swiftFont())
                .foregroundStyle((level == .error ? Theme.down : level == .warning ? Theme.warning : Theme.muted).swiftUI)
                .padding(.horizontal, 6)
                .fixedSize(horizontal: false, vertical: true)
        case let .chart(spec):
            ChartBlockView(spec: spec, feed: feed)
                .frame(minHeight: spec.heightRows == 0 ? 200 : CGFloat(spec.heightRows) * Theme.rowHeight,
                       maxHeight: spec.heightRows == 0 ? .infinity : CGFloat(spec.heightRows) * Theme.rowHeight)
        case let .xy(chart):
            XyChartView(chart: chart)
        case let .heat(map):
            HeatMapView(map: map)
        case let .diff(title, lines):
            DiffView(title: title, lines: lines)
        }
    }
}

struct SectionTitle: View {
    let text: String
    var body: some View {
        Text(text)
            .font(Theme.swiftFont(weight: .bold))
            .foregroundStyle(Theme.white.swiftUI)
            .padding(.horizontal, 6)
            .padding(.top, 2)
    }
}

struct FieldsView: View {
    let title: String?
    let columns: Int
    let fields: [FieldFfi]

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if let title { SectionTitle(text: title) }
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 14, alignment: .leading), count: max(columns, 1)), alignment: .leading, spacing: 1) {
                ForEach(Array(fields.enumerated()), id: \.offset) { _, f in
                    HStack(spacing: 6) {
                        Text(f.label)
                            .font(Theme.swiftFont())
                            .foregroundStyle(Theme.amber.swiftUI)
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        Text(valueText(f))
                            .font(Theme.swiftFont(weight: f.style == .emphasis ? .medium : .regular))
                            .foregroundStyle(valueColor(f).swiftUI)
                            .lineLimit(1)
                            .textSelection(.enabled)
                    }
                    .overlay(alignment: .bottom) { Rectangle().fill(Color(white: 0.11)).frame(height: 1) }
                }
            }
            .padding(.horizontal, 6)
        }
    }

    private func valueText(_ f: FieldFfi) -> String {
        if let t = f.text { return t }
        let s = TerminalFormatter.string(f.value, f.format)
        return s.isEmpty ? "--" : s
    }

    private func valueColor(_ f: FieldFfi) -> NSColor {
        if let s = TerminalFormatter.signedStyle(f.value, f.format) { return s.color }
        return f.style == .normal ? Theme.white : f.style.color
    }
}

/// Highlighted, editable input cells. Return applies one; GO in the command
/// line applies all pending edits.
struct InputsView: View {
    let title: String?
    let inputs: [InputFfi]
    @Bindable var panel: PanelModel

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if let title { SectionTitle(text: title) }
            FlowLayout(spacing: 12) {
                ForEach(inputs, id: \.id) { input in
                    HStack(spacing: 4) {
                        Text(input.label)
                            .font(Theme.swiftFont())
                            .foregroundStyle(Theme.amber.swiftUI)
                        switch input.kind {
                        case .choice:
                            Menu {
                                ForEach(input.options, id: \.self) { o in
                                    Button(o) { panel.applyInput(id: input.id, value: o) }
                                }
                            } label: {
                                Text(panel.pendingInputs[input.id] ?? input.value)
                                    .font(Theme.swiftFont())
                                    .foregroundStyle(Theme.inputText.swiftUI)
                                    .padding(.horizontal, 4)
                                    .background(Theme.inputFill.swiftUI)
                            }
                            .menuStyle(.button)
                            .buttonStyle(.plain)
                            .fixedSize()
                        default:
                            InputCell(initial: input.value, width: inputWidth(input)) { v in
                                panel.pendingInputs[input.id] = v
                            } onCommit: { v in
                                panel.pendingInputs[input.id] = v
                                panel.applyPendingInputs()
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, 6)
        }
    }

    private func inputWidth(_ i: InputFfi) -> CGFloat {
        let chars: Int = switch i.kind {
        case .date: 10
        case .number: 9
        default: max(14, min(i.value.count + 2, 48))
        }
        return CGFloat(chars) * Theme.charWidth + 8
    }
}

struct InputCell: View {
    let initial: String
    let width: CGFloat
    let onChange: (String) -> Void
    let onCommit: (String) -> Void
    @State private var text = ""

    var body: some View {
        TextField("", text: $text)
            .textFieldStyle(.plain)
            .font(Theme.swiftFont())
            .foregroundStyle(Theme.inputText.swiftUI)
            .padding(.horizontal, 4)
            .frame(width: width)
            .background(Theme.inputFill.swiftUI)
            .onAppear { text = initial }
            .onChange(of: initial) { _, v in text = v }
            .onChange(of: text) { _, v in onChange(v) }
            .onSubmit { onCommit(text) }
    }
}

/// Wrapping horizontal layout for input cells.
struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxW = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, rowH: CGFloat = 0, width: CGFloat = 0
        for s in subviews {
            let sz = s.sizeThatFits(.unspecified)
            if x > 0, x + sz.width > maxW { x = 0; y += rowH + 2; rowH = 0 }
            x += sz.width + spacing
            rowH = max(rowH, sz.height)
            width = max(width, x)
        }
        return CGSize(width: min(width, maxW), height: y + rowH)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowH: CGFloat = 0
        for s in subviews {
            let sz = s.sizeThatFits(.unspecified)
            if x > bounds.minX, x + sz.width > bounds.maxX { x = bounds.minX; y += rowH + 2; rowH = 0 }
            s.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(sz))
            x += sz.width + spacing
            rowH = max(rowH, sz.height)
        }
    }
}

struct XyChartView: View {
    let chart: XyChartFfi

    private var isTime: Bool { chart.xLabel == "Date" }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                SectionTitle(text: chart.title)
                Spacer()
                ForEach(Array(chart.series.enumerated()), id: \.offset) { i, s in
                    Text(s.name).font(Theme.swiftFont(11)).foregroundStyle(color(i, s).swiftUI)
                }
            }
            Chart {
                ForEach(Array(chart.series.enumerated()), id: \.offset) { si, s in
                    ForEach(Array(zip(s.x, s.y).enumerated()), id: \.offset) { _, p in
                        if p.1.isFinite {
                            if s.bars {
                                BarMark(x: .value("x", "\(Int(p.0))"), y: .value("y", p.1))
                                    .foregroundStyle(color(si, s).swiftUI)
                                    .position(by: .value("series", s.name))
                            } else if isTime {
                                LineMark(x: .value("x", Date(timeIntervalSince1970: p.0 / 1e9)), y: .value("y", p.1), series: .value("s", s.name))
                                    .foregroundStyle(color(si, s).swiftUI)
                                    .lineStyle(StrokeStyle(lineWidth: 1.4))
                            } else {
                                LineMark(x: .value("x", p.0), y: .value("y", p.1), series: .value("s", s.name))
                                    .foregroundStyle(color(si, s).swiftUI)
                                    .lineStyle(StrokeStyle(lineWidth: 1.4))
                            }
                        }
                    }
                }
                if let m = chart.xMarker {
                    RuleMark(x: .value("marker", m)).foregroundStyle(Theme.yellow.swiftUI.opacity(0.6))
                        .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 3]))
                }
            }
            .chartXAxis { AxisMarks { _ in AxisGridLine().foregroundStyle(Theme.grid.swiftUI); AxisValueLabel().font(Theme.swiftFont(10)).foregroundStyle(Theme.amber.swiftUI) } }
            .chartYAxis { AxisMarks(position: .trailing) { _ in AxisGridLine().foregroundStyle(Theme.grid.swiftUI); AxisValueLabel().font(Theme.swiftFont(10)).foregroundStyle(Theme.amber.swiftUI) } }
            .chartLegend(.hidden)
            .frame(height: CGFloat(max(chart.heightRows, 6)) * Theme.rowHeight)
            .padding(.horizontal, 6)
            if chart.series.contains(where: \.bars), chart.xLabel.contains("|") {
                Text(chart.xLabel).font(Theme.swiftFont(10)).foregroundStyle(Theme.muted.swiftUI).padding(.horizontal, 6)
            }
        }
    }

    private func color(_ i: Int, _ s: XySeriesFfi) -> NSColor {
        switch s.style {
        case .muted: Theme.muted
        case .emphasis: i == 0 ? Theme.amber : Theme.white
        default: s.style.color
        }
    }
}

struct HeatMapView: View {
    let map: HeatMapFfi

    var body: some View {
        let rows = map.rowLabels.count, cols = map.colLabels.count
        let cell: CGFloat = 54
        let labelW: CGFloat = 120
        VStack(alignment: .leading, spacing: 2) {
            SectionTitle(text: map.title)
            Canvas { ctx, _ in
                for (j, l) in map.colLabels.enumerated() {
                    ctx.draw(Text(l).font(Theme.swiftFont(10)).foregroundColor(Theme.white.swiftUI), at: CGPoint(x: labelW + CGFloat(j) * cell + cell / 2, y: 8))
                }
                for i in 0..<rows {
                    ctx.draw(Text(map.rowLabels[i]).font(Theme.swiftFont(11)).foregroundColor(Theme.white.swiftUI), at: CGPoint(x: 4, y: 24 + CGFloat(i) * 18 + 9), anchor: .leading)
                    for j in 0..<cols {
                        let v = map.values[i * cols + j]
                        let r = CGRect(x: labelW + CGFloat(j) * cell, y: 16 + CGFloat(i) * 18, width: cell - 2, height: 16)
                        ctx.fill(Path(r), with: .color(color(v)))
                        if v.isFinite {
                            ctx.draw(Text(TerminalFormatter.string(v, map.format)).font(Theme.swiftFont(10)).foregroundColor(.white), at: CGPoint(x: r.midX, y: r.midY))
                        }
                    }
                }
            }
            .frame(width: labelW + CGFloat(cols) * cell + 8, height: 20 + CGFloat(rows) * 18)
            .padding(.horizontal, 6)
        }
    }

    private var range: (Double, Double) {
        let f = map.values.filter(\.isFinite)
        return (f.min() ?? 0, f.max() ?? 1)
    }

    private func color(_ v: Double) -> Color {
        guard v.isFinite else { return Color(white: 0.08) }
        if map.diverging {
            let t = max(-1, min(1, v))
            return t >= 0 ? Color(red: 0.05, green: 0.15 + 0.5 * t, blue: 0.1) : Color(red: 0.2 + 0.5 * -t, green: 0.05, blue: 0.05)
        }
        let (lo, hi) = range
        let t = hi > lo ? (v - lo) / (hi - lo) : 0.5
        return Color(red: 0.2 + 0.7 * t, green: 0.12 + 0.35 * t, blue: 0.05)
    }
}

struct DiffView: View {
    let title: String
    let lines: [DiffLineFfi]

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            SectionTitle(text: title)
            ForEach(Array(lines.enumerated()), id: \.offset) { _, l in
                HStack(alignment: .top, spacing: 4) {
                    Text(l.kind == .added ? "+" : l.kind == .removed ? "−" : " ")
                        .font(Theme.swiftFont(weight: .bold))
                        .foregroundStyle((l.kind == .added ? Theme.up : l.kind == .removed ? Theme.down : Theme.muted).swiftUI)
                    Text(l.text)
                        .font(Theme.swiftFont())
                        .foregroundStyle((l.kind == .same ? Theme.muted : Theme.white).swiftUI)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
                .padding(.horizontal, 6)
                .background((l.kind == .added ? Theme.flashUp : l.kind == .removed ? Theme.flashDown : Theme.background).swiftUI)
            }
        }
    }
}
