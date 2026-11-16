// Renders the app icon into an .iconset directory:  swift Scripts/make-icon.swift <out.iconset>
// With a second argument it also writes a preview sheet (every size on light and dark):
//   swift Scripts/make-icon.swift <out.iconset> <preview.png>
// For Windows, the same icon with its tile filling the canvas, as Windows icons do, as an .ico:
//   swift Scripts/make-icon.swift --windows windows/crates/app/res/DiffCode.ico
import AppKit

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!

/// Windows icons fill their canvas; the Mac's sit on Apple's grid with room for a shadow.
let forWindows = CommandLine.arguments.count == 3 && CommandLine.arguments[1] == "--windows"

/// Draws the icon on a 1024-unit canvas (origin bottom-left).
///
/// One glyph on a dark tile: a short block on the left flows into a longer one on the right,
/// the way a changed hunk is joined across the two sides of a diff.
/// - Parameter small: below 64 px the glyph is drawn heavier so it survives the downscale.
func draw(_ ctx: CGContext, small: Bool) {
    // Apple's icon grid: an 824 pt rounded square centred on a 1024 pt canvas.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)

    if forWindows {
        // The whole drawing scaled up so the tile all but fills the canvas (a 2.5% margin), and no shadow.
        let k: CGFloat = 0.95 * 1024 / 824
        ctx.translateBy(x: 512, y: 512)
        ctx.scaleBy(x: k, y: k)
        ctx.translateBy(x: -512, y: -512)
    } else {
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: color(0x000000, 0.32))
        ctx.addPath(shape)
        ctx.setFillColor(color(0x15161A))
        ctx.fillPath()
        ctx.restoreGState()
    }

    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    let background = CGGradient(colorsSpace: space, colors: [color(0x2B2D35), color(0x111216)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(background, start: CGPoint(x: 0, y: body.maxY), end: CGPoint(x: 0, y: body.minY), options: [])
    // A faint glow behind the glyph gives the dark surface some depth.
    let glow = CGGradient(colorsSpace: space, colors: [color(0x4C8DFF, 0.22), color(0x4C8DFF, 0)] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(glow, startCenter: CGPoint(x: 512, y: 512), startRadius: 0,
                           endCenter: CGPoint(x: 512, y: 512), endRadius: 440, options: [])

    let barWidth: CGFloat = small ? 104 : 76
    let leftX: CGFloat = small ? 250 : 268
    let rightX = 1024 - leftX - barWidth
    let leftY: ClosedRange<CGFloat> = small ? 520...748 : 532...744
    let rightY: ClosedRange<CGFloat> = small ? 276...620 : 280...612

    // One gradient runs through the whole glyph, blue to mint.
    let ink = CGGradient(colorsSpace: space, colors: [color(0x4C8DFF), color(0x36D6A2)] as CFArray, locations: [0, 1])!
    func fill(_ path: CGPath, alpha: CGFloat) {
        ctx.saveGState()
        ctx.addPath(path)
        ctx.clip()
        ctx.setAlpha(alpha)
        ctx.drawLinearGradient(ink, start: CGPoint(x: leftX, y: 0), end: CGPoint(x: rightX + barWidth, y: 0),
                               options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
        ctx.restoreGState()
    }

    // The band starts and ends under the bars' centre lines, so no seam shows at their round ends.
    let a = leftX + barWidth / 2, b = rightX + barWidth / 2, mid = (b - a) * 0.5
    let band = CGMutablePath()
    band.move(to: CGPoint(x: a, y: leftY.upperBound))
    band.addCurve(to: CGPoint(x: b, y: rightY.upperBound),
                  control1: CGPoint(x: a + mid, y: leftY.upperBound), control2: CGPoint(x: b - mid, y: rightY.upperBound))
    band.addLine(to: CGPoint(x: b, y: rightY.lowerBound))
    band.addCurve(to: CGPoint(x: a, y: leftY.lowerBound),
                  control1: CGPoint(x: b - mid, y: rightY.lowerBound), control2: CGPoint(x: a + mid, y: leftY.lowerBound))
    band.closeSubpath()
    fill(band, alpha: small ? 0.46 : 0.36)

    func bar(_ x: CGFloat, _ y: ClosedRange<CGFloat>) -> CGPath {
        CGPath(roundedRect: CGRect(x: x, y: y.lowerBound, width: barWidth, height: y.upperBound - y.lowerBound),
               cornerWidth: barWidth / 2, cornerHeight: barWidth / 2, transform: nil)
    }
    fill(bar(leftX, leftY), alpha: 1)
    fill(bar(rightX, rightY), alpha: 1)

    // A soft highlight along the top edge.
    let sheen = CGGradient(colorsSpace: space, colors: [color(0xFFFFFF, 0.09), color(0xFFFFFF, 0)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(sheen, start: CGPoint(x: 0, y: body.maxY), end: CGPoint(x: 0, y: body.maxY - 300), options: [])
    ctx.restoreGState()

    ctx.addPath(CGPath(roundedRect: body.insetBy(dx: 1.5, dy: 1.5), cornerWidth: 184.5, cornerHeight: 184.5, transform: nil))
    ctx.setStrokeColor(color(0xFFFFFF, 0.10))
    ctx.setLineWidth(3)
    ctx.strokePath()
}

func image(_ pixels: Int) -> CGImage {
    let ctx = CGContext(data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    ctx.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
    draw(ctx, small: pixels < 64)
    return ctx.makeImage()!
}

func write(_ image: CGImage, to url: URL) throws {
    try NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])!.write(to: url)
}

if forWindows {
    // An .ico of PNG images, one per size Windows asks for (taskbar, title bar, Explorer at every scale).
    let sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
    let images = sizes.map { NSBitmapImageRep(cgImage: image($0)).representation(using: .png, properties: [:])! }
    var ico = Data()
    func le16(_ v: Int) { ico.append(contentsOf: [UInt8(v & 0xFF), UInt8((v >> 8) & 0xFF)]) }
    func le32(_ v: Int) { le16(v & 0xFFFF); le16((v >> 16) & 0xFFFF) }
    le16(0); le16(1); le16(sizes.count)
    var offset = 6 + 16 * sizes.count
    for (size, png) in zip(sizes, images) {
        ico.append(UInt8(size >= 256 ? 0 : size)); ico.append(UInt8(size >= 256 ? 0 : size))
        ico.append(0); ico.append(0)
        le16(1); le16(32); le32(png.count); le32(offset)
        offset += png.count
    }
    images.forEach { ico.append($0) }
    try ico.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
    exit(0)
}

guard (2...3).contains(CommandLine.arguments.count) else {
    FileHandle.standardError.write(Data("usage: make-icon.swift <out.iconset> [preview.png]\n".utf8))
    exit(2)
}
let out = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    try write(image(size), to: out.appendingPathComponent("icon_\(size)x\(size).png"))
    try write(image(size * 2), to: out.appendingPathComponent("icon_\(size)x\(size)@2x.png"))
}

if CommandLine.arguments.count == 3 {
    // Every size twice, on a light and on a dark backdrop; small ones are magnified so they can be judged.
    let sizes = [512, 256, 128, 64, 32, 16]
    let cell = 560, width = cell * sizes.count, height = cell * 2
    let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    for (row, backdrop) in [UInt32(0x1B1C20), 0xECEDF1].enumerated() {
        ctx.setFillColor(color(backdrop))
        ctx.fill(CGRect(x: 0, y: row * cell, width: width, height: cell))
        for (column, size) in sizes.enumerated() {
            let shown = size >= 128 ? size : size * (size >= 64 ? 2 : 4)
            ctx.interpolationQuality = size >= 128 ? .high : .none
            ctx.draw(image(size), in: CGRect(x: column * cell + (cell - shown) / 2, y: row * cell + (cell - shown) / 2,
                                             width: shown, height: shown))
        }
    }
    try write(ctx.makeImage()!, to: URL(fileURLWithPath: CommandLine.arguments[2]))
}
