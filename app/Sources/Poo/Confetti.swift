import AppKit
import SwiftUI

/// A short burst of confetti over its container after a clean. Two seconds, then nothing:
/// it ignores clicks and draws nothing under Reduce Motion.
struct ConfettiView: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var start = Date.now
    @State private var pieces = (0..<90).map { _ in Piece() }
    @State private var done = false

    private static let duration: TimeInterval = 2.2
    private static let colors: [Color] = [.pink, .orange, .yellow, .green, .mint, .cyan, .blue, .purple]

    var body: some View {
        if !reduceMotion && !done {
            TimelineView(.animation) { tl in
                Canvas { ctx, size in
                    Self.draw(&ctx, size: size, t: tl.date.timeIntervalSince(start), pieces: pieces)
                }
            }
            .allowsHitTesting(false)
            .accessibilityHidden(true)
            .onAppear {
                start = .now
                NSHapticFeedbackManager.defaultPerformer.perform(.levelChange, performanceTime: .now)
            }
            .task {
                // Stop the timeline once the last piece is gone.
                try? await Task.sleep(for: .seconds(Self.duration))
                done = true
            }
        }
    }

    static func draw(_ ctx: inout GraphicsContext, size: CGSize, t: TimeInterval, pieces: [Piece]) {
        guard t < duration else { return }
        let fade = min(1, (duration - t) / 0.5)
        for p in pieces {
            // Thrown up and out from the middle, then gravity and a little drag.
            let x = size.width / 2 + p.vx * t * (1 - t * 0.18) + sin(t * p.wobble) * 6
            let y = size.height * 0.4 + p.vy * t + 340 * t * t
            guard y < size.height + 10 else { continue }
            var c = ctx
            c.opacity = fade
            c.translateBy(x: x, y: y)
            c.rotate(by: .radians(p.spin * t + p.angle))
            // Flip in 3D-ish by squashing one axis.
            c.scaleBy(x: 1, y: abs(cos(t * p.flip)) * 0.8 + 0.2)
            let rect = CGRect(x: -p.w / 2, y: -p.h / 2, width: p.w, height: p.h)
            let shape = p.round ? Path(ellipseIn: rect) : Path(roundedRect: rect, cornerRadius: 1)
            c.fill(shape, with: .color(colors[p.color]))
        }
    }

    struct Piece {
        // A round burst, mostly upward.
        var vx: Double, vy: Double
        var spin = Double.random(in: -9...9)
        var angle = Double.random(in: 0 ... .pi)
        var flip = Double.random(in: 4...12)
        var wobble = Double.random(in: 3...8)
        var w = Double.random(in: 5...9)
        var h = Double.random(in: 3...6)
        var round = Double.random(in: 0...1) < 0.25
        var color = Int.random(in: 0..<8)

        init() {
            let angle = Double.random(in: -0.95 * .pi ... -0.05 * .pi)
            let speed = Double.random(in: 160...460)
            vx = cos(angle) * speed
            vy = sin(angle) * speed
        }
    }
}
