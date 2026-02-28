import AppKit

let inputPath = "/Users/nwarner/tmp/gotify-desktop-osx/src/assets/gotify-logo.png"
let outDir = "/Users/nwarner/tmp/gotify-desktop-osx/src-tauri/icons"

guard let source = NSImage(contentsOfFile: inputPath) else {
  fputs("Failed to load source image\n", stderr)
  exit(1)
}

let size = NSSize(width: 32, height: 32)
let statuses: [(String, NSColor)] = [
  ("connected", NSColor.systemGreen),
  ("connecting", NSColor.systemOrange),
  ("backoff", NSColor.systemOrange),
  ("disconnected", NSColor.systemRed)
]

for (name, color) in statuses {
  let image = NSImage(size: size)
  image.lockFocus()

  NSGraphicsContext.current?.imageInterpolation = .high
  source.draw(in: NSRect(x: 0, y: 0, width: 32, height: 32),
              from: NSRect(origin: .zero, size: source.size),
              operation: .sourceOver,
              fraction: 1.0)

  let dotRect = NSRect(x: 20, y: 1, width: 10, height: 10)
  NSColor.black.withAlphaComponent(0.25).setFill()
  NSBezierPath(ovalIn: dotRect.insetBy(dx: -1, dy: -1)).fill()
  color.setFill()
  NSBezierPath(ovalIn: dotRect).fill()

  image.unlockFocus()

  guard let tiffData = image.tiffRepresentation,
        let rep = NSBitmapImageRep(data: tiffData),
        let pngData = rep.representation(using: .png, properties: [:]) else {
    fputs("Failed to render icon \(name)\n", stderr)
    exit(1)
  }

  let outPath = "\(outDir)/tray-\(name).png"
  try pngData.write(to: URL(fileURLWithPath: outPath))
  print("wrote \(outPath)")
}
