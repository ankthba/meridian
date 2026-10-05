#!/usr/bin/env swift
// Lists the most frequent colors in a reference screenshot so theme tokens
// come from measurement, not guesses.   swift scripts/palette.swift ref.png [top]
import AppKit

let args = CommandLine.arguments
guard args.count >= 2, let img = NSImage(contentsOfFile: args[1]), let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    print("usage: palette.swift image.png [top]"); exit(2)
}
let top = args.count > 2 ? Int(args[2]) ?? 16 : 16
let w = cg.width, h = cg.height
var buf = [UInt8](repeating: 0, count: w * h * 4)
let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
var counts: [UInt32: Int] = [:]
for i in stride(from: 0, to: buf.count, by: 4) {
    let key = (UInt32(buf[i]) << 16) | (UInt32(buf[i + 1]) << 8) | UInt32(buf[i + 2])
    counts[key, default: 0] += 1
}
for (k, n) in counts.sorted(by: { $0.value > $1.value }).prefix(top) {
    print(String(format: "#%06X  %6.2f%%", k, Double(n) / Double(w * h) * 100))
}
