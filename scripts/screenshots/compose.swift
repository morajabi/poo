import AppKit
// compose <in.png> <out.png> <light|dark> [menubar <cropTopPx> <statusText>]
let a = CommandLine.arguments
let rep = NSBitmapImageRep(data: try! Data(contentsOf: URL(fileURLWithPath: a[1])))!
var win = rep.cgImage!
let dark = a[3] == "dark"
let menubar = a.count > 4 && a[4] == "menubar"
let s: CGFloat = 2 // retina
if menubar, let crop = Int(a[5]) {
    win = win.cropping(to: CGRect(x: 0, y: crop, width: win.width, height: win.height - crop))!
}
let ww = CGFloat(win.width), wh = CGFloat(win.height)
let margin: CGFloat = menubar ? 48 * s : 40 * s
let barH: CGFloat = menubar ? 26 * s : 0
let W = menubar ? max(ww + 2 * margin, 760 * s) : ww + 2 * margin
let gap: CGFloat = 6 * s // panel below the menu bar, like the real one
let H = menubar ? barH + gap + wh + margin : wh + 2 * margin
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: Int(W), height: Int(H), bitsPerComponent: 8, bytesPerRow: 0, space: cs,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
// Wallpaper: a soft two-tone gradient, no imagery.
let top = dark ? CGColor(srgbRed: 0.13, green: 0.14, blue: 0.24, alpha: 1) : CGColor(srgbRed: 0.80, green: 0.86, blue: 0.97, alpha: 1)
let bottom = dark ? CGColor(srgbRed: 0.24, green: 0.16, blue: 0.27, alpha: 1) : CGColor(srgbRed: 0.98, green: 0.88, blue: 0.84, alpha: 1)
// A flat, calm wallpaper tone: crisp and tiny as PNG.
_ = bottom
ctx.setFillColor(top); ctx.fill(CGRect(x: 0, y: 0, width: W, height: H))
NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
var panelX = (W - ww) / 2
if menubar {
    let barY = H - barH
    ctx.setFillColor(dark ? CGColor(gray: 0, alpha: 0.28) : CGColor(gray: 1, alpha: 0.45))
    ctx.fill(CGRect(x: 0, y: barY, width: W, height: barH))
    let fg = dark ? NSColor.white : NSColor.black
    let font = NSFont.systemFont(ofSize: 13 * s, weight: .regular)
    func text(_ t: String, _ x: CGFloat, bold: Bool = false) -> CGFloat {
        let attr = NSAttributedString(string: t, attributes: [.font: bold ? NSFont.systemFont(ofSize: 13 * s, weight: .bold) : font, .foregroundColor: fg])
        let sz = attr.size(); attr.draw(at: CGPoint(x: x, y: barY + (barH - sz.height) / 2)); return sz.width
    }
    func symbol(_ n: String, _ x: CGFloat, size: CGFloat = 13) -> CGFloat {
        let cfg = NSImage.SymbolConfiguration(pointSize: size * s, weight: .regular).applying(.init(paletteColors: [fg]))
        guard let img = NSImage(systemSymbolName: n, accessibilityDescription: nil)?.withSymbolConfiguration(cfg) else { return 0 }
        img.draw(in: CGRect(x: x, y: barY + (barH - img.size.height) / 2, width: img.size.width, height: img.size.height)); return img.size.width
    }
    var x: CGFloat = 18 * s
    x += symbol("apple.logo", x, size: 14) + 20 * s
    x += text("Finder", x, bold: true) + 20 * s
    for m in ["File", "Edit", "View", "Go", "Window", "Help"] { x += text(m, x) + 20 * s }
    // Right side, from the edge inward: clock, control center, wifi, then Poo.
    var r = W - 16 * s
    let clock = NSAttributedString(string: "Mon 9:41 AM", attributes: [.font: font]).size().width
    r -= clock; _ = text("Mon 9:41 AM", r); r -= 18 * s
    r -= 16 * s; _ = symbol("switch.2", r); r -= 16 * s
    r -= 18 * s; _ = symbol("wifi", r); r -= 16 * s
    let label = a[6]
    let lw = NSAttributedString(string: label, attributes: [.font: font]).size().width + 22 * s
    let itemW = lw + 12 * s
    let itemX = r - itemW
    ctx.setFillColor(dark ? CGColor(gray: 1, alpha: 0.18) : CGColor(gray: 0, alpha: 0.10))
    ctx.addPath(CGPath(roundedRect: CGRect(x: itemX, y: barY + 3 * s, width: itemW, height: barH - 6 * s), cornerWidth: 5 * s, cornerHeight: 5 * s, transform: nil)); ctx.fillPath()
    let sx = itemX + 6 * s
    _ = symbol("internaldrive", sx); _ = text(label, sx + 22 * s)
    panelX = min(max(itemX + itemW / 2 - ww / 2, 16 * s), W - ww - 16 * s)
}
let rect = CGRect(x: panelX, y: menubar ? margin : margin, width: ww, height: wh)
let radius: CGFloat = menubar ? 14 * s : 0
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -10 * s), blur: 28 * s, color: CGColor(gray: 0, alpha: dark ? 0.5 : 0.22))
if menubar {
    let path = CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil)
    ctx.setFillColor(dark ? CGColor(gray: 0.15, alpha: 1) : CGColor(gray: 0.97, alpha: 1))
    ctx.addPath(path); ctx.fillPath()
    ctx.setShadow(offset: .zero, blur: 0, color: nil)
    ctx.addPath(path); ctx.clip(); ctx.draw(win, in: rect)
} else {
    ctx.draw(win, in: rect)
}
ctx.restoreGState()
if menubar {
    // A hairline edge like the system panel's.
    ctx.setStrokeColor(dark ? CGColor(gray: 1, alpha: 0.15) : CGColor(gray: 0, alpha: 0.12)); ctx.setLineWidth(1)
    ctx.addPath(CGPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), cornerWidth: radius, cornerHeight: radius, transform: nil)); ctx.strokePath()
}
let out = NSBitmapImageRep(cgImage: ctx.makeImage()!)
try! out.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
