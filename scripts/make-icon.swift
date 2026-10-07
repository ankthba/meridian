// Renders Meridian's app icon into app/Meridian/Assets.xcassets/AppIcon.appiconset.
// Original artwork in the Instrument palette (docs/DESIGN.md): a graphite
// pane with its header (an underlined tab and a live dot) and a bone "M_"
// prompt in SF Mono. Run: swift scripts/make-icon.swift
import AppKit
import CoreText

let root = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().deletingLastPathComponent()
let out = root.appendingPathComponent("app/Meridian/Assets.xcassets/AppIcon.appiconset")
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: a)
}
let bone = rgb(0xE8E6E1), muted = rgb(0x8A877F), header = rgb(0x1B1B1B), line = rgb(0x2E2E2E), up = rgb(0x5BC98A)

/// SF Mono, the app's data font.
func mono(_ size: CGFloat, _ weight: NSFont.Weight) -> CTFont {
    NSFont.monospacedSystemFont(ofSize: size, weight: weight) as CTFont
}

/// Draws text with its baseline at (x, y) in a top-left-origin context;
/// returns its advance width.
@discardableResult
func text(_ ctx: CGContext, _ s: String, _ f: CTFont, _ color: CGColor, x: CGFloat, y: CGFloat) -> CGFloat {
    let attrs: [NSAttributedString.Key: Any] = [.font: f, .foregroundColor: color]
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: s, attributes: attrs))
    ctx.saveGState()
    ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
    ctx.textPosition = CGPoint(x: x, y: y)
    CTLineDraw(line, ctx)
    ctx.restoreGState()
    return CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
func gradient(_ colors: [CGColor]) -> CGGradient {
    CGGradient(colorsSpace: space, colors: colors as CFArray, locations: nil)!
}

/// Continuous-corner (superellipse) body, closer to macOS icon geometry than
/// a circular-cornered rounded rect.
func squircle(_ r: CGRect) -> CGPath {
    let p = CGMutablePath()
    let n: CGFloat = 5, steps = 720
    for i in 0...steps {
        let t = CGFloat(i) / CGFloat(steps) * 2 * .pi
        let c = cos(t), s = sin(t)
        let x = r.midX + r.width / 2 * (c < 0 ? -1 : 1) * pow(abs(c), 2 / n)
        let y = r.midY + r.height / 2 * (s < 0 ? -1 : 1) * pow(abs(s), 2 / n)
        i == 0 ? p.move(to: CGPoint(x: x, y: y)) : p.addLine(to: CGPoint(x: x, y: y))
    }
    p.closeSubpath()
    return p
}

/// The 1024-pt master, in top-left coordinates.
func draw(_ ctx: CGContext) {
    // macOS icon grid: 824-pt body centred on the 1024 canvas.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = squircle(body)

    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 14), blur: 30, color: rgb(0x000000, 0.5))
    ctx.addPath(shape)
    ctx.setFillColor(rgb(0x050506))
    ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    ctx.drawLinearGradient(gradient([rgb(0x1C1C1C), rgb(0x0C0C0C)]), start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 924), options: [])

    // Pane header: hairline below, the selected tab in bone, two muted tabs,
    // and the live dot.
    ctx.setFillColor(header)
    ctx.fill(CGRect(x: 100, y: 100, width: 824, height: 150))
    ctx.setFillColor(line)
    ctx.fill(CGRect(x: 100, y: 246, width: 824, height: 4))
    ctx.setFillColor(bone)
    ctx.fill(CGRect(x: 210, y: 166, width: 110, height: 16))
    ctx.setFillColor(muted.copy(alpha: 0.7)!)
    ctx.fill(CGRect(x: 352, y: 166, width: 92, height: 16))
    ctx.fill(CGRect(x: 476, y: 166, width: 92, height: 16))
    ctx.setFillColor(up)
    ctx.fillEllipse(in: CGRect(x: 790, y: 159, width: 30, height: 30))

    // Bone "M" and an underscore cursor, centred together in the pane.
    let mFont = mono(500, .bold)
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: "M", attributes: [.font: mFont]))
    let mWidth = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
    let cap = CTFontGetCapHeight(mFont)
    // Underscore cursor after the M ("M_"): a prompt waiting for input.
    let gap: CGFloat = 20, cursorW: CGFloat = 170, cursorH: CGFloat = 46
    let groupW = mWidth + gap + cursorW
    let x0 = 512 - groupW / 2
    let baseline = 590 + cap / 2
    text(ctx, "M", mFont, bone, x: x0, y: baseline)
    ctx.setFillColor(bone.copy(alpha: 0.85)!)
    ctx.fill(CGRect(x: x0 + mWidth + gap, y: baseline - cursorH, width: cursorW, height: cursorH))

    // Top sheen.
    ctx.drawLinearGradient(gradient([rgb(0xFFFFFF, 0.04), rgb(0xFFFFFF, 0)]), start: CGPoint(x: 512, y: 250), end: CGPoint(x: 512, y: 520), options: [])
    ctx.restoreGState()

    // Hairline rim so the body reads on dark Docks.
    ctx.addPath(squircle(body.insetBy(dx: 1.5, dy: 1.5)))
    ctx.setStrokeColor(rgb(0xFFFFFF, 0.12))
    ctx.setLineWidth(3)
    ctx.strokePath()
}

func render(_ px: Int) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    let g = NSGraphicsContext(bitmapImageRep: rep)!
    let ctx = g.cgContext
    ctx.interpolationQuality = .high
    ctx.setShouldAntialias(true)
    // Top-left origin, 1024-pt master scaled to the target size.
    ctx.translateBy(x: 0, y: CGFloat(px))
    ctx.scaleBy(x: CGFloat(px) / 1024, y: -CGFloat(px) / 1024)
    draw(ctx)
    g.flushGraphics()
    return rep.representation(using: .png, properties: [:])!
}

// macOS asset catalog slots: point size × scale.
var images: [[String: String]] = []
for pt in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let px = pt * scale
        let name = "icon_\(pt)x\(pt)\(scale == 2 ? "@2x" : "").png"
        try! render(px).write(to: out.appendingPathComponent(name))
        images.append(["idiom": "mac", "size": "\(pt)x\(pt)", "scale": "\(scale)x", "filename": name])
    }
}
let contents: [String: Any] = ["images": images, "info": ["author": "xcode", "version": 1]]
try! JSONSerialization.data(withJSONObject: contents, options: [.prettyPrinted, .sortedKeys]).write(to: out.appendingPathComponent("Contents.json"))
let catalog = out.deletingLastPathComponent().appendingPathComponent("Contents.json")
try! JSONSerialization.data(withJSONObject: ["info": ["author": "xcode", "version": 1]], options: [.prettyPrinted]).write(to: catalog)
try! render(1024).write(to: root.appendingPathComponent("docs/preview/app-icon.png"))
try! render(256).write(to: root.appendingPathComponent("docs/screenshots/icon.png"))
print("icon → \(out.path)")
