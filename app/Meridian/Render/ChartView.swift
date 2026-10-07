import AppKit
import MeridianCore
import SwiftUI

/// Decoded chart series. Prices are Float offsets from `origin` (keeps
/// precision while halving memory; see `pack_bars` in Rust).
struct ChartSeries {
    var ts: [Int64] = []
    var open: [Float] = []
    var high: [Float] = []
    var low: [Float] = []
    var close: [Float] = []
    var volume: [Float] = []
    var origin: Double = 0
    var studies: [(name: String, pane: Int, values: [Float])] = []
    var decimals: Int = 2
    var count: Int { ts.count }

    init() {}

    init(_ d: ChartDataFfi) {
        origin = d.origin
        decimals = Int(d.priceDecimals)
        let n = Int(d.count)
        d.bars.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            var off = 12
            func f32(_ o: Int) -> Float { Float(bitPattern: raw.loadUnaligned(fromByteOffset: o, as: UInt32.self).littleEndian) }
            ts = (0..<n).map { raw.loadUnaligned(fromByteOffset: off + $0 * 8, as: Int64.self).littleEndian }
            off += n * 8
            open = (0..<n).map { f32(off + $0 * 4) }; off += n * 4
            high = (0..<n).map { f32(off + $0 * 4) }; off += n * 4
            low = (0..<n).map { f32(off + $0 * 4) }; off += n * 4
            close = (0..<n).map { f32(off + $0 * 4) }; off += n * 4
            volume = (0..<n).map { f32(off + $0 * 4) }
        }
        studies = d.studies.map { st in
            let vals: [Float] = st.values.withUnsafeBytes { raw in
                (0..<(raw.count / 4)).map { Float(bitPattern: raw.loadUnaligned(fromByteOffset: $0 * 4, as: UInt32.self).littleEndian) }
            }
            return (st.name, Int(st.pane), vals)
        }
    }
}

enum DrawTool: String, CaseIterable {
    case cursor = "Cursor"
    case trend = "Trend"
    case hline = "H-Line"
}

struct Drawing: Codable, Equatable {
    var kind: String
    /// Bar index and absolute price at each end.
    var x1: Double, y1: Double, x2: Double, y2: Double
}

/// Price chart view: candles/bars/line/mountain, volume, overlays, lower
/// study panes, crosshair, pan/zoom, trend and horizontal lines.
final class PriceChartNSView: NSView {
    var series = ChartSeries() { didSet { resetViewport(); needsDisplay = true } }
    var style: ChartStyleFfi = .candles { didSet { needsDisplay = true } }
    var livePrice: Double? { didSet { if livePrice != oldValue { needsDisplay = true } } }
    var tool: DrawTool = .cursor
    var drawings: [Drawing] = [] { didSet { needsDisplay = true; onDrawingsChanged?(drawings) } }
    var onDrawingsChanged: (([Drawing]) -> Void)?

    /// Visible window in bar-index units (fractional for smooth zoom).
    private var viewStart: Double = 0
    private var viewEnd: Double = 1
    private var mouse: NSPoint?
    private var dragStart: (NSPoint, Double, Double)?
    private var pendingDrawing: Drawing?
    private let axisWidth: CGFloat = 70
    private let axisHeight: CGFloat = 18
    private var font = Theme.font(11)
    private(set) var lastDrawDuration: CFTimeInterval = 0

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self))
    }

    private func resetViewport() {
        viewStart = 0
        viewEnd = Double(max(series.count, 1))
    }

    // MARK: Layout

    private var lowerPanes: [Int] { Array(Set(series.studies.map(\.pane).filter { $0 > 0 })).sorted() }

    private func panes() -> (price: NSRect, volume: NSRect, lower: [NSRect]) {
        let plot = NSRect(x: 0, y: 0, width: bounds.width - axisWidth, height: bounds.height - axisHeight)
        let nLower = CGFloat(lowerPanes.count)
        let lowerH = nLower > 0 ? plot.height * 0.18 : 0
        let priceH = plot.height - lowerH * nLower
        let price = NSRect(x: 0, y: 0, width: plot.width, height: priceH)
        let volume = NSRect(x: 0, y: priceH * 0.8, width: plot.width, height: priceH * 0.2)
        let lower = (0..<lowerPanes.count).map { NSRect(x: 0, y: priceH + CGFloat($0) * lowerH, width: plot.width, height: lowerH) }
        return (price, volume, lower)
    }

    private func xFor(_ i: Double, _ r: NSRect) -> CGFloat {
        r.minX + CGFloat((i - viewStart) / max(viewEnd - viewStart, 1e-9)) * r.width
    }

    private func indexFor(_ x: CGFloat, _ r: NSRect) -> Double {
        viewStart + Double((x - r.minX) / max(r.width, 1)) * (viewEnd - viewStart)
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        let t0 = CACurrentMediaTime()
        defer { lastDrawDuration = CACurrentMediaTime() - t0 }
        Theme.background.setFill()
        bounds.fill()
        guard series.count > 0, let ctx = NSGraphicsContext.current?.cgContext else {
            ("No data" as NSString).draw(at: NSPoint(x: 10, y: 10), withAttributes: [.font: font, .foregroundColor: Theme.muted])
            return
        }
        let (pricePane, volPane, lowerRects) = panes()
        let n = series.count
        let lo = max(0, Int(floor(viewStart)))
        let hi = min(n, Int(ceil(viewEnd)))
        guard hi > lo else { return }

        // Price range over the visible window (plus overlays).
        var minP = Float.greatestFiniteMagnitude
        var maxP = -Float.greatestFiniteMagnitude
        var maxV: Float = 0
        for i in lo..<hi {
            minP = min(minP, series.low[i]); maxP = max(maxP, series.high[i]); maxV = max(maxV, series.volume[i])
        }
        for st in series.studies where st.pane == 0 {
            for i in lo..<min(hi, st.values.count) where st.values[i].isFinite {
                minP = min(minP, st.values[i]); maxP = max(maxP, st.values[i])
            }
        }
        if let lp = livePrice {
            let rel = Float(lp - series.origin)
            minP = min(minP, rel); maxP = max(maxP, rel)
        }
        let padP = max((maxP - minP) * 0.06, 1e-6)
        minP -= padP; maxP += padP
        let yFor: (Float) -> CGFloat = { v in pricePane.maxY - CGFloat((v - minP) / (maxP - minP)) * pricePane.height }

        // Grid + price axis
        ctx.setLineWidth(1)
        let ticks = niceTicks(Double(minP) + series.origin, Double(maxP) + series.origin, count: 6)
        for t in ticks {
            let y = yFor(Float(t - series.origin))
            ctx.setStrokeColor(Theme.hairline.cgColor)
            ctx.move(to: CGPoint(x: 0, y: y)); ctx.addLine(to: CGPoint(x: pricePane.maxX, y: y)); ctx.strokePath()
            (TerminalFormatter.fixed(t, series.decimals) as NSString).draw(at: NSPoint(x: pricePane.maxX + 6, y: y - 7), withAttributes: [.font: font, .foregroundColor: Theme.muted])
        }

        // Decimation: aggregate into pixel buckets when bars exceed pixels.
        let pxPerBar = pricePane.width / CGFloat(max(viewEnd - viewStart, 1))
        let step = max(1, Int(ceil(Double(1.5 / max(pxPerBar, 1e-6)))))
        let barW = max(1, min(pxPerBar * CGFloat(step) * 0.7, 12))

        // Volume
        if maxV > 0 {
            ctx.setFillColor(Theme.volume.cgColor)
            var i = lo
            while i < hi {
                let j = min(i + step, hi)
                var v: Float = 0
                for k in i..<j { v = max(v, series.volume[k]) }
                let x = xFor(Double(i) + Double(j - i) / 2, pricePane)
                let h = CGFloat(v / maxV) * volPane.height
                ctx.fill(CGRect(x: x - barW / 2, y: volPane.maxY - h, width: barW, height: h))
                i = j
            }
        }

        // Price series
        switch style {
        case .line, .mountain:
            let path = CGMutablePath()
            var first = true
            var i = lo
            while i < hi {
                let j = min(i + step, hi)
                // min/max per bucket preserves extremes at any zoom.
                var mn = Float.greatestFiniteMagnitude, mx = -Float.greatestFiniteMagnitude
                for k in i..<j { mn = min(mn, series.close[k]); mx = max(mx, series.close[k]) }
                let x = xFor(Double(i) + Double(j - i) / 2, pricePane)
                let pts = step > 1 ? [mn, mx] : [series.close[i]]
                for v in pts {
                    let p = CGPoint(x: x, y: yFor(v))
                    if first { path.move(to: p); first = false } else { path.addLine(to: p) }
                }
                i = j
            }
            if style == .mountain, let last = path.currentPoint as CGPoint? {
                let fill = path.mutableCopy()!
                fill.addLine(to: CGPoint(x: last.x, y: pricePane.maxY))
                fill.addLine(to: CGPoint(x: xFor(Double(lo), pricePane), y: pricePane.maxY))
                fill.closeSubpath()
                ctx.saveGState()
                ctx.addPath(fill)
                ctx.clip()
                let colors = [Theme.text.withAlphaComponent(0.08).cgColor, Theme.text.withAlphaComponent(0.0).cgColor] as CFArray
                if let g = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1]) {
                    ctx.drawLinearGradient(g, start: CGPoint(x: 0, y: pricePane.minY), end: CGPoint(x: 0, y: pricePane.maxY), options: [])
                }
                ctx.restoreGState()
            }
            ctx.setStrokeColor(Theme.text.cgColor)
            ctx.setLineWidth(1.8)
            ctx.addPath(path)
            ctx.strokePath()
        case .candles, .bars:
            var i = lo
            ctx.setLineWidth(1)
            while i < hi {
                let j = min(i + step, hi)
                let o = series.open[i], c = series.close[j - 1]
                var h = -Float.greatestFiniteMagnitude, l = Float.greatestFiniteMagnitude
                for k in i..<j { h = max(h, series.high[k]); l = min(l, series.low[k]) }
                let x = xFor(Double(i) + Double(j - i) / 2, pricePane)
                let color = (c >= o ? Theme.up : Theme.down).cgColor
                ctx.setStrokeColor(color)
                ctx.setFillColor(color)
                ctx.move(to: CGPoint(x: x, y: yFor(h))); ctx.addLine(to: CGPoint(x: x, y: yFor(l))); ctx.strokePath()
                if style == .candles {
                    let top = yFor(max(o, c)), bot = yFor(min(o, c))
                    ctx.fill(CGRect(x: x - barW / 2, y: top, width: barW, height: max(bot - top, 1)))
                } else {
                    ctx.move(to: CGPoint(x: x - barW / 2, y: yFor(o))); ctx.addLine(to: CGPoint(x: x, y: yFor(o)))
                    ctx.move(to: CGPoint(x: x, y: yFor(c))); ctx.addLine(to: CGPoint(x: x + barW / 2, y: yFor(c)))
                    ctx.strokePath()
                }
                i = j
            }
        }

        // Overlays (pane 0) and lower panes
        var legendX: CGFloat = 6
        for (si, st) in series.studies.enumerated() {
            let color = Theme.series[(si + 1) % Theme.series.count]
            if st.pane == 0 {
                strokeSeries(ctx, st.values, lo, hi, step, rect: pricePane, color: color) { yFor($0) }
                let last = st.values.last(where: { $0.isFinite }).map { Double($0) + series.origin }
                let label = "\(st.name) \(last.map { TerminalFormatter.fixed($0, series.decimals) } ?? "")" as NSString
                label.draw(at: NSPoint(x: legendX, y: 6), withAttributes: [.font: font, .foregroundColor: color])
                legendX += label.size(withAttributes: [.font: font]).width + 14
            }
        }
        for (pi, pane) in lowerPanes.enumerated() {
            let r = lowerRects[pi]
            ctx.setStrokeColor(Theme.line.cgColor)
            ctx.move(to: CGPoint(x: 0, y: r.minY)); ctx.addLine(to: CGPoint(x: bounds.width, y: r.minY)); ctx.strokePath()
            let lines = series.studies.enumerated().filter { $0.element.pane == pane }
            var mn = Float.greatestFiniteMagnitude, mx = -Float.greatestFiniteMagnitude
            for (_, st) in lines { for i in lo..<min(hi, st.values.count) where st.values[i].isFinite { mn = min(mn, st.values[i]); mx = max(mx, st.values[i]) } }
            guard mx > mn else { continue }
            let yp: (Float) -> CGFloat = { v in r.maxY - 4 - CGFloat((v - mn) / (mx - mn)) * (r.height - 8) }
            var lx: CGFloat = 6
            for (si, st) in lines {
                let color = Theme.series[(si + 1) % Theme.series.count]
                strokeSeries(ctx, st.values, lo, hi, step, rect: r, color: color, yp)
                let last = st.values.last(where: { $0.isFinite }) ?? .nan
                let label = "\(st.name) \(last.isFinite ? String(format: "%.2f", last) : "")" as NSString
                label.draw(at: NSPoint(x: lx, y: r.minY + 2), withAttributes: [.font: font, .foregroundColor: color])
                lx += label.size(withAttributes: [.font: font]).width + 14
            }
            (String(format: "%.2f", mx) as NSString).draw(at: NSPoint(x: r.maxX + 4, y: r.minY + 2), withAttributes: [.font: font, .foregroundColor: Theme.muted])
            (String(format: "%.2f", mn) as NSString).draw(at: NSPoint(x: r.maxX + 4, y: r.maxY - 14), withAttributes: [.font: font, .foregroundColor: Theme.muted])
        }

        // Time axis
        let axisY = bounds.height - axisHeight + 2
        let labelEvery = max(1, Int(Double(hi - lo) / Double(max(pricePane.width / 110, 1))))
        let intraday = series.count > 1 && (series.ts[min(1, n - 1)] - series.ts[0]) < 86_400_000_000_000
        var i = lo
        while i < hi {
            let x = xFor(Double(i) + 0.5, pricePane)
            let d = Date(timeIntervalSince1970: Double(series.ts[i]) / 1e9)
            let s = intraday ? Self.timeFmt.string(from: d) : Self.axisFmt.string(from: d)
            (s as NSString).draw(at: NSPoint(x: x - 20, y: axisY), withAttributes: [.font: font, .foregroundColor: Theme.muted])
            i += labelEvery
        }

        // Live last price
        let lastPrice = livePrice ?? (Double(series.close[n - 1]) + series.origin)
        let ly = yFor(Float(lastPrice - series.origin))
        ctx.setStrokeColor(Theme.muted.withAlphaComponent(0.7).cgColor)
        ctx.setLineDash(phase: 0, lengths: [2, 3])
        ctx.move(to: CGPoint(x: 0, y: ly)); ctx.addLine(to: CGPoint(x: pricePane.maxX, y: ly)); ctx.strokePath()
        ctx.setLineDash(phase: 0, lengths: [])
        let tag = NSRect(x: pricePane.maxX + 1, y: ly - 8, width: axisWidth - 2, height: 16)
        Theme.text.setFill(); NSBezierPath(roundedRect: tag, xRadius: 3, yRadius: 3).fill()
        (TerminalFormatter.fixed(lastPrice, series.decimals) as NSString).draw(at: NSPoint(x: tag.minX + 4, y: tag.minY + 1), withAttributes: [.font: font, .foregroundColor: Theme.bg])

        // Drawings
        for d in drawings + (pendingDrawing.map { [$0] } ?? []) {
            ctx.setStrokeColor(Theme.text2.cgColor)
            ctx.setLineWidth(1.2)
            if d.kind == "hline" {
                let y = yFor(Float(d.y1 - series.origin))
                ctx.move(to: CGPoint(x: 0, y: y)); ctx.addLine(to: CGPoint(x: pricePane.maxX, y: y)); ctx.strokePath()
                (TerminalFormatter.fixed(d.y1, series.decimals) as NSString).draw(at: NSPoint(x: 4, y: y - 14), withAttributes: [.font: font, .foregroundColor: Theme.text2])
            } else {
                ctx.move(to: CGPoint(x: xFor(d.x1, pricePane), y: yFor(Float(d.y1 - series.origin))))
                ctx.addLine(to: CGPoint(x: xFor(d.x2, pricePane), y: yFor(Float(d.y2 - series.origin))))
                ctx.strokePath()
            }
        }

        // Crosshair
        if let m = mouse, pricePane.contains(m) || lowerRects.contains(where: { $0.contains(m) }) {
            ctx.setStrokeColor(Theme.muted.cgColor)
            ctx.setLineDash(phase: 0, lengths: [2, 2])
            ctx.move(to: CGPoint(x: m.x, y: 0)); ctx.addLine(to: CGPoint(x: m.x, y: bounds.height - axisHeight))
            ctx.move(to: CGPoint(x: 0, y: m.y)); ctx.addLine(to: CGPoint(x: pricePane.maxX, y: m.y))
            ctx.strokePath()
            ctx.setLineDash(phase: 0, lengths: [])
            let idx = min(max(Int(indexFor(m.x, pricePane)), 0), n - 1)
            let o = Double(series.open[idx]) + series.origin, h = Double(series.high[idx]) + series.origin
            let l = Double(series.low[idx]) + series.origin, c = Double(series.close[idx]) + series.origin
            let d = Date(timeIntervalSince1970: Double(series.ts[idx]) / 1e9)
            let f = { (v: Double) in TerminalFormatter.fixed(v, self.series.decimals) }
            let info = "\(intraday ? Self.dateTimeFmt.string(from: d) : Self.dateFmt.string(from: d))  O \(f(o))  H \(f(h))  L \(f(l))  C \(f(c))  V \(TerminalFormatter.large(Double(series.volume[idx]), 2))"
            let box = NSRect(x: 6, y: 24, width: (info as NSString).size(withAttributes: [.font: font]).width + 16, height: 22)
            Theme.raised.setFill(); NSBezierPath(roundedRect: box, xRadius: 5, yRadius: 5).fill()
            Theme.line.setStroke(); NSBezierPath(roundedRect: box.insetBy(dx: 0.5, dy: 0.5), xRadius: 5, yRadius: 5).stroke()
            (info as NSString).draw(at: NSPoint(x: box.minX + 8, y: box.minY + 4), withAttributes: [.font: font, .foregroundColor: Theme.text])
            if pricePane.contains(m) {
                let price = Double(minP + Float((pricePane.maxY - m.y) / pricePane.height) * (maxP - minP)) + series.origin
                let pt = NSRect(x: pricePane.maxX + 1, y: m.y - 8, width: axisWidth - 2, height: 16)
                Theme.hover.setFill(); NSBezierPath(roundedRect: pt, xRadius: 3, yRadius: 3).fill()
                (TerminalFormatter.fixed(price, series.decimals) as NSString).draw(at: NSPoint(x: pt.minX + 4, y: pt.minY + 1), withAttributes: [.font: font, .foregroundColor: Theme.text])
            }
        }
    }

    private func strokeSeries(_ ctx: CGContext, _ v: [Float], _ lo: Int, _ hi: Int, _ step: Int, rect: NSRect, color: NSColor, _ y: (Float) -> CGFloat) {
        let path = CGMutablePath()
        var drawing = false
        var i = lo
        while i < min(hi, v.count) {
            let j = min(i + step, hi, v.count)
            let val = v[j - 1]
            if val.isFinite {
                let p = CGPoint(x: xFor(Double(i) + Double(j - i) / 2, rect), y: y(val))
                if drawing { path.addLine(to: p) } else { path.move(to: p); drawing = true }
            } else {
                drawing = false
            }
            i = j
        }
        ctx.setStrokeColor(color.cgColor)
        ctx.setLineWidth(1.2)
        ctx.addPath(path)
        ctx.strokePath()
    }

    private func niceTicks(_ lo: Double, _ hi: Double, count: Int) -> [Double] {
        let span = max(hi - lo, 1e-9)
        let raw = span / Double(count)
        let mag = pow(10, floor(log10(raw)))
        let norm = raw / mag
        let step = (norm < 1.5 ? 1 : norm < 3 ? 2 : norm < 7 ? 5 : 10) * mag
        var t = ceil(lo / step) * step
        var out: [Double] = []
        while t <= hi { out.append(t); t += step }
        return out
    }

    private static let dateFmt: DateFormatter = { let f = DateFormatter(); f.dateFormat = "EEE, MMM d, yyyy"; return f }()
    private static let axisFmt: DateFormatter = { let f = DateFormatter(); f.dateFormat = "MMM ''yy"; return f }()
    private static let timeFmt: DateFormatter = { let f = DateFormatter(); f.dateFormat = "HH:mm"; return f }()
    private static let dateTimeFmt: DateFormatter = { let f = DateFormatter(); f.dateFormat = "EEE, MMM d HH:mm"; return f }()

    // MARK: Interaction

    override func mouseMoved(with event: NSEvent) {
        mouse = convert(event.locationInWindow, from: nil)
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) {
        mouse = nil
        needsDisplay = true
    }

    private func priceAt(_ y: CGFloat) -> Double {
        // Recompute the visible price range the same way draw() does.
        let r = panes().price
        let lo = max(0, Int(floor(viewStart))), hi = min(series.count, Int(ceil(viewEnd)))
        guard hi > lo else { return 0 }
        var minP = Float.greatestFiniteMagnitude, maxP = -Float.greatestFiniteMagnitude
        for i in lo..<hi { minP = min(minP, series.low[i]); maxP = max(maxP, series.high[i]) }
        let pad = max((maxP - minP) * 0.06, 1e-6)
        minP -= pad; maxP += pad
        return Double(minP + Float((r.maxY - y) / r.height) * (maxP - minP)) + series.origin
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let p = convert(event.locationInWindow, from: nil)
        let r = panes().price
        switch tool {
        case .cursor:
            dragStart = (p, viewStart, viewEnd)
        case .hline:
            drawings.append(Drawing(kind: "hline", x1: 0, y1: priceAt(p.y), x2: 0, y2: 0))
        case .trend:
            let i = indexFor(p.x, r), y = priceAt(p.y)
            pendingDrawing = Drawing(kind: "trend", x1: i, y1: y, x2: i, y2: y)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        mouse = p
        let r = panes().price
        if tool == .trend, var d = pendingDrawing {
            d.x2 = indexFor(p.x, r)
            d.y2 = priceAt(p.y)
            pendingDrawing = d
        } else if let (start, s0, e0) = dragStart {
            let span = e0 - s0
            let shift = Double((start.x - p.x) / max(r.width, 1)) * span
            setViewport(s0 + shift, e0 + shift)
        }
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        if let d = pendingDrawing {
            drawings.append(d)
            pendingDrawing = nil
        }
        dragStart = nil
    }

    override func scrollWheel(with event: NSEvent) {
        let r = panes().price
        if event.modifierFlags.contains(.command) || abs(event.scrollingDeltaY) > abs(event.scrollingDeltaX) {
            // Zoom around the mouse.
            let anchor = indexFor(convert(event.locationInWindow, from: nil).x, r)
            let factor = exp(Double(event.scrollingDeltaY) * (event.hasPreciseScrollingDeltas ? 0.01 : 0.1))
            zoom(by: factor, anchor: anchor)
        } else {
            let span = viewEnd - viewStart
            let shift = Double(-event.scrollingDeltaX / max(r.width, 1)) * span
            setViewport(viewStart + shift, viewEnd + shift)
        }
    }

    override func magnify(with event: NSEvent) {
        let anchor = indexFor(convert(event.locationInWindow, from: nil).x, panes().price)
        zoom(by: 1 / (1 + Double(event.magnification)), anchor: anchor)
    }

    func zoom(by factor: Double, anchor: Double? = nil) {
        let a = anchor ?? (viewStart + viewEnd) / 2
        let newSpan = min(max((viewEnd - viewStart) * factor, 10), Double(max(series.count, 10)) * 1.05)
        let ratio = (a - viewStart) / max(viewEnd - viewStart, 1e-9)
        setViewport(a - newSpan * ratio, a - newSpan * ratio + newSpan)
    }

    func pan(by fraction: Double) {
        let span = viewEnd - viewStart
        setViewport(viewStart + span * fraction, viewEnd + span * fraction)
    }

    private func setViewport(_ s: Double, _ e: Double) {
        let span = e - s
        let n = Double(series.count)
        var s2 = s, e2 = e
        if s2 < -span * 0.1 { s2 = -span * 0.1; e2 = s2 + span }
        if e2 > n + span * 0.1 { e2 = n + span * 0.1; s2 = e2 - span }
        viewStart = s2
        viewEnd = e2
        needsDisplay = true
    }

    override func keyDown(with event: NSEvent) {
        switch event.charactersIgnoringModifiers {
        case "+", "=": zoom(by: 0.8)
        case "-": zoom(by: 1.25)
        case "0": resetViewport(); needsDisplay = true
        default:
            switch event.keyCode {
            case 123: pan(by: -0.1)
            case 124: pan(by: 0.1)
            default: super.keyDown(with: event)
            }
        }
    }

    /// Benchmark hook: renders `frames` frames while zooming/panning.
    func benchmarkFrames(_ frames: Int) -> [CFTimeInterval] {
        guard let rep = bitmapImageRepForCachingDisplay(in: bounds) else { return [] }
        var times: [CFTimeInterval] = []
        for f in 0..<frames {
            // Alternate zoom in/out and pans across the whole series.
            if f % 2 == 0 { zoom(by: f % 4 == 0 ? 0.9 : 1.1) } else { pan(by: f % 8 < 4 ? 0.02 : -0.02) }
            let t = CACurrentMediaTime()
            cacheDisplay(in: bounds, to: rep) // renders draw(_:) for real
            times.append(CACurrentMediaTime() - t)
        }
        return times
    }
}

/// SwiftUI chart block: loads data for a spec, binds the live price.
struct ChartBlockView: View {
    let spec: ChartSpecFfi
    let feed: QuoteFeed?
    @State private var series: ChartSeries?
    @State private var error: String?
    @State private var tool: DrawTool = .cursor
    @State private var livePrice: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                ForEach(DrawTool.allCases, id: \.self) { t in
                    Text(t.rawValue)
                        .font(Theme.ui(11.5))
                        .foregroundStyle((tool == t ? Theme.text : Theme.muted).swiftUI)
                        .padding(.vertical, 2)
                        .overlay(alignment: .bottom) { if tool == t { Rectangle().fill(Theme.text.swiftUI).frame(height: 1) } }
                        .contentShape(Rectangle())
                        .onTapGesture { tool = t }
                }
                Text("Clear").font(Theme.ui(11.5)).foregroundStyle(Theme.muted.swiftUI)
                    .onTapGesture { UserDefaults.standard.removeObject(forKey: drawingsKey); reloadToken += 1 }
                Spacer()
                if let error { Text(error).font(Theme.ui(11.5)).foregroundStyle(Theme.warn.swiftUI) }
                Text("scroll to zoom · drag to pan").font(Theme.ui(11)).foregroundStyle(Theme.muted.swiftUI)
            }
            .padding(.horizontal, 12)
            .frame(height: 22)
            PriceChartRepresentable(series: series ?? ChartSeries(), style: spec.style, livePrice: livePrice, tool: tool, drawingsKey: drawingsKey, reloadToken: reloadToken)
        }
        .task(id: spec) { await load() }
        .onReceive(Timer.publish(every: 0.25, on: .main, in: .common).autoconnect()) { _ in
            if let r = feed?.row(for: spec.security), r.last.isFinite { livePrice = r.last }
        }
    }

    @State private var reloadToken = 0
    private var drawingsKey: String { "drawings.\(spec.security)" }

    /// Chart loads in flight (snapshot mode waits for zero).
    nonisolated(unsafe) static var inFlight = 0

    private func load() async {
        guard let core = AppModel.shared.core else { return }
        Self.inFlight += 1
        defer { Self.inFlight -= 1 }
        do {
            let d = try await core.chartData(security: spec.security, interval: spec.interval, range: spec.range, studies: spec.indicators)
            series = ChartSeries(d)
            error = d.stale ? "OFFLINE — cached" : nil
        } catch {
            self.error = error.userMessage
        }
    }
}

struct PriceChartRepresentable: NSViewRepresentable {
    let series: ChartSeries
    let style: ChartStyleFfi
    let livePrice: Double?
    let tool: DrawTool
    let drawingsKey: String
    let reloadToken: Int

    final class Coordinator { var lastCount = -1; var lastFirst: Int64 = 0; var lastToken = -1 }
    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> PriceChartNSView {
        let v = PriceChartNSView()
        v.onDrawingsChanged = { d in
            if let data = try? JSONEncoder().encode(d) { UserDefaults.standard.set(data, forKey: drawingsKey) }
        }
        return v
    }

    func updateNSView(_ v: PriceChartNSView, context: Context) {
        let c = context.coordinator
        if c.lastCount != series.count || c.lastFirst != (series.ts.first ?? 0) {
            v.series = series
            c.lastCount = series.count
            c.lastFirst = series.ts.first ?? 0
        }
        if c.lastToken != reloadToken {
            c.lastToken = reloadToken
            let saved = UserDefaults.standard.data(forKey: drawingsKey).flatMap { try? JSONDecoder().decode([Drawing].self, from: $0) } ?? []
            if saved != v.drawings { v.drawings = saved }
        }
        v.style = style
        v.livePrice = livePrice
        v.tool = tool
    }
}
