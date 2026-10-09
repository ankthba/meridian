// Renders the DMG window background in the Instrument palette
// (docs/DESIGN.md): a graphite pane with the app's top bar, a faint chart
// grid and price line whose last-price tag shows the version, and the
// command bar telling you what to do.
//   swift scripts/make-dmg-background.swift <version> <out-dir>
// Writes <out-dir>/background.png and background@2x.png (660 × 400 pt).
import AppKit
import CoreText

let args = CommandLine.arguments
guard args.count == 3 else {
    FileHandle.standardError.write(Data("usage: make-dmg-background.swift <version> <out-dir>\n".utf8))
    exit(2)
}
let version = args[1]
let out = URL(fileURLWithPath: args[2], isDirectory: true)
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

let W: CGFloat = 660, H: CGFloat = 400
/// Where dmgbuild puts the icons (centers, in points from the top left).
let appCenter = CGPoint(x: 170, y: 190), linkCenter = CGPoint(x: 490, y: 190)

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: a)
}
let bg = rgb(0x121212), header = rgb(0x181818), line = rgb(0x2A2A2A), grid = rgb(0x1A1A1A)
let bone = rgb(0xE8E6E1), text2 = rgb(0xBEBBB3), muted = rgb(0x8A877F), up = rgb(0x5BC98A)

func ui(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> CTFont { NSFont.systemFont(ofSize: size, weight: weight) as CTFont }
func mono(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> CTFont { NSFont.monospacedSystemFont(ofSize: size, weight: weight) as CTFont }

func line(_ s: String, _ f: CTFont, _ color: CGColor, kern: CGFloat = 0) -> CTLine {
    var attrs: [NSAttributedString.Key: Any] = [.font: f, .foregroundColor: color]
    if kern != 0 { attrs[.kern] = kern }
    return CTLineCreateWithAttributedString(NSAttributedString(string: s, attributes: attrs))
}

func width(_ l: CTLine) -> CGFloat { CGFloat(CTLineGetTypographicBounds(l, nil, nil, nil)) }

/// Draws `l` with its baseline at (x, y) in the top-left-origin context.
func draw(_ ctx: CGContext, _ l: CTLine, x: CGFloat, y: CGFloat) {
    ctx.saveGState()
    ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
    ctx.textPosition = CGPoint(x: x, y: y)
    CTLineDraw(l, ctx)
    ctx.restoreGState()
}

/// A deterministic random walk: decoration, not data.
func walk(_ n: Int) -> [CGFloat] {
    var state: UInt64 = 0x9E37_79B9_7F4A_7C15
    var v: CGFloat = 0, out: [CGFloat] = []
    for i in 0..<n {
        state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        let r = CGFloat(Int64(bitPattern: state >> 11) % 2001) / 1000 - 1
        v += r * 0.9 + 0.045 * sin(CGFloat(i) / 9)
        out.append(v)
    }
    return out
}

func render(scale: CGFloat) -> Data {
    let px = (Int(W * scale), Int(H * scale))
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px.0, pixelsHigh: px.1, bitsPerSample: 8, samplesPerPixel: 4,
                               hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    let ctx = NSGraphicsContext(bitmapImageRep: rep)!.cgContext
    // Top-left origin, in points.
    ctx.translateBy(x: 0, y: CGFloat(px.1))
    ctx.scaleBy(x: scale, y: -scale)
    let hair = 1 / scale

    ctx.setFillColor(bg)
    ctx.fill(CGRect(x: 0, y: 0, width: W, height: H))

    // Chart grid.
    ctx.setFillColor(grid)
    for x in stride(from: CGFloat(40), to: W, by: 58) { ctx.fill(CGRect(x: x, y: 48, width: hair, height: H - 96)) }
    for y in stride(from: CGFloat(104), to: H - 48, by: 56) { ctx.fill(CGRect(x: 0, y: y, width: W, height: hair)) }

    // Price line with a faint mountain, under the icons.
    let values = walk(120)
    let lo = values.min()!, hi = values.max()!
    let top: CGFloat = 278, bottom: CGFloat = 336
    let pts = values.enumerated().map { i, v in
        CGPoint(x: CGFloat(i) / CGFloat(values.count - 1) * (W - 86), y: bottom - (v - lo) / (hi - lo) * (bottom - top))
    }
    let path = CGMutablePath()
    path.addLines(between: pts)
    let fill = path.mutableCopy()!
    fill.addLine(to: CGPoint(x: pts.last!.x, y: H - 48))
    fill.addLine(to: CGPoint(x: 0, y: H - 48))
    fill.closeSubpath()
    ctx.addPath(fill)
    ctx.setFillColor(rgb(0xE8E6E1, 0.035))
    ctx.fillPath()
    ctx.addPath(path)
    ctx.setStrokeColor(rgb(0xE8E6E1, 0.30))
    ctx.setLineWidth(1.4)
    ctx.setLineJoin(.round)
    ctx.strokePath()

    // Last-price line and tag, showing the version.
    let last = pts.last!
    ctx.setStrokeColor(rgb(0x8A877F, 0.55))
    ctx.setLineWidth(1)
    ctx.setLineDash(phase: 0, lengths: [3, 3])
    ctx.move(to: CGPoint(x: 0, y: last.y))
    ctx.addLine(to: CGPoint(x: W - 76, y: last.y))
    ctx.strokePath()
    ctx.setLineDash(phase: 0, lengths: [])
    let tag = line("v\(version)", mono(11, .semibold), bg)
    let tagRect = CGRect(x: W - 74, y: last.y - 9, width: width(tag) + 14, height: 18)
    ctx.addPath(CGPath(roundedRect: tagRect, cornerWidth: 3, cornerHeight: 3, transform: nil))
    ctx.setFillColor(bone)
    ctx.fillPath()
    draw(ctx, tag, x: tagRect.minX + 7, y: tagRect.maxY - 5)

    // Top bar: wordmark, then a live dot and what this window is for.
    ctx.setFillColor(header)
    ctx.fill(CGRect(x: 0, y: 0, width: W, height: 48))
    ctx.setFillColor(line)
    ctx.fill(CGRect(x: 0, y: 48 - hair, width: W, height: hair))
    draw(ctx, line("MERIDIAN", ui(11, .bold), bone, kern: 3), x: 24, y: 29)
    let ready = line("Install", ui(12), text2)
    let rx = W - 24 - width(ready)
    draw(ctx, ready, x: rx, y: 29)
    ctx.setFillColor(up)
    ctx.fillEllipse(in: CGRect(x: rx - 13, y: 22, width: 6, height: 6))

    // Arrow from the app to Applications.
    let ax0 = appCenter.x + 82, ax1 = linkCenter.x - 82, ay = appCenter.y
    ctx.setStrokeColor(muted)
    ctx.setLineWidth(1.5)
    ctx.setLineCap(.round)
    ctx.move(to: CGPoint(x: ax0, y: ay))
    ctx.addLine(to: CGPoint(x: ax1, y: ay))
    ctx.move(to: CGPoint(x: ax1 - 7, y: ay - 6))
    ctx.addLine(to: CGPoint(x: ax1, y: ay))
    ctx.addLine(to: CGPoint(x: ax1 - 7, y: ay + 6))
    ctx.strokePath()

    // Command bar: what to do, and a few things to type once it's open.
    ctx.setFillColor(header)
    ctx.fill(CGRect(x: 0, y: H - 48, width: W, height: 48))
    ctx.setFillColor(line)
    ctx.fill(CGRect(x: 0, y: H - 48, width: W, height: hair))
    draw(ctx, line("›", ui(17), muted), x: 20, y: H - 18)
    draw(ctx, line("Drag Meridian onto Applications", ui(13), text2), x: 38, y: H - 19)
    let hint = line("aapl 5y  ·  earnings this week  ·  filings", mono(11), muted)
    draw(ctx, hint, x: W - 20 - width(hint), y: H - 19)

    return rep.representation(using: .png, properties: [:])!
}

try render(scale: 1).write(to: out.appendingPathComponent("background.png"))
try render(scale: 2).write(to: out.appendingPathComponent("background@2x.png"))
print("wrote \(out.path)/background.png, background@2x.png")
