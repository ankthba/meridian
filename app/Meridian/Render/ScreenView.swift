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
            Color.clear.frame(height: 6)
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
                Text(message)
                    .font(Theme.ui(13))
                    .foregroundStyle(Theme.down.swiftUI)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .accessibilityLabel("Error: \(message)")
                Spacer()
            }
        }
    }

    private func notAvailable(_ reason: String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("Not available")
                .font(Theme.ui(13, weight: .semibold))
                .foregroundStyle(Theme.warn.swiftUI)
            Text(reason)
                .font(Theme.ui(13))
                .foregroundStyle(Theme.text2.swiftUI)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Not available: \(reason)")
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
        Text(badge.synthetic ? "mock" : SourceText.describe(badge))
            .font(Theme.ui(11))
            .foregroundStyle((badge.synthetic ? Theme.warn : Theme.muted).swiftUI)
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
                ScrollView(.horizontal) {
                    TableBlockView(table: table, page: panel.pages[index] ?? 0, feed: feed) { row in
                        if let a = table.rows[row].action { panel.runRowAction(a) }
                    }
                }
                .scrollIndicators(.never)
            }
        case let .text(title, body):
            VStack(alignment: .leading, spacing: 2) {
                if let title { SectionTitle(text: title) }
                TextBlockBody(text: body)
                    .padding(.horizontal, 12)
            }
        case let .inputs(title, inputs):
            InputsView(title: title, inputs: inputs, panel: panel)
        case let .notice(level, text):
            Text(text)
                .font(Theme.ui(12.5))
                .foregroundStyle((level == .error ? Theme.down : level == .warning ? Theme.warn : Theme.muted).swiftUI)
                .padding(.horizontal, 12)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityLabel(level == .error ? "Error: \(text)" : text)
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
            .font(Theme.ui(12, weight: .semibold))
            .foregroundStyle(Theme.text.swiftUI)
            .padding(.horizontal, 12)
            .padding(.top, 8)
            .padding(.bottom, 2)
            .accessibilityAddTraits(.isHeader)
    }
}

struct FieldsView: View {
    let title: String?
    let columns: Int
    let fields: [FieldFfi]

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if let title { SectionTitle(text: title) }
            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 0) {
                ForEach(Array(stride(from: 0, to: fields.count, by: max(columns, 1))), id: \.self) { start in
                    GridRow {
                        ForEach(start..<min(start + max(columns, 1), fields.count), id: \.self) { i in
                            fieldCell(fields[i])
                        }
                    }
                }
            }
            .padding(.horizontal, 12)
        }
    }

    private func fieldCell(_ f: FieldFfi) -> some View {
        HStack(spacing: 8) {
            Text(f.label)
                .font(Theme.ui(12.5))
                .foregroundStyle(Theme.muted.swiftUI)
                .lineLimit(1)
            Spacer(minLength: 6)
            Text(valueText(f))
                .font(Theme.swiftFont(13, weight: f.style == .emphasis ? .semibold : .regular))
                .foregroundStyle(valueColor(f).swiftUI)
                .lineLimit(1)
                .textSelection(.enabled)
        }
        .padding(.vertical, 4)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.hairline.swiftUI).frame(height: 1) }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([SpokenText.header(f.label), SpokenText.value(valueText(f), format: f.format, formatted: f.text == nil)]
            .filter { !$0.isEmpty }.joined(separator: ", "))
        .accessibilityAddTraits(.isStaticText)
    }

    private func valueText(_ f: FieldFfi) -> String {
        if let t = f.text { return t }
        let s = TerminalFormatter.string(f.value, f.format)
        return s.isEmpty ? "--" : s
    }

    private func valueColor(_ f: FieldFfi) -> NSColor {
        if let s = TerminalFormatter.signedStyle(f.value, f.format) { return s.color }
        return f.style.color
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
            FlowLayout(spacing: 18) {
                ForEach(inputs, id: \.id) { input in
                    HStack(spacing: 6) {
                        Text(input.label)
                            .font(Theme.ui(12.5))
                            .foregroundStyle(Theme.muted.swiftUI)
                            .accessibilityHidden(true) // the control carries it
                        switch input.kind {
                        case .choice:
                            Menu {
                                ForEach(input.options, id: \.self) { o in
                                    Button(o) { panel.applyInput(id: input.id, value: o) }
                                }
                            } label: {
                                HStack(spacing: 3) {
                                    Text(panel.pendingInputs[input.id] ?? input.value)
                                        .font(Theme.ui(12.5))
                                        .foregroundStyle(Theme.text.swiftUI)
                                    Image(systemName: "chevron.down").font(.system(size: 8, weight: .semibold)).foregroundStyle(Theme.muted.swiftUI)
                                        .accessibilityHidden(true) // decoration; VoiceOver says "menu button"
                                }
                                .padding(.bottom, 2)
                                .overlay(alignment: .bottom) { Rectangle().fill(Theme.line.swiftUI).frame(height: 1) }
                            }
                            .menuStyle(.button)
                            .buttonStyle(.plain)
                            .fixedSize()
                            .accessibilityLabel(SpokenText.header(input.label))
                            .accessibilityValue(panel.pendingInputs[input.id] ?? input.value)
                        default:
                            InputCell(label: input.label, initial: input.value, width: inputWidth(input)) { v in
                                panel.pendingInputs[input.id] = v
                            } onCommit: { v in
                                panel.pendingInputs[input.id] = v
                                panel.applyPendingInputs()
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
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
    let label: String
    let initial: String
    let width: CGFloat
    let onChange: (String) -> Void
    let onCommit: (String) -> Void
    @State private var text = ""

    var body: some View {
        TextField("", text: $text)
            .textFieldStyle(.plain)
            .font(Theme.swiftFont(12.5))
            .foregroundStyle(Theme.text.swiftUI)
            .padding(.bottom, 2)
            .frame(width: width)
            .overlay(alignment: .bottom) { Rectangle().fill(Theme.line.swiftUI).frame(height: 1) }
            .onAppear { text = initial }
            .onChange(of: initial) { _, v in text = v }
            .onChange(of: text) { _, v in onChange(v) }
            .onSubmit { onCommit(text) }
            .accessibilityLabel(SpokenText.header(label))
            .accessibilityHint("Return applies it")
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

    /// Bar x value → category label from Rust (e.g. "Strong Buy", "Q1 26"),
    /// falling back to the index when no label is given.
    private func barKey(_ x: Double) -> String {
        let i = Int(x)
        if let c = chart.xCategories, c.indices.contains(i) { return c[i] }
        return "\(i)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                SectionTitle(text: chart.title)
                Spacer()
                ForEach(Array(chart.series.enumerated()), id: \.offset) { i, s in
                    HStack(spacing: 4) {
                        Rectangle().fill(color(i, s).swiftUI).frame(width: 10, height: 2)
                        Text(s.name).font(Theme.ui(11)).foregroundStyle(Theme.muted.swiftUI)
                    }
                }
            }
            .padding(.trailing, 12)
            Chart {
                ForEach(Array(chart.series.enumerated()), id: \.offset) { si, s in
                    ForEach(Array(zip(s.x, s.y).enumerated()), id: \.offset) { _, p in
                        if p.1.isFinite {
                            if s.bars {
                                BarMark(x: .value(xName, barKey(p.0)), y: .value(yName, p.1), width: barWidth)
                                    .foregroundStyle(color(si, s).swiftUI)
                                    .position(by: .value("Series", s.name))
                            } else if isTime {
                                LineMark(x: .value(xName, Date(timeIntervalSince1970: p.0 / 1e9)), y: .value(yName, p.1), series: .value("Series", s.name))
                                    .foregroundStyle(color(si, s).swiftUI)
                                    .lineStyle(StrokeStyle(lineWidth: 1.4))
                            } else {
                                LineMark(x: .value(xName, p.0), y: .value(yName, p.1), series: .value("Series", s.name))
                                    .foregroundStyle(color(si, s).swiftUI)
                                    .lineStyle(StrokeStyle(lineWidth: 1.4))
                            }
                        }
                    }
                }
                if let m = chart.xMarker {
                    RuleMark(x: .value("marker", m)).foregroundStyle(Theme.muted.swiftUI.opacity(0.8))
                        .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 3]))
                }
            }
            .chartXAxis { AxisMarks { _ in AxisGridLine().foregroundStyle(Theme.hairline.swiftUI); AxisValueLabel().font(Theme.ui(10.5)).foregroundStyle(Theme.muted.swiftUI) } }
            .chartYAxis { AxisMarks(position: .trailing) { _ in AxisGridLine().foregroundStyle(Theme.hairline.swiftUI); AxisValueLabel().font(Theme.swiftFont(10.5)).foregroundStyle(Theme.muted.swiftUI) } }
            .chartLegend(.hidden)
            .modifier(XDomain(chart: chart, isTime: isTime))
            .modifier(YDomain(chart: chart))
            .frame(height: CGFloat(max(chart.heightRows, 6)) * Theme.rowHeight)
            .padding(.horizontal, 12)
            .accessibilityElement(children: .contain)
            .accessibilityLabel(XyChartView.summary(chart))
            if chart.series.contains(where: \.bars), chart.xLabel.contains("|") {
                Text(chart.xLabel).font(Theme.ui(10.5)).foregroundStyle(Theme.muted.swiftUI).padding(.horizontal, 12)
            }
        }
    }

    /// Axis names VoiceOver reads with each point ("Date", "IV %").
    private var xName: String { chart.xLabel.isEmpty || chart.xLabel.contains("|") ? "Category" : chart.xLabel }
    private var yName: String { chart.yLabel.isEmpty ? "Value" : chart.yLabel }

    /// The chart in words: its kind, then each series from first to last
    /// point with its range, or each bar with its value.
    static func summary(_ chart: XyChartFfi) -> String {
        let f = { (v: Double) in TerminalFormatter.fixed(v, 2) }
        let isTime = chart.xLabel == "Date"
        let series: [String] = chart.series.compactMap { s in
            let points = zip(s.x, s.y).filter { $0.1.isFinite }
            guard let first = points.first, let last = points.last else { return nil }
            if s.bars {
                let bars = points.prefix(12).map { p -> String in
                    let i = Int(p.0)
                    let key = chart.xCategories.flatMap { $0.indices.contains(i) ? $0[i] : nil } ?? f(p.0)
                    return "\(key) \(f(p.1))"
                }
                return "\(s.name): " + bars.joined(separator: ", ")
            }
            let ys = points.map(\.1)
            let x = { (v: Double) in isTime ? xDate.string(from: TerminalFormatter.date(fromNanos: v)) : f(v) }
            return "\(s.name) from \(f(first.1)) at \(x(first.0)) to \(f(last.1)) at \(x(last.0)), low \(f(ys.min() ?? first.1)), high \(f(ys.max() ?? first.1))"
        }
        // The section heading above already reads the title.
        let kind = chart.series.allSatisfy(\.bars) ? "Bar chart" : "Line chart"
        return ([kind] + (series.isEmpty ? ["no data"] : series)).joined(separator: ". ")
    }

    private static let xDate: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .medium
        return f
    }()

    /// Few categories would otherwise stretch each bar across the pane.
    private var barWidth: MarkDimension {
        let n = Set(chart.series.filter(\.bars).flatMap(\.x)).count
        return n <= 4 ? .fixed(56) : .ratio(0.7)
    }

    private func color(_ i: Int, _ s: XySeriesFfi) -> NSColor {
        switch s.style {
        case .muted: Theme.muted
        case .up, .down: s.style.color
        default: Theme.series[i % Theme.series.count]
        }
    }
}

/// Fits the x axis to the data (Swift Charts otherwise includes zero). Bar
/// charts with category labels keep every category, in Rust's order, even
/// when a category has no finite value.
private struct XDomain: ViewModifier {
    let chart: XyChartFfi
    let isTime: Bool

    func body(content: Content) -> some View {
        let xs = chart.series.filter { !$0.bars }.flatMap(\.x).filter(\.isFinite)
        if let cats = chart.xCategories, !cats.isEmpty, chart.series.allSatisfy(\.bars) {
            var seen = Set<String>()
            content.chartXScale(domain: cats.filter { seen.insert($0).inserted })
        } else if !isTime, let lo = xs.min(), let hi = xs.max(), hi > lo {
            content.chartXScale(domain: lo...hi)
        } else {
            content
        }
    }
}

/// Prose in SF Pro; lines aligned with runs of spaces (command tables in
/// HELP, examples) in SF Mono so their columns line up.
struct TextBlockBody: View {
    let text: String

    private var runs: [(mono: Bool, text: String)] {
        var out: [(mono: Bool, text: String)] = []
        for line in text.components(separatedBy: "\n") {
            let mono = line.contains("  ")
            if let last = out.last, last.mono == mono {
                out[out.count - 1].text += "\n" + line
            } else {
                out.append((mono, line))
            }
        }
        return out
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(Array(runs.enumerated()), id: \.offset) { _, r in
                Text(r.text)
                    .font(r.mono ? Theme.swiftFont(12.5) : Theme.ui(13))
                    .lineSpacing(3)
                    .foregroundStyle(Theme.text2.swiftUI)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// Line charts fit the y axis to the data (a rebased series near 100
/// shouldn't share its height with zero); bars keep their zero baseline.
private struct YDomain: ViewModifier {
    let chart: XyChartFfi

    func body(content: Content) -> some View {
        let ys = chart.series.flatMap(\.y).filter(\.isFinite)
        if !chart.series.contains(where: \.bars), let lo = ys.min(), let hi = ys.max(), hi > lo {
            let pad = (hi - lo) * 0.06
            content.chartYScale(domain: (lo - pad)...(hi + pad))
        } else {
            content
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
                    ctx.draw(Text(l).font(Theme.ui(10.5)).foregroundColor(Theme.muted.swiftUI), at: CGPoint(x: labelW + CGFloat(j) * cell + cell / 2, y: 8))
                }
                for i in 0..<rows {
                    ctx.draw(Text(map.rowLabels[i]).font(Theme.ui(11.5)).foregroundColor(Theme.text2.swiftUI), at: CGPoint(x: 4, y: 24 + CGFloat(i) * 18 + 9), anchor: .leading)
                    for j in 0..<cols {
                        let v = map.values[i * cols + j]
                        let r = CGRect(x: labelW + CGFloat(j) * cell, y: 16 + CGFloat(i) * 18, width: cell - 2, height: 16)
                        ctx.fill(Path(r), with: .color(color(v)))
                        if v.isFinite {
                            ctx.draw(Text(TerminalFormatter.string(v, map.format)).font(Theme.swiftFont(10.5)).foregroundColor(Theme.text.swiftUI), at: CGPoint(x: r.midX, y: r.midY))
                        }
                    }
                }
            }
            .frame(width: labelW + CGFloat(cols) * cell + 8, height: 20 + CGFloat(rows) * 18)
            .padding(.horizontal, 12)
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Heat map") // the heading above names it
            .accessibilityChildren {
                ForEach(0..<rows, id: \.self) { i in
                    Text(rowDescription(i))
                }
            }
        }
    }

    /// "AAPL: SPY 0.85, QQQ 0.91", values as drawn.
    private func rowDescription(_ i: Int) -> String {
        let cols = map.colLabels.count
        let cells = map.colLabels.enumerated().map { j, label -> String in
            let k = i * cols + j
            let v = map.values.indices.contains(k) ? map.values[k] : .nan
            let text = v.isFinite ? SpokenText.value(TerminalFormatter.string(v, map.format), format: map.format) : "no data"
            return "\(label) \(text)"
        }
        return "\(map.rowLabels[i]): " + cells.joined(separator: ", ")
    }

    private var range: (Double, Double) {
        let f = map.values.filter(\.isFinite)
        return (f.min() ?? 0, f.max() ?? 1)
    }

    private func color(_ v: Double) -> Color {
        guard v.isFinite else { return Theme.hairline.swiftUI }
        if map.diverging {
            let t = max(-1, min(1, v))
            return (t >= 0 ? Theme.up : Theme.down).swiftUI.opacity(0.10 + 0.55 * abs(t))
        }
        let (lo, hi) = range
        let t = hi > lo ? (v - lo) / (hi - lo) : 0.5
        return Theme.text.swiftUI.opacity(0.06 + 0.40 * t)
    }
}

struct DiffView: View {
    let title: String
    let lines: [DiffLineFfi]

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            SectionTitle(text: title)
            ForEach(Array(lines.enumerated()), id: \.offset) { _, l in
                let color = l.kind == .added ? Theme.diffAddText : l.kind == .removed ? Theme.diffDelText : Theme.muted
                HStack(alignment: .top, spacing: 6) {
                    Text(l.kind == .added ? "+" : l.kind == .removed ? "−" : " ")
                        .font(Theme.swiftFont(11.5))
                        .foregroundStyle(color.swiftUI)
                    Text(l.text)
                        .font(Theme.swiftFont(11.5))
                        .foregroundStyle(color.swiftUI)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 1.5)
                .background((l.kind == .added ? Theme.upTint : l.kind == .removed ? Theme.downTint : Theme.bg).swiftUI)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(l.kind == .added ? "Added: \(l.text)" : l.kind == .removed ? "Removed: \(l.text)" : l.text)
            }
        }
    }
}
