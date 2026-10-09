import AppKit
import CoreText
import MeridianCore
import QuartzCore
import SwiftUI

/// Draws one grid row into a layer. A separate delegate object, because an
/// NSView must not be the delegate of layers other than its backing layer.
private final class RowLayerDelegate: NSObject, CALayerDelegate {
    weak var grid: TerminalGridView?

    func draw(_ layer: CALayer, in ctx: CGContext) {
        guard let grid, let vi = layer.value(forKey: "vi") as? Int else { return }
        let flipped = layer.contentsAreFlipped()
        MainActor.assumeIsolated { grid.drawRowLayer(vi, ctx: ctx, height: layer.bounds.height, contextFlipped: flipped) }
    }

    func action(for layer: CALayer, forKey event: String) -> CAAction? { NSNull() }
}

/// High-frequency table grid (ARCHITECTURE §7.3).
///
/// Each visible row is its own CALayer, so a tick on one instrument repaints
/// only that row; AppKit's dirty-rect merging would otherwise repaint every
/// row between scattered updates. Live columns overlay hot-row values; cells
/// flash briefly on change. Text uses cached CTLines and monospace metrics
/// (width = characters × cell width), so nothing is measured per frame.
/// `draw(_:)` paints the full grid for offscreen snapshot rendering.
///
/// For VoiceOver the view is a table whose rows and cells are
/// `NSAccessibilityElement`s (`GridAccessibility.swift`), made the first
/// time an assistive app asks and read from the same cell text the grid
/// draws. Drawing and live updates do no accessibility work.
final class TerminalGridView: NSView {
    var table: TableFfi = TableFfi(title: nil, columns: [], rows: [], pageSize: nil, numbered: false) {
        didSet { rowsByIdValid = false; recompute(); resetRowLayers(); axShapeChanged() }
    }
    var page: Int = 0 { didSet { if page != oldValue { rowsByIdValid = false; resetRowLayers(); axShapeChanged() } } }
    var feed: QuoteFeed? {
        didSet {
            oldValue?.removeListener(self)
            rowsByIdValid = false
            feed?.addListener(self) { [weak self] ids in self?.liveChanged(Set(ids)) }
            redrawAllRows()
        }
    }
    var onSelect: ((Int) -> Void)?
    var selectedRow: Int? {
        didSet {
            for ri in [oldValue, selectedRow].compactMap({ $0 }) {
                rowLayers[ri - visibleRows.lowerBound]?.setNeedsDisplay()
            }
            if ax != nil, selectedRow != oldValue { NSAccessibility.post(element: self, notification: .selectedRowsChanged) }
        }
    }

    /// Rows drawn since launch (perf diagnostics).
    nonisolated(unsafe) static var rowsDrawn = 0
    nonisolated(unsafe) static var drawCalls = 0

    private var colX: [CGFloat] = []
    private var colW: [CGFloat] = []
    private let numberWidth: CGFloat = 4
    private let pad: CGFloat = 12
    private let font = Theme.font()
    private let boldFont = Theme.font(weight: .medium)
    private let flashDuration: CFTimeInterval = 0.35
    private lazy var cellWidth: CGFloat = Theme.charWidth
    private lazy var ascent: CGFloat = font.ascender

    private let layerDelegate = RowLayerDelegate()
    private var rowLayers: [Int: CALayer] = [:]
    private let headerLayer = CALayer()
    private let footerLayer = CALayer()
    private var clipObserver: NSObjectProtocol?

    override init(frame: NSRect) {
        super.init(frame: frame)
        commonInit()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        commonInit()
    }

    private func commonInit() {
        wantsLayer = true
        layerDelegate.grid = self
        for l in [headerLayer, footerLayer] {
            l.delegate = layerDelegate
            l.isOpaque = true
            l.contentsFormat = .RGBA8Uint
        }
        headerLayer.setValue(-1, forKey: "vi")
        footerLayer.setValue(-2, forKey: "vi")
    }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { false }
    override var wantsUpdateLayer: Bool { true }

    override func makeBackingLayer() -> CALayer {
        let l = CALayer()
        l.backgroundColor = Theme.background.cgColor
        return l
    }

    override func updateLayer() {
        layer?.backgroundColor = Theme.background.cgColor
        layoutRowLayers()
    }

    override func layout() {
        super.layout()
        layoutRowLayers()
    }

    override func setFrameSize(_ newSize: NSSize) {
        let widthChanged = newSize.width != frame.width
        super.setFrameSize(newSize)
        if widthChanged {
            headerLayer.setNeedsDisplay()
            footerLayer.setNeedsDisplay()
            redrawAllRows()
        }
        layoutRowLayers()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let o = clipObserver { NotificationCenter.default.removeObserver(o) }
        clipObserver = nil
        var ancestor = superview
        while let a = ancestor {
            if let clip = a as? NSClipView {
                clip.postsBoundsChangedNotifications = true
                clipObserver = NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: clip, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.layoutRowLayers() }
                }
                break
            }
            ancestor = a.superview
        }
        let scale = window?.backingScaleFactor ?? 2
        for l in [headerLayer, footerLayer] + Array(rowLayers.values) {
            l.contentsScale = scale
            l.setNeedsDisplay()
        }
        layoutRowLayers()
    }

    // MARK: Geometry

    var visibleRows: Range<Int> {
        guard let size = table.pageSize.map(Int.init), size > 0 else { return 0..<table.rows.count }
        let start = min(page * size, max(0, table.rows.count - 1))
        return start..<min(start + size, table.rows.count)
    }

    var contentHeight: CGFloat {
        CGFloat(visibleRows.count + 1) * Theme.rowHeight + (pageCount > 1 ? Theme.rowHeight : 0)
    }

    var pageCount: Int {
        guard let size = table.pageSize.map(Int.init), size > 0 else { return 1 }
        return max(1, (table.rows.count + size - 1) / size)
    }

    private func recompute() {
        var x: CGFloat = pad
        colX = []
        colW = []
        if table.numbered { x += numberWidth * cellWidth }
        for c in table.columns {
            colX.append(x)
            let w = CGFloat(max(c.width, 2)) * cellWidth
            colW.append(w)
            x += w + pad * 1.5
        }
        invalidateIntrinsicContentSize()
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: (colX.last ?? 0) + (colW.last ?? 0) + pad, height: contentHeight)
    }

    private func rowRect(_ visibleIndex: Int) -> NSRect {
        NSRect(x: 0, y: CGFloat(visibleIndex + 1) * Theme.rowHeight, width: bounds.width, height: Theme.rowHeight)
    }

    // MARK: Row layers

    private func resetRowLayers() {
        for l in rowLayers.values { l.removeFromSuperlayer() }
        rowLayers.removeAll()
        flashing.removeAll()
        headerLayer.setNeedsDisplay()
        footerLayer.setNeedsDisplay()
        layoutRowLayers()
        needsDisplay = true
    }

    private func redrawAllRows() {
        for l in rowLayers.values { l.setNeedsDisplay() }
    }

    /// Ensures a layer exists for each row in the visible rect, and only those.
    func layoutRowLayers() {
        guard let host = layer else { return }
        let h = Theme.rowHeight
        let scale = window?.backingScaleFactor ?? 2
        // Sublayer frames use the view's flipped (top-left) coordinates.
        host.isGeometryFlipped = true
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        if headerLayer.superlayer == nil { host.addSublayer(headerLayer) }
        headerLayer.contentsScale = scale
        let hf = NSRect(x: 0, y: 0, width: bounds.width, height: h)
        if headerLayer.frame != hf { headerLayer.frame = hf; headerLayer.setNeedsDisplay() }
        if pageCount > 1 {
            if footerLayer.superlayer == nil { host.addSublayer(footerLayer) }
            footerLayer.contentsScale = scale
            let ff = NSRect(x: 0, y: CGFloat(visibleRows.count + 1) * h, width: bounds.width, height: h)
            if footerLayer.frame != ff { footerLayer.frame = ff; footerLayer.setNeedsDisplay() }
        } else {
            footerLayer.removeFromSuperlayer()
        }
        let vis = visibleRect.isEmpty ? bounds : visibleRect
        let n = visibleRows.count
        // Before the view is in a window its rects can be empty or non-finite.
        guard n > 0, window != nil, vis.minY.isFinite, vis.maxY.isFinite, vis.height > 0 else { return }
        let first = max(0, min(n - 1, Int(max(0, floor(vis.minY / h))) - 3))
        let last = min(n - 1, Int(min(Double(n + 3), ceil(vis.maxY / h))) + 2)
        guard first <= last else { return }
        let wanted = first...last
        for (vi, l) in rowLayers where !wanted.contains(vi) {
            l.removeFromSuperlayer()
            rowLayers[vi] = nil
        }
        for vi in wanted {
            let frame = rowRect(vi)
            if let l = rowLayers[vi] {
                if l.frame != frame { l.frame = frame; l.setNeedsDisplay() }
            } else {
                let l = CALayer()
                l.delegate = layerDelegate
                l.contentsScale = scale
                l.isOpaque = true
                // 8-bit backing: half the fill/blend cost of the default
                // extended-range format on wide-gamut displays.
                l.contentsFormat = .RGBA8Uint
                l.setValue(vi, forKey: "vi")
                l.frame = frame
                host.addSublayer(l)
                l.setNeedsDisplay()
                rowLayers[vi] = l
            }
        }
    }

    // MARK: Live updates

    private var rowsById: [UInt32: [Int]] = [:]
    private var rowsByIdValid = false
    private var flashing: [Int: CFTimeInterval] = [:]
    private var flashTimer: Timer?

    private func rebuildRowIndex() {
        rowsById = [:]
        guard let feed else { return }
        for (vi, ri) in visibleRows.enumerated() {
            if let s = table.rows[ri].security, let id = feed.idBySecurity[s] {
                rowsById[id, default: []].append(vi)
            }
        }
        rowsByIdValid = true
    }

    /// Repaints only rows whose instrument changed in the last poll.
    func liveChanged(_ ids: Set<UInt32>) {
        guard feed != nil else { return }
        if !rowsByIdValid { rebuildRowIndex() }
        let now = CACurrentMediaTime()
        // The tick flash is the app's only animation; Reduce Motion turns
        // it off (values and colors still update).
        let flash = !MotionPreference.reduceMotion
        for id in ids {
            guard let rows = rowsById[id] else { continue }
            for vi in rows {
                guard let l = rowLayers[vi] else { continue }
                l.setNeedsDisplay()
                if flash { flashing[vi] = now + flashDuration }
            }
        }
        if flashTimer == nil, !flashing.isEmpty {
            let t = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.clearExpiredFlashes() }
            }
            RunLoop.main.add(t, forMode: .common)
            flashTimer = t
        }
    }

    private func clearExpiredFlashes() {
        let now = CACurrentMediaTime()
        for (vi, until) in flashing where until <= now {
            flashing[vi] = nil
            rowLayers[vi]?.setNeedsDisplay()
        }
        if flashing.isEmpty {
            flashTimer?.invalidate()
            flashTimer = nil
        }
    }

    // MARK: Drawing

    fileprivate func drawRowLayer(_ vi: Int, ctx: CGContext, height: CGFloat, contextFlipped: Bool) {
        // Drawing below assumes top-left origin. Core Animation already
        // flips the context for layers under a geometry-flipped ancestor
        // (`contentsAreFlipped`); flip by hand only when it hasn't.
        ctx.saveGState()
        if !contextFlipped {
            ctx.translateBy(x: 0, y: height)
            ctx.scaleBy(x: 1, y: -1)
        }
        ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        ctx.setFillColor(Theme.background.cgColor)
        ctx.fill(CGRect(x: 0, y: 0, width: bounds.width, height: height))
        switch vi {
        case -1: drawHeader(ctx, y: 0)
        case -2: drawFooter(ctx, y: 0)
        default:
            let ri = visibleRows.lowerBound + vi
            if ri < visibleRows.upperBound { drawRow(ctx, ri: ri, vi: vi, y: 0) }
        }
        ctx.restoreGState()
    }

    /// Full repaint (offscreen snapshots use `cacheDisplay`, which calls this).
    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        Self.drawCalls += 1
        ctx.setFillColor(Theme.background.cgColor)
        ctx.fill(dirtyRect)
        ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        drawHeader(ctx, y: 0)
        for (vi, ri) in visibleRows.enumerated() {
            let r = rowRect(vi)
            if r.intersects(dirtyRect) { drawRow(ctx, ri: ri, vi: vi, y: r.minY) }
        }
        if pageCount > 1 { drawFooter(ctx, y: CGFloat(visibleRows.count + 1) * Theme.rowHeight) }
    }

    private func drawHeader(_ ctx: CGContext, y: CGFloat) {
        let h = Theme.rowHeight
        for (i, c) in table.columns.enumerated() where i < colX.count {
            drawLabel(ctx, c.title, in: NSRect(x: colX[i], y: y, width: colW[i], height: h), align: c.align)
        }
        if table.numbered {
            drawLabel(ctx, "#", in: NSRect(x: pad, y: y, width: 4 * cellWidth, height: h), align: .left)
        }
        ctx.setFillColor(Theme.line.cgColor)
        ctx.fill(CGRect(x: 0, y: y + h - 1, width: bounds.width, height: 1))
    }

    private static let labelFont = Theme.uiFont(11)
    private static var labelLines: [String: CTLine] = [:]

    /// Column headers: SF Pro 11 in `muted`, measured (not cell-width) for
    /// alignment. Not on the hot path.
    private func drawLabel(_ ctx: CGContext, _ s: String, in rect: NSRect, align: AlignFfi) {
        guard !s.isEmpty else { return }
        let l = Self.labelLines[s] ?? {
            let l = CTLineCreateWithAttributedString(NSAttributedString(string: s, attributes: [.font: Self.labelFont, .foregroundColor: Theme.muted]))
            Self.labelLines[s] = l
            return l
        }()
        let w = min(CGFloat(CTLineGetTypographicBounds(l, nil, nil, nil)), rect.width)
        let x: CGFloat = switch align {
        case .left: rect.minX
        case .right: rect.maxX - w
        case .center: rect.midX - w / 2
        }
        let f = Self.labelFont
        ctx.textPosition = CGPoint(x: x, y: rect.minY + (rect.height - (f.ascender - f.descender)) / 2 + f.ascender)
        CTLineDraw(l, ctx)
    }

    private func drawFooter(_ ctx: CGContext, y: CGFloat) {
        let label = "Page \(page + 1) of \(pageCount)  —  PgDn/PgUp (⌘↓/⌘↑) to page"
        drawText(ctx, label, in: NSRect(x: pad, y: y, width: bounds.width, height: Theme.rowHeight), align: .left, color: Theme.muted)
    }

    private func drawRow(_ ctx: CGContext, ri: Int, vi: Int, y: CGFloat) {
        Self.rowsDrawn += 1
        let h = Theme.rowHeight
        let row = table.rows[ri]
        if selectedRow == ri {
            ctx.setFillColor(Theme.selected.cgColor)
            ctx.fill(NSRect(x: 0, y: y, width: bounds.width, height: h))
        }
        ctx.setFillColor(Theme.hairline.cgColor)
        ctx.fill(NSRect(x: 0, y: y + h - 1, width: bounds.width, height: 1))
        let live = row.security.flatMap { feed?.row(for: $0) }
        let isFlashing = (flashing[vi] ?? 0) > CACurrentMediaTime()
        if table.numbered {
            drawText(ctx, "\(ri + 1)", in: NSRect(x: pad, y: y, width: 4 * cellWidth, height: h), align: .left, color: Theme.muted)
        }
        let indent = CGFloat(row.depth) * 2 * cellWidth
        for (ci, col) in table.columns.enumerated() where ci < row.cells.count && ci < colX.count {
            let c = Self.content(row.cells[ci], column: col, row: row, live: live)
            var cellRect = NSRect(x: colX[ci], y: y, width: colW[ci], height: h)
            if ci == 0 { cellRect.origin.x += indent; cellRect.size.width -= indent }
            if isFlashing, let lf = col.live, let live, live.changed & HotRow.changedBit(lf) != 0 {
                let up = live.flags & HotRow.tickUp != 0
                ctx.setFillColor((up ? Theme.flashUp : Theme.flashDown).cgColor)
                ctx.fill(cellRect.insetBy(dx: -3, dy: 1))
            }
            drawText(ctx, c.text, in: cellRect, align: col.align, color: c.style.color, bold: row.emphasis)
        }
    }

    /// What one cell shows: the text the grid draws (and VoiceOver reads),
    /// its style, and whether the text was formatted from a number. Live
    /// columns take the streamed value when there is one.
    struct CellContent: Equatable {
        var text: String
        var style: StyleFfi
        var formatted: Bool
    }

    static func content(_ cell: CellFfi, column col: ColumnFfi, row: RowFfi, live: HotRow?) -> CellContent {
        var value = cell.value
        var style = cell.style
        if let lf = col.live, let live { value = live.value(lf) }
        let text: String
        let formatted: Bool
        if let t = cell.text, col.live == nil || value == nil {
            // A cell showing the row's own security key reads as the ticker.
            text = t == row.security ? SecurityText.ticker(t) : t
            formatted = false
        } else {
            text = TerminalFormatter.string(value, col.format)
            formatted = true
        }
        if let s = TerminalFormatter.signedStyle(value, col.format) { style = s }
        if row.emphasis && style == .normal { style = .emphasis }
        return CellContent(text: text, style: style, formatted: formatted)
    }

    private struct LineKey: Hashable {
        let text: String
        let color: NSColor
        let bold: Bool
    }

    private static var lines: [LineKey: CTLine] = [:]

    private func line(_ text: String, _ color: NSColor, _ bold: Bool) -> CTLine {
        let key = LineKey(text: text, color: color, bold: bold)
        if let l = Self.lines[key] { return l }
        if Self.lines.count > 20_000 { Self.lines.removeAll(keepingCapacity: true) }
        let attrs: [NSAttributedString.Key: Any] = [.font: bold ? boldFont : font, .foregroundColor: color]
        let l = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attrs))
        Self.lines[key] = l
        return l
    }

    /// Draws `s` in `rect` (top-left coordinates; ctx text matrix flipped).
    private func drawText(_ ctx: CGContext, _ s: String, in rect: NSRect, align: AlignFfi, color: NSColor, bold: Bool = false) {
        guard !s.isEmpty else { return }
        let maxChars = max(1, Int(rect.width / cellWidth))
        var text = s
        var n = s.count
        if n > maxChars {
            text = String(s.prefix(max(0, maxChars - 1))) + "…"
            n = maxChars
        }
        let w = CGFloat(n) * cellWidth
        let x: CGFloat = switch align {
        case .left: rect.minX
        case .right: rect.maxX - w
        case .center: rect.midX - w / 2
        }
        ctx.textPosition = CGPoint(x: x, y: rect.minY + (rect.height - (ascent - font.descender)) / 2 + ascent)
        CTLineDraw(line(text, color, bold), ctx)
    }

    // MARK: Input

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        activate(Int(p.y / Theme.rowHeight) - 1)
    }

    /// Selects the page's row `vi` and runs its action (a click, or a
    /// VoiceOver press).
    @discardableResult
    private func activate(_ vi: Int) -> Bool {
        let rows = visibleRows
        guard vi >= 0, vi < rows.count else { return false }
        let ri = rows.lowerBound + vi
        selectedRow = ri
        onSelect?(ri)
        return true
    }

    // MARK: Accessibility

    private struct AXElements {
        var rows: [GridRowElement] = []
        var headers: [GridHeaderElement] = []
        var columns: [GridColumnElement] = []
        var headerGroup: GridHeaderGroupElement?
        var columnCount = -1
    }

    /// Nil until an assistive app first asks about the grid.
    private var ax: AXElements?

    /// Whether an assistive app has asked about the grid (tests check that
    /// drawing alone builds nothing).
    var hasAccessibilityElements: Bool { ax != nil }

    /// Data columns plus the number column of numbered tables.
    var axColumnCount: Int { table.columns.count + (table.numbered ? 1 : 0) }

    /// Accessibility column → data column; nil for the number column.
    private func axDataColumn(_ c: Int) -> Int? { table.numbered ? (c == 0 ? nil : c - 1) : c }

    /// Makes or reuses elements for the current table shape. Row and cell
    /// elements hold only indices, so they survive refreshes of the same
    /// shape and VoiceOver keeps its place.
    @discardableResult
    private func axElements() -> AXElements {
        var e = ax ?? AXElements()
        let cols = axColumnCount
        if e.columnCount != cols || e.headerGroup == nil {
            e.headerGroup = GridHeaderGroupElement(grid: self)
            e.headers = (0..<cols).map { GridHeaderElement(grid: self, column: $0) }
            e.columns = (0..<cols).map { GridColumnElement(grid: self, column: $0) }
            e.rows = []
            e.columnCount = cols
        }
        let n = visibleRows.count
        if e.rows.count > n {
            e.rows.removeLast(e.rows.count - n)
        } else if e.rows.count < n {
            e.rows += (e.rows.count..<n).map { GridRowElement(grid: self, vi: $0) }
        }
        ax = e
        return e
    }

    /// New table or page: tell VoiceOver only if it has been looking.
    private func axShapeChanged() {
        guard let old = ax else { return }
        let e = axElements()
        if e.columnCount != old.columnCount {
            NSAccessibility.post(element: self, notification: .layoutChanged)
        } else if e.rows.count != old.rows.count {
            NSAccessibility.post(element: self, notification: .rowCountChanged)
        }
    }

    var axRowElements: [GridRowElement] { axElements().rows }
    var axHeaderCells: [GridHeaderElement] { axElements().headers }
    var axHeaderGroup: GridHeaderGroupElement? { axElements().headerGroup }

    func axHeaderCell(_ c: Int) -> GridHeaderElement? {
        let h = axElements().headers
        return h.indices.contains(c) ? h[c] : nil
    }

    func axColumnTitle(_ c: Int) -> String? {
        guard let d = axDataColumn(c) else { return "Number" }
        guard table.columns.indices.contains(d) else { return nil }
        let t = SpokenText.header(table.columns[d].title)
        return t.isEmpty ? nil : t
    }

    /// The row's cells as drawn, for speech.
    private func axCells(_ ri: Int) -> [SpokenText.Cell] {
        let row = table.rows[ri]
        let live = row.security.flatMap { feed?.row(for: $0) }
        var out: [SpokenText.Cell] = []
        if table.numbered { out.append(SpokenText.Cell(header: "", text: "\(ri + 1)", format: .integer, formatted: false)) }
        for (ci, col) in table.columns.enumerated() where ci < row.cells.count {
            let c = Self.content(row.cells[ci], column: col, row: row, live: live)
            out.append(SpokenText.Cell(header: col.title, text: c.text, format: col.format, formatted: c.formatted))
        }
        return out
    }

    private func axRowIndex(_ vi: Int) -> Int? {
        let rows = visibleRows
        let ri = rows.lowerBound + vi
        return vi >= 0 && rows.contains(ri) ? ri : nil
    }

    func axRowDescription(_ vi: Int) -> String? {
        axRowIndex(vi).map { SpokenText.row(axCells($0)) }
    }

    func axCellText(row vi: Int, column c: Int) -> String? {
        guard let ri = axRowIndex(vi) else { return nil }
        let cells = axCells(ri)
        guard cells.indices.contains(c) else { return nil }
        let cell = cells[c]
        return SpokenText.value(cell.text, format: cell.format, formatted: cell.formatted)
    }

    func axRowHelp(_ vi: Int) -> String? {
        guard let ri = axRowIndex(vi), let a = table.rows[ri].action else { return nil }
        return "Opens \(FunctionLabel.short(a.function))"
    }

    func axIsSelected(_ vi: Int) -> Bool { axRowIndex(vi).map { $0 == selectedRow } ?? false }
    func axSelect(_ vi: Int) { if let ri = axRowIndex(vi) { selectedRow = ri } }
    func axActivate(_ vi: Int) -> Bool { activate(vi) }

    /// Frame in view coordinates. `row` -1 is the header row and nil the
    /// whole column; `column` nil is the whole row.
    private func axLocalFrame(row vi: Int?, column c: Int?) -> NSRect {
        let h = Theme.rowHeight
        var r: NSRect = switch vi {
        case nil: NSRect(x: 0, y: 0, width: bounds.width, height: CGFloat(visibleRows.count + 1) * h)
        case .some(-1): NSRect(x: 0, y: 0, width: bounds.width, height: h)
        case let v?: rowRect(v)
        }
        if let c {
            if let d = axDataColumn(c) {
                guard d < colX.count else { return .zero }
                r.origin.x = colX[d]
                r.size.width = colW[d]
            } else {
                r.origin.x = pad
                r.size.width = numberWidth * cellWidth
            }
        }
        return r
    }

    func axScreenFrame(row vi: Int?, column c: Int?) -> NSRect {
        NSAccessibility.screenRect(fromView: self, rect: axLocalFrame(row: vi, column: c))
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .table }

    override func accessibilityLabel() -> String? {
        guard pageCount > 1 else { return table.title }
        let p = "page \(page + 1) of \(pageCount)"
        return table.title.map { "\($0), \(p)" } ?? p
    }

    override func accessibilityHelp() -> String? {
        pageCount > 1 ? "Page Down and Page Up turn pages" : nil
    }

    override func accessibilityChildren() -> [Any]? {
        let e = axElements()
        return (e.headerGroup.map { [$0] } ?? []) + e.rows + e.columns
    }

    override func accessibilityRows() -> [Any]? { axElements().rows }

    override func accessibilityVisibleRows() -> [Any]? {
        let vis = visibleRect
        return axElements().rows.filter { rowRect($0.vi).intersects(vis) }
    }

    override func accessibilitySelectedRows() -> [Any]? {
        guard let s = selectedRow else { return [] }
        let rows = axElements().rows
        let vi = s - visibleRows.lowerBound
        return rows.indices.contains(vi) ? [rows[vi]] : []
    }

    override func setAccessibilitySelectedRows(_ rows: [Any]?) {
        if let r = rows?.first as? GridRowElement { axSelect(r.vi) }
    }

    override func accessibilityColumns() -> [Any]? { axElements().columns }
    override func accessibilityVisibleColumns() -> [Any]? { axElements().columns }
    override func accessibilityHeader() -> Any? { axElements().headerGroup }
    override func accessibilityColumnHeaderUIElements() -> [Any]? { axElements().headers }
    override func accessibilityRowCount() -> Int { visibleRows.count }
    override func accessibilityColumnCount() -> Int { axColumnCount }

    override func accessibilityCell(forColumn column: Int, row: Int) -> Any? {
        let rows = axElements().rows
        return rows.indices.contains(row) ? rows[row].cell(column) : nil
    }

    /// `NSAccessibility` declares hit testing nonisolated; AppKit calls it on
    /// the main thread.
    nonisolated override func accessibilityHitTest(_ point: NSPoint) -> Any? {
        nonisolated(unsafe) var hit: Any?
        MainActor.assumeIsolated { hit = axHit(point) }
        return hit
    }

    private func axHit(_ point: NSPoint) -> Any? {
        guard let window else { return self }
        let p = convert(window.convertPoint(fromScreen: point), from: nil)
        let vi = Int(floor(p.y / Theme.rowHeight)) - 1
        let e = axElements()
        if vi == -1 {
            return (0..<axColumnCount).first { axLocalFrame(row: -1, column: $0).contains(p) }.flatMap { axHeaderCell($0) } ?? e.headerGroup
        }
        guard e.rows.indices.contains(vi) else { return self }
        let row = e.rows[vi]
        return (0..<axColumnCount).first { axLocalFrame(row: vi, column: $0).contains(p) }.flatMap { row.cell($0) } ?? row
    }
}

/// SwiftUI wrapper for a table block.
struct TableBlockView: NSViewRepresentable {
    let table: TableFfi
    let page: Int
    let feed: QuoteFeed?
    let onSelect: (Int) -> Void

    func makeNSView(context: Context) -> TerminalGridView {
        let v = TerminalGridView()
        v.table = table
        v.page = page
        v.feed = feed
        v.onSelect = onSelect
        return v
    }

    func updateNSView(_ v: TerminalGridView, context: Context) {
        if v.table != table { v.table = table }
        if v.page != page { v.page = page }
        if v.feed !== feed { v.feed = feed }
        v.onSelect = onSelect
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: TerminalGridView, context: Context) -> CGSize? {
        // Fill the panel, but never narrower than the columns need (the
        // enclosing horizontal ScrollView pans wide tables).
        let natural = nsView.intrinsicContentSize.width
        let w = max(natural, proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? natural)
        return CGSize(width: w, height: nsView.contentHeight)
    }
}
