import AppKit

// VoiceOver elements for `TerminalGridView`, which draws its rows into
// layers and so has no subviews to expose. Each element stores only its
// position (row, column); labels and frames are read from the grid when
// VoiceOver asks, so live prices are current without any work per tick,
// and nothing here exists until an assistive app queries the grid.
//
// `NSAccessibilityElement` is not main-actor isolated, so these classes are
// `nonisolated` and reach the grid through `onMain`.

/// Runs `f` on the main actor and returns its result, which may be an
/// element (not `Sendable`). AppKit asks for accessibility on the main
/// thread; `assumeIsolated` traps if that ever changes, so `out` is always
/// set when it is read.
private nonisolated func onMain<T>(_ f: @MainActor @Sendable () -> T) -> T {
    nonisolated(unsafe) var out: T?
    MainActor.assumeIsolated { out = f() }
    return out!
}

/// One row of the current page.
nonisolated final class GridRowElement: NSAccessibilityElement {
    private weak var grid: TerminalGridView?
    /// Index among the current page's rows.
    let vi: Int
    private var cells: [GridCellElement]?

    init(grid: TerminalGridView, vi: Int) {
        self.grid = grid
        self.vi = vi
        super.init()
    }

    var owner: TerminalGridView? { grid }

    func cell(_ column: Int) -> GridCellElement? {
        let all = cellElements()
        return all.indices.contains(column) ? all[column] : nil
    }

    private func cellElements() -> [GridCellElement] {
        if let cells { return cells }
        let g = grid
        let n = onMain { g?.axColumnCount ?? 0 }
        let made = (0..<n).map { GridCellElement(row: self, column: $0) }
        cells = made
        return made
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .row }
    override func accessibilityParent() -> Any? { grid }
    override func accessibilityIndex() -> Int { vi }
    override func accessibilityChildren() -> [Any]? { cellElements() }

    override func accessibilityLabel() -> String? {
        let (g, vi) = (grid, vi)
        return onMain { g?.axRowDescription(vi) }
    }

    override func accessibilityHelp() -> String? {
        let (g, vi) = (grid, vi)
        return onMain { g?.axRowHelp(vi) }
    }

    override func accessibilityFrame() -> NSRect {
        let (g, vi) = (grid, vi)
        return onMain { g?.axScreenFrame(row: vi, column: nil) ?? .zero }
    }

    override func isAccessibilitySelected() -> Bool {
        let (g, vi) = (grid, vi)
        return onMain { g?.axIsSelected(vi) ?? false }
    }

    override func setAccessibilitySelected(_ selected: Bool) {
        guard selected else { return }
        let (g, vi) = (grid, vi)
        onMain { g?.axSelect(vi) }
    }

    /// VoiceOver's VO-Space: what a click does (select, run the row's action).
    override func accessibilityPerformPress() -> Bool {
        let (g, vi) = (grid, vi)
        return onMain { g?.axActivate(vi) ?? false }
    }
}

/// One cell: its drawn value as words; VoiceOver adds the column header.
nonisolated final class GridCellElement: NSAccessibilityElement {
    private weak var row: GridRowElement?
    /// Accessibility column (the number column first in numbered tables).
    let column: Int

    init(row: GridRowElement, column: Int) {
        self.row = row
        self.column = column
        super.init()
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .cell }
    override func accessibilityParent() -> Any? { row }
    override func accessibilityRowIndexRange() -> NSRange { NSRange(location: row?.vi ?? 0, length: 1) }
    override func accessibilityColumnIndexRange() -> NSRange { NSRange(location: column, length: 1) }
    override func isAccessibilitySelected() -> Bool { row?.isAccessibilitySelected() ?? false }
    override func accessibilityPerformPress() -> Bool { row?.accessibilityPerformPress() ?? false }

    override func accessibilityLabel() -> String? {
        guard let row else { return nil }
        let (g, vi, c) = (row.owner, row.vi, column)
        return onMain { g?.axCellText(row: vi, column: c) }
    }

    override func accessibilityFrame() -> NSRect {
        guard let row else { return .zero }
        let (g, vi, c) = (row.owner, row.vi, column)
        return onMain { g?.axScreenFrame(row: vi, column: c) ?? .zero }
    }

    override func accessibilityColumnHeaderUIElements() -> [Any]? {
        let (g, c) = (row?.owner, column)
        return onMain { g?.axHeaderCell(c) }.map { [$0] }
    }
}

/// A column header cell ("Last", "percent change").
nonisolated final class GridHeaderElement: NSAccessibilityElement {
    private weak var grid: TerminalGridView?
    let column: Int

    init(grid: TerminalGridView, column: Int) {
        self.grid = grid
        self.column = column
        super.init()
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .cell }
    override func accessibilityColumnIndexRange() -> NSRange { NSRange(location: column, length: 1) }

    override func accessibilityParent() -> Any? {
        let g = grid
        return onMain { g?.axHeaderGroup }
    }

    override func accessibilityLabel() -> String? {
        let (g, c) = (grid, column)
        return onMain { g?.axColumnTitle(c) }
    }

    override func accessibilityFrame() -> NSRect {
        let (g, c) = (grid, column)
        return onMain { g?.axScreenFrame(row: -1, column: c) ?? .zero }
    }
}

/// The header row as a group of header cells.
nonisolated final class GridHeaderGroupElement: NSAccessibilityElement {
    private weak var grid: TerminalGridView?

    init(grid: TerminalGridView) {
        self.grid = grid
        super.init()
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? { "Column headers" }
    override func accessibilityParent() -> Any? { grid }

    override func accessibilityChildren() -> [Any]? {
        let g = grid
        return onMain { g?.axHeaderCells }
    }

    override func accessibilityFrame() -> NSRect {
        let g = grid
        return onMain { g?.axScreenFrame(row: -1, column: nil) ?? .zero }
    }
}

/// One column of the current page, for VoiceOver's column navigation.
nonisolated final class GridColumnElement: NSAccessibilityElement {
    private weak var grid: TerminalGridView?
    let column: Int

    init(grid: TerminalGridView, column: Int) {
        self.grid = grid
        self.column = column
        super.init()
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .column }
    override func accessibilityParent() -> Any? { grid }
    override func accessibilityIndex() -> Int { column }

    override func accessibilityLabel() -> String? {
        let (g, c) = (grid, column)
        return onMain { g?.axColumnTitle(c) }
    }

    override func accessibilityHeader() -> Any? {
        let (g, c) = (grid, column)
        return onMain { g?.axHeaderCell(c) }
    }

    override func accessibilityFrame() -> NSRect {
        let (g, c) = (grid, column)
        return onMain { g?.axScreenFrame(row: nil, column: c) ?? .zero }
    }

    override func accessibilityChildren() -> [Any]? {
        let (g, c) = (grid, column)
        return onMain { g?.axRowElements ?? [] }.compactMap { $0.cell(c) }
    }
}
