// Renders Meridian's app icon into app/Meridian/Assets.xcassets/AppIcon.appiconset.
// Original artwork in the app's own palette (Design/Theme.swift): a terminal
// screen with the function bar, an amber monospace "M", candles and a
// command line. Run: swift scripts/make-icon.swift
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
let amber = rgb(0xFFA028), up = rgb(0x2FD36B), down = rgb(0xFF4D4D), bar = rgb(0x7A0F0F), white = rgb(0xF2F2F2)

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

/// The 1024-pt master, in top-left coordinates.
func draw(_ ctx: CGContext) {
    // macOS icon grid: 824-pt body centred on the 1024 canvas.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = CGPath(roundedRect: body, cornerWidth: 185, cornerHeight: 185, transform: nil)

    // Soft drop shadow below the body.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 12), blur: 28, color: rgb(0x000000, 0.45))
    ctx.addPath(shape)
    ctx.setFillColor(rgb(0x0B0B0B))
    ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()

    // Screen: near-black with a faint vertical falloff.
    let grad = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [rgb(0x1A1A1A), rgb(0x030303)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(grad, start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 924), options: [])

    // Function bar with a selected tab, as in every panel.
    ctx.setFillColor(bar)
    ctx.fill(CGRect(x: 100, y: 100, width: 824, height: 150))
    ctx.setFillColor(white)
    ctx.fill(CGRect(x: 214, y: 148, width: 150, height: 68))
    let barFont = font("Iosevka-Fixed-SS08-Bold", 56)
    text(ctx, "1)", barFont, bar, x: 236, y: 202)
    text(ctx, "2)", barFont, amber, x: 404, y: 202)
    text(ctx, "3)", barFont, amber, x: 544, y: 202)

    // Amber monospace M.
    text(ctx, "M", font("Iosevka-Fixed-SS08-Bold", 580), amber, x: 140, y: 712)

    // Candles trending up, with an amber average through them.
    let candles: [(o: CGFloat, c: CGFloat, h: CGFloat, l: CGFloat)] = [
        (655, 581, 551, 700), (588, 625, 563, 658), (622, 521, 491, 640),
        (524, 447, 417, 551), (450, 488, 424, 521), (484, 357, 320, 503),
    ]
    let x0: CGFloat = 482, step: CGFloat = 68, w: CGFloat = 42
    for (i, k) in candles.enumerated() {
        let x = x0 + CGFloat(i) * step
        let rising = k.c < k.o   // y grows downward
        let color = rising ? up : down
        ctx.setFillColor(color)
        ctx.fill(CGRect(x: x + w / 2 - 4, y: k.h, width: 8, height: k.l - k.h))
        ctx.fill(CGRect(x: x, y: min(k.o, k.c), width: w, height: max(8, abs(k.c - k.o))))
    }
    ctx.setStrokeColor(amber)
    ctx.setLineWidth(9)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.move(to: CGPoint(x: 500, y: 660))
    ctx.addCurve(to: CGPoint(x: 850, y: 400), control1: CGPoint(x: 620, y: 645), control2: CGPoint(x: 740, y: 520))
    ctx.strokePath()

    // Command line: prompt and block cursor.
    ctx.setFillColor(rgb(0x141414))
    ctx.fill(CGRect(x: 100, y: 770, width: 824, height: 154))
    ctx.setFillColor(rgb(0x2A2A2A))
    ctx.fill(CGRect(x: 100, y: 770, width: 824, height: 4))
    let cmdFont = font("Iosevka-Fixed-SS08-Bold", 84)
    text(ctx, ">", cmdFont, amber, x: 196, y: 876)
    let go = text(ctx, "GO", cmdFont, up, x: 290, y: 876)
    ctx.setFillColor(amber)
    ctx.fill(CGRect(x: 290 + go + 18, y: 806, width: 42, height: 84))
    ctx.restoreGState()

    // Hairline edge so the body reads on dark Docks.
    ctx.addPath(CGPath(roundedRect: body.insetBy(dx: 1.5, dy: 1.5), cornerWidth: 184, cornerHeight: 184, transform: nil))
    ctx.setStrokeColor(rgb(0xFFFFFF, 0.10))
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
print("icon → \(out.path)")
