// Report on-screen windows and how much of each is black.
//
// Used by the steam-diagnose workflow to tell a working Steam window from a
// black rectangle. Each large window is captured with `screencapture -l` and
// its pixels are sampled.
//
// Usage: swift scripts/window-stats.swift [output-dir]
// Prints one line per window; exits 0 always (it reports, it doesn't judge).
import CoreGraphics
import Foundation
import ImageIO

let outDir = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : NSTemporaryDirectory()

/// Fraction of near-black pixels and the mean colour of an image file.
func stats(of path: String) -> (black: Double, mean: (Double, Double, Double))? {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(src, 0, nil)
    else { return nil }
    // Downscale while drawing; sampling every pixel of a Retina capture is slow.
    let width = max(1, min(image.width, 400))
    let height = max(1, image.height * width / max(1, image.width))
    var pixels = [UInt8](repeating: 0, count: width * height * 4)
    // The buffer must stay valid while the context draws into it.
    let drawn = pixels.withUnsafeMutableBytes { buf -> Bool in
        guard let ctx = CGContext(
            data: buf.baseAddress, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return false }
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return true
    }
    guard drawn else { return nil }
    var black = 0
    var sum = (0.0, 0.0, 0.0)
    for i in stride(from: 0, to: pixels.count, by: 4) {
        let (r, g, b) = (pixels[i], pixels[i + 1], pixels[i + 2])
        if r < 8 && g < 8 && b < 8 { black += 1 }
        sum.0 += Double(r); sum.1 += Double(g); sum.2 += Double(b)
    }
    let n = Double(width * height)
    return (Double(black) / n, (sum.0 / n, sum.1 / n, sum.2 / n))
}

let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
    as? [[String: Any]] ?? []
for w in windows {
    let owner = w[kCGWindowOwnerName as String] as? String ?? "?"
    let pid = w[kCGWindowOwnerPID as String] as? Int ?? 0
    let id = w[kCGWindowNumber as String] as? Int ?? 0
    let layer = w[kCGWindowLayer as String] as? Int ?? 0
    let b = w[kCGWindowBounds as String] as? [String: Double] ?? [:]
    let (width, height) = (b["Width"] ?? 0, b["Height"] ?? 0)
    var line = "window id=\(id) owner=\(owner) pid=\(pid) layer=\(layer) size=\(Int(width))x\(Int(height))"
    if layer == 0 && width >= 200 && height >= 150 {
        let file = (outDir as NSString).appendingPathComponent("window-\(id).png")
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        task.arguments = ["-x", "-o", "-l", String(id), file]
        // waitUntilExit() on a process that never launched would crash.
        if (try? task.run()) != nil {
            task.waitUntilExit()
        }
        if let s = stats(of: file) {
            line += String(format: " black=%.0f%% mean=(%.0f,%.0f,%.0f)",
                           s.black * 100, s.mean.0, s.mean.1, s.mean.2)
            line += " capture=\(file)"
        } else {
            line += " capture=failed"
        }
    }
    print(line)
}
