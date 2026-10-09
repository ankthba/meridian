import AppKit
import MeridianCore
import Testing
@testable import Meridian

@Suite struct SpokenTextTests {
    @Test func signsPercentsAndSuffixesBecomeWords() {
        #expect(SpokenText.value("+1.74%", format: .changePercent(decimals: 2)) == "up 1.74 percent")
        #expect(SpokenText.value("-0.19", format: .change(decimals: 2)) == "down 0.19")
        #expect(SpokenText.value("0.00", format: .change(decimals: 2)) == "0.00")
        #expect(SpokenText.value("3.39T", format: .large(decimals: 2)) == "3.39 trillion")
        #expect(SpokenText.value("12.5%", format: .percent(decimals: 1)) == "12.5 percent")
        #expect(SpokenText.value("289.44", format: .price(decimals: 2)) == "289.44")
        #expect(SpokenText.value("--", format: .number(decimals: 2)) == "no data")
        // Text from the data reads as is, even in a signed column.
        #expect(SpokenText.value("-n/a-", format: .change(decimals: 2), formatted: false) == "-n/a-")
    }

    @Test func headersAndRanges() {
        #expect(SpokenText.header("% Chg") == "percent change")
        #expect(SpokenText.header("1D %") == "1 day percent")
        #expect(SpokenText.header("Mkt Cap") == "market cap")
        #expect(SpokenText.header("Last") == "Last")
        #expect(SpokenText.header("–") == "to")
        #expect(SpokenText.range("1Y") == "1 year")
        #expect(SpokenText.range("5D") == "5 days")
        #expect(SpokenText.range("YTD") == "year to date")
        #expect(SpokenText.range("weird") == "weird")
        #expect(SpokenText.list("iex · rt · alpaca   sec · eod") == "iex, rt, alpaca; sec, eod")
    }

    @Test func rowReadsNumbersWithTheirColumn() {
        let cells = [
            SpokenText.Cell(header: "Security", text: "AAPL", format: .text, formatted: false),
            SpokenText.Cell(header: "Name", text: "Apple Inc.", format: .text, formatted: false),
            SpokenText.Cell(header: "Last", text: "289.44", format: .price(decimals: 2), formatted: true),
            SpokenText.Cell(header: "% Chg", text: "+1.74%", format: .changePercent(decimals: 2), formatted: true),
            SpokenText.Cell(header: "Time", text: "14:32:05", format: .time, formatted: true),
            SpokenText.Cell(header: "Volume", text: "", format: .large(decimals: 1), formatted: true),
        ]
        #expect(SpokenText.row(cells) == "AAPL, Apple Inc., Last 289.44, percent change up 1.74 percent, 14:32:05")
    }

    @Test func suggestionRowsReadGroupTitleSubtitleHint() {
        let s = SuggestionFfi(kind: .security, display: "AAPL US Equity", detail: "Apple Inc.", completion: "AAPL US Equity ",
                              group: "Security", title: "AAPL", subtitle: "", hint: "", action: nil, best: true)
        #expect(s.spokenDescription == "Security, AAPL, Apple Inc., US Equity")
        let f = SuggestionFfi(kind: .function, display: "FILINGS", detail: "", completion: "aapl filings ",
                              group: "On AAPL", title: "Filings", subtitle: "10-K, 10-Q and 8-K", hint: "FILINGS", action: nil, best: false)
        #expect(f.spokenDescription == "On AAPL, Filings, 10-K, 10-Q and 8-K, FILINGS")
    }
}

@Suite struct GridAccessibilityTests {
    private func table(pageSize: UInt32? = nil) -> TableFfi {
        let cols = [
            ColumnFfi(title: "Security", format: .text, align: .left, width: 8, live: nil),
            ColumnFfi(title: "Name", format: .text, align: .left, width: 16, live: nil),
            ColumnFfi(title: "Last", format: .price(decimals: 2), align: .right, width: 9, live: .last),
            ColumnFfi(title: "% Chg", format: .changePercent(decimals: 2), align: .right, width: 8, live: .pctChange),
        ]
        func row(_ sec: String, _ name: String, _ last: Double, _ pct: Double) -> RowFfi {
            RowFfi(cells: [
                CellFfi(value: nil, text: sec, style: .normal),
                CellFfi(value: nil, text: name, style: .normal),
                CellFfi(value: last, text: nil, style: .normal),
                CellFfi(value: pct, text: nil, style: .normal),
            ], depth: 0, security: sec, action: ActionFfi(function: "DES", security: sec, args: []), emphasis: false)
        }
        return TableFfi(title: "Watchlist", columns: cols, rows: [
            row("AAPL US Equity", "Apple Inc.", 289.44, 1.74),
            row("MSFT US Equity", "Microsoft Corp.", 512.1, -0.42),
        ], pageSize: pageSize, numbered: false)
    }

    @Test func gridIsATableOfRowsAndCells() throws {
        let grid = TerminalGridView(frame: NSRect(x: 0, y: 0, width: 600, height: 200))
        grid.table = table()
        // Nothing is built until an assistive app asks, drawing included.
        if let rep = grid.bitmapImageRepForCachingDisplay(in: grid.bounds) { grid.cacheDisplay(in: grid.bounds, to: rep) }
        #expect(!grid.hasAccessibilityElements)

        #expect(grid.isAccessibilityElement())
        #expect(grid.accessibilityRole() == .table)
        #expect(grid.accessibilityLabel() == "Watchlist")
        #expect(grid.accessibilityRowCount() == 2)
        #expect(grid.accessibilityColumnCount() == 4)
        let rows = try #require(grid.accessibilityRows() as? [GridRowElement])
        #expect(grid.hasAccessibilityElements)
        #expect(rows.count == 2)
        #expect(rows[0].accessibilityRole() == .row)
        #expect(rows[0].accessibilityLabel() == "AAPL, Apple Inc., Last 289.44, percent change up 1.74 percent")
        #expect(rows[1].accessibilityLabel() == "MSFT, Microsoft Corp., Last 512.10, percent change down 0.42 percent")
        #expect(rows[0].accessibilityHelp() == "Opens Overview")

        let cells = try #require(rows[1].accessibilityChildren() as? [GridCellElement])
        #expect(cells.count == 4)
        #expect(cells[0].accessibilityLabel() == "MSFT")
        #expect(cells[3].accessibilityLabel() == "down 0.42 percent")
        #expect(cells[3].accessibilityColumnIndexRange() == NSRange(location: 3, length: 1))
        #expect(cells[3].accessibilityRowIndexRange() == NSRange(location: 1, length: 1))
        let headers = try #require(grid.accessibilityColumnHeaderUIElements() as? [GridHeaderElement])
        #expect(headers.map { $0.accessibilityLabel() } == ["Security", "Name", "Last", "percent change"])
        #expect((grid.accessibilityCell(forColumn: 2, row: 0) as? GridCellElement)?.accessibilityLabel() == "289.44")
    }

    @Test func pressingARowDoesWhatAClickDoes() throws {
        let grid = TerminalGridView(frame: NSRect(x: 0, y: 0, width: 600, height: 200))
        grid.table = table()
        var selected: [Int] = []
        grid.onSelect = { selected.append($0) }
        let rows = try #require(grid.accessibilityRows() as? [GridRowElement])
        #expect(rows[1].accessibilityPerformPress())
        #expect(selected == [1])
        #expect(grid.selectedRow == 1)
        #expect(rows[1].isAccessibilitySelected())
        #expect((grid.accessibilitySelectedRows() as? [GridRowElement])?.first === rows[1])
        // A cell press presses its row.
        let cells = try #require(rows[0].accessibilityChildren() as? [GridCellElement])
        #expect(cells[2].accessibilityPerformPress())
        #expect(selected == [1, 0])
    }

    @Test func pagesExposeOnlyTheirRowsAndKeepElementsAcrossRefreshes() throws {
        let grid = TerminalGridView(frame: NSRect(x: 0, y: 0, width: 600, height: 200))
        grid.table = table(pageSize: 1)
        #expect(grid.accessibilityLabel() == "Watchlist, page 1 of 2")
        let first = try #require(grid.accessibilityRows() as? [GridRowElement])
        #expect(first.count == 1)
        grid.page = 1
        let second = try #require(grid.accessibilityRows() as? [GridRowElement])
        #expect(second.first === first.first)
        #expect(second.first?.accessibilityLabel()?.hasPrefix("MSFT") == true)
        // Same shape, new data: the same elements read the new values.
        var t = table(pageSize: 1)
        t.rows[1].cells[2] = CellFfi(value: 520, text: nil, style: .normal)
        grid.table = t
        let again = try #require(grid.accessibilityRows() as? [GridRowElement])
        #expect(again.first === first.first)
        #expect(again.first?.accessibilityLabel()?.contains("Last 520.00") == true)
    }

    @Test func liveValuesReplaceTheSnapshotInTextAndSpeech() {
        let t = table()
        var live = HotRow(instrument: 1, changed: 0, tsEvent: 0, bid: .nan, ask: .nan, last: 300, open: .nan, high: .nan, low: .nan,
                          prevClose: .nan, volume: .nan, bidSize: .nan, askSize: .nan, lastSize: .nan, netChange: .nan, pctChange: -2.5, flags: 0)
        let c = TerminalGridView.content(t.rows[0].cells[2], column: t.columns[2], row: t.rows[0], live: live)
        #expect(c == TerminalGridView.CellContent(text: "300.00", style: .normal, formatted: true))
        let p = TerminalGridView.content(t.rows[0].cells[3], column: t.columns[3], row: t.rows[0], live: live)
        #expect(p == TerminalGridView.CellContent(text: "-2.50%", style: .down, formatted: true))
        live.last = .nan
        let fallback = TerminalGridView.content(t.rows[0].cells[2], column: t.columns[2], row: t.rows[0], live: live)
        #expect(fallback.text == "")
        let ticker = TerminalGridView.content(t.rows[0].cells[0], column: t.columns[0], row: t.rows[0], live: nil)
        #expect(ticker == TerminalGridView.CellContent(text: "AAPL", style: .normal, formatted: false))
    }
}

@Suite struct ChartAccessibilityTests {
    @Test func summaryNamesRangeChangeHighAndLow() {
        var s = ChartSeries()
        let day: Int64 = 86_400_000_000_000
        s.ts = [0, day, 2 * day]
        s.origin = 100
        s.decimals = 2
        s.open = [0, 1, 2]
        s.close = [0, 2, 10]
        s.high = [1, 3, 12]
        s.low = [-1, 0, 1]
        s.volume = [1, 1, 1]
        let text = PriceChartNSView.summary(s, 0..<3, livePrice: nil)
        #expect(text.hasPrefix("From 100.00 on "))
        #expect(text.contains(" to 110.00 on "))
        #expect(text.hasSuffix(", change up 10.00, up 10.00 percent. High 112.00, low 99.00."))
        #expect(PriceChartNSView.summary(s, 0..<3, livePrice: 95).contains("change down 5.00, down 5.00 percent"))
        #expect(PriceChartNSView.summary(ChartSeries(), 0..<0, livePrice: nil) == "No data")

        let v = PriceChartNSView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
        v.accessibilityTitle = "AAPL price chart, 1 year"
        v.series = s
        #expect(v.accessibilityRole() == .image)
        #expect(v.accessibilityLabel()?.hasPrefix("AAPL price chart, 1 year. From 100.00") == true)
    }

    @Test func xyChartSummaryReadsEachSeries() {
        let chart = XyChartFfi(title: "Analyst ratings", xLabel: "Rating", yLabel: "Analysts", series: [
            XySeriesFfi(name: "Now", x: [0, 1, 2], y: [12, 20, .nan], style: .normal, bars: true),
        ], xMarker: nil, heightRows: 8, xCategories: ["Buy", "Hold", "Sell"])
        #expect(XyChartView.summary(chart) == "Bar chart. Now: Buy 12.00, Hold 20.00")
        let line = XyChartFfi(title: "Smile", xLabel: "Strike", yLabel: "IV %", series: [
            XySeriesFfi(name: "Oct 17", x: [90, 100, 110], y: [30, 25, 28], style: .normal, bars: false),
        ], xMarker: 100, heightRows: 8, xCategories: nil)
        #expect(XyChartView.summary(line) == "Line chart. Oct 17 from 30.00 at 90.00 to 28.00 at 110.00, low 25.00, high 30.00")
    }
}
