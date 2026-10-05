import AppKit
import MeridianCore
import SwiftUI

/// High-frequency table grid. Draws only visible rows with cached text
/// attributes, overlays live hot-row values onto bound columns, and flashes
/// cells briefly when they change. NSTableView/SwiftUI List are not used for
/// streaming grids (ARCHITECTURE §7.3).
final class TerminalGridView: NSView {
    var table: TableFfi = TableFfi(title: nil, columns: [], rows: [], pageSize: nil, numbered: false) { didSet { recompute() } }
    var page: Int = 0 { didSet { needsDisplay = true } }
    var feed: QuoteFeed? {
        didSet {
            oldValue?.removeListener(self)
            feed?.addListener(self) { [weak self] ids in self?.liveChanged(Set(ids)) }
            needsDisplay = true
        }
    }
    var onSelect: ((Int) -> Void)?
    var selectedRow: Int? { didSet { needsDisplay = true } }

    private var colX: [CGFloat] = []
    private var colW: [CGFloat] = []
    private let numberWidth: CGFloat = 4
    private let pad: CGFloat = 6
    private var font = Theme.font()
    private var boldFont = Theme.font(weight: .medium)
    private let flashDuration: CFTimeInterval = 0.35

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { false }

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
        let cw = Theme.charWidth
        var x: CGFloat = pad
        colX = []
        colW = []
        if table.numbered {
            x += numberWidth * cw
        }
        for c in table.columns {
            colX.append(x)
            let w = CGFloat(max(c.width, 2)) * cw
            colW.append(w)
            x += w + pad * 1.5
        }
        invalidateIntrinsicContentSize()
        needsDisplay = true
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: (colX.last ?? 0) + (colW.last ?? 0) + pad, height: contentHeight)
    }

    /// Redraws rows whose security changed in the last poll.
    func liveChanged(_ ids: Set<UInt32>) {
        guard let feed else { return }
        let rows = visibleRows
        for (i, r) in table.rows[rows].enumerated() {
            if let s = r.security, let id = feed.idBySecurity[s], ids.contains(id) {
                setNeedsDisplay(rowRect(i))
            }
        }
        // Clear flashes shortly after.
        DispatchQueue.main.asyncAfter(deadline: .now() + flashDuration) { [weak self] in self?.needsDisplay = true }
    }

    private func rowRect(_ visibleIndex: Int) -> NSRect {
        NSRect(x: 0, y: CGFloat(visibleIndex + 1) * Theme.rowHeight, width: bounds.width, height: Theme.rowHeight)
    }

    override func draw(_ dirtyRect: NSRect) {
        Theme.background.setFill()
        dirtyRect.fill()
        let h = Theme.rowHeight
        let baseline: CGFloat = 2
        // Header
        if dirtyRect.minY < h {
            for (i, c) in table.columns.enumerated() {
                let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: Theme.white]
                drawText(c.title, in: NSRect(x: colX[i], y: baseline, width: colW[i], height: h), align: c.align, attrs: attrs)
            }
            if table.numbered {
                ("#" as NSString).draw(at: NSPoint(x: pad, y: baseline), withAttributes: [.font: font, .foregroundColor: Theme.white])
            }
            Theme.grid.setFill()
            NSRect(x: 0, y: h - 1, width: bounds.width, height: 1).fill()
        }
        let now = CACurrentMediaTime()
        let rows = visibleRows
        for (vi, ri) in rows.enumerated() {
            let rect = rowRect(vi)
            guard rect.intersects(dirtyRect) else { continue }
            let row = table.rows[ri]
            if selectedRow == ri {
                Theme.selection.setFill()
                rect.fill()
            }
            let live = row.security.flatMap { feed?.row(for: $0) }
            let changedAt = row.security.flatMap { s in feed?.idBySecurity[s].flatMap { feed?.lastChanged[$0] } }
            let flashing = changedAt.map { now - $0 < flashDuration } ?? false
            if table.numbered {
                ("\(ri + 1))" as NSString).draw(at: NSPoint(x: pad, y: rect.minY + baseline), withAttributes: [.font: font, .foregroundColor: Theme.white])
            }
            let indent = CGFloat(row.depth) * 2 * Theme.charWidth
            for (ci, col) in table.columns.enumerated() where ci < row.cells.count {
                let cell = row.cells[ci]
                var value = cell.value
                var style = cell.style
                var cellFlash = false
                if let lf = col.live, let live {
                    value = live.value(lf)
                    if flashing, live.changed & HotRow.changedBit(lf) != 0 { cellFlash = true }
                }
                let text: String = {
                    if let t = cell.text, col.live == nil || value == nil { return t }
                    return TerminalFormatter.string(value, col.format)
                }()
                if let s = TerminalFormatter.signedStyle(value, col.format) { style = s }
                if row.emphasis && style == .normal { style = .emphasis }
                var cellRect = NSRect(x: colX[ci], y: rect.minY, width: colW[ci], height: h)
                if ci == 0 { cellRect.origin.x += indent; cellRect.size.width -= indent }
                if cellFlash {
                    let up = live.map { $0.flags & HotRow.tickUp != 0 } ?? true
                    (up ? Theme.flashUp : Theme.flashDown).setFill()
                    cellRect.insetBy(dx: -2, dy: 0).fill()
                }
                let attrs: [NSAttributedString.Key: Any] = [
                    .font: row.emphasis ? boldFont : font,
                    .foregroundColor: style.color,
                ]
                drawText(text, in: NSRect(x: cellRect.minX, y: rect.minY + baseline, width: cellRect.width, height: h), align: col.align, attrs: attrs)
            }
        }
        if pageCount > 1 {
            let y = CGFloat(rows.count + 1) * h
            let label = "Page \(page + 1) of \(pageCount)  —  PgDn/PgUp (⌘↓/⌘↑) to page"
            (label as NSString).draw(at: NSPoint(x: pad, y: y + baseline), withAttributes: [.font: font, .foregroundColor: Theme.muted])
        }
    }

    private func drawText(_ s: String, in rect: NSRect, align: AlignFfi, attrs: [NSAttributedString.Key: Any]) {
        let ns = s as NSString
        var size = ns.size(withAttributes: attrs)
        var str = ns
        if size.width > rect.width, rect.width > 10 {
            // Truncate with an ellipsis to the column width.
            var t = s
            while !t.isEmpty, (t + "…" as NSString).size(withAttributes: attrs).width > rect.width { t.removeLast() }
            str = (t + "…") as NSString
            size = str.size(withAttributes: attrs)
        }
        let x: CGFloat
        switch align {
        case .left: x = rect.minX
        case .right: x = rect.maxX - size.width
        case .center: x = rect.midX - size.width / 2
        }
        str.draw(at: NSPoint(x: x, y: rect.minY), withAttributes: attrs)
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let vi = Int(p.y / Theme.rowHeight) - 1
        let rows = visibleRows
        guard vi >= 0, vi < rows.count else { return }
        let ri = rows.lowerBound + vi
        selectedRow = ri
        if event.clickCount >= 1 { onSelect?(ri) }
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
        v.needsDisplay = true
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: TerminalGridView, context: Context) -> CGSize? {
        CGSize(width: proposal.width ?? nsView.intrinsicContentSize.width, height: nsView.contentHeight)
    }
}
