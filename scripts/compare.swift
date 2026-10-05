#!/usr/bin/env swift
// Side-by-side fidelity comparison: reference | ours | diff heat map.
//   swift scripts/compare.swift reference.png ours.png out.png
// Prints the share of pixels whose color differs by more than a tolerance.
import AppKit
import CoreGraphics

func load(_ p: String) -> CGImage {
    guard let img = NSImage(contentsOfFile: p), let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
        fatalError("cannot read \(p)")
    }
    return cg
}

func pixels(_ img: CGImage, _ w: Int, _ h: Int) -> [UInt8] {
    var buf = [UInt8](repeating: 0, count: w * h * 4)
    let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
    return buf
}

let args = CommandLine.arguments
guard args.count == 4 else { print("usage: compare.swift reference.png ours.png out.png"); exit(2) }
let ref = load(args[1]), ours = load(args[2])
let w = ref.width, h = ref.height
let a = pixels(ref, w, h), b = pixels(ours, w, h)
var diff = [UInt8](repeating: 0, count: w * h * 4)
var differing = 0
for i in stride(from: 0, to: a.count, by: 4) {
    let d = abs(Int(a[i]) - Int(b[i])) + abs(Int(a[i + 1]) - Int(b[i + 1])) + abs(Int(a[i + 2]) - Int(b[i + 2]))
    if d > 48 { differing += 1 }
    let v = UInt8(min(255, d))
    diff[i] = v; diff[i + 1] = 0; diff[i + 2] = v / 3; diff[i + 3] = 255
}
let outW = w * 3 + 20
let ctx = CGContext(data: nil, width: outW, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
ctx.setFillColor(CGColor(gray: 0.15, alpha: 1)); ctx.fill(CGRect(x: 0, y: 0, width: outW, height: h))
ctx.draw(ref, in: CGRect(x: 0, y: 0, width: w, height: h))
ctx.draw(ours, in: CGRect(x: w + 10, y: 0, width: w, height: h))
let dctx = CGContext(data: &diff, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
ctx.draw(dctx.makeImage()!, in: CGRect(x: 2 * w + 20, y: 0, width: w, height: h))
let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[3]))
print(String(format: "differing pixels: %.2f%%", Double(differing) / Double(w * h) * 100))
