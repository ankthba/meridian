// Renders Meridian's app icon into app/Meridian/Assets.xcassets/AppIcon.appiconset.
// Original artwork in the app's own palette (Design/Theme.swift): a terminal
// screen with the panel function bar, a large amber monospace "M" and an
// underscore cursor. Run: swift scripts/make-icon.swift
import AppKit
import CoreText

let root = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().deletingLastPathComponent()
let fonts = root.appendingPathComponent("app/Meridian/Resources/Fonts")
let out = root.appendingPathComponent("app/Meridian/Assets.xcassets/AppIcon.appiconset")
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
for f in ["IosevkaFixedSS08-Bold.ttf", "IosevkaFixedSS08-Regular.ttf"] {
    CTFontManagerRegisterFontsForURL(fonts.appendingPathComponent(f) as CFURL, .process, nil)
}

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255, blue: CGFloat(hex & 0xFF) / 255, alpha: a)
}
let amber = rgb(0xFFA028), bar = rgb(0x7A0F0F), white = rgb(0xF2F2F2)

func font(_ name: String, _ size: CGFloat) -> CTFont {
    let f = CTFontCreateWithName(name as CFString, size, nil)
    precondition(CTFontCopyPostScriptName(f) as String == name, "font \(name) not found; run scripts/fetch-fonts.sh")
    return f
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
    ctx.drawLinearGradient(gradient([rgb(0x1A1B1F), rgb(0x050506)]), start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 924), options: [])

    // Panel function bar with the selected tab and two more.
    ctx.setFillColor(bar)
    ctx.fill(CGRect(x: 100, y: 100, width: 824, height: 150))
    ctx.setFillColor(rgb(0x000000, 0.25))
    ctx.fill(CGRect(x: 100, y: 244, width: 824, height: 6))
    ctx.setFillColor(white)
    ctx.fill(CGRect(x: 214, y: 156, width: 120, height: 40))
    ctx.setFillColor(rgb(0xFFA028, 0.9))
    ctx.fill(CGRect(x: 364, y: 156, width: 120, height: 40))
    ctx.fill(CGRect(x: 514, y: 156, width: 120, height: 40))

    // Amber M and a block cursor, centred together in the screen area,
    // over a faint amber glow.
    let mFont = font("Iosevka-Fixed-SS08-Bold", 560)
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: "M", attributes: [.font: mFont]))
    let mWidth = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
    let cap = CTFontGetCapHeight(mFont)
    // Underscore cursor after the M ("M_"): a prompt waiting for input. A
    // full-height block reads as a second letter at icon sizes.
    let gap: CGFloat = 26, cursorW: CGFloat = 170, cursorH: CGFloat = 52
    let groupW = mWidth + gap + cursorW
    let x0 = 512 - groupW / 2
    let baseline = 580 + cap / 2
    ctx.drawRadialGradient(gradient([rgb(0xFFA028, 0.16), rgb(0xFFA028, 0)]), startCenter: CGPoint(x: 512, y: 590), startRadius: 0, endCenter: CGPoint(x: 512, y: 590), endRadius: 400, options: [])
    text(ctx, "M", mFont, amber, x: x0, y: baseline)
    ctx.setFillColor(rgb(0xFFA028, 0.9))
    ctx.fill(CGRect(x: x0 + mWidth + gap, y: baseline - cursorH, width: cursorW, height: cursorH))

    // Top sheen.
    ctx.drawLinearGradient(gradient([rgb(0xFFFFFF, 0.09), rgb(0xFFFFFF, 0)]), start: CGPoint(x: 512, y: 250), end: CGPoint(x: 512, y: 520), options: [])
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
