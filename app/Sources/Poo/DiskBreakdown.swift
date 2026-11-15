import SwiftUI

/// The whole disk in five labeled parts that add up to its capacity:
/// not in use, in use, check first & kept (what Poo tracks), other files, free.
struct DiskBreakdown {
    struct Segment: Identifiable {
        enum Kind: String { case notInUse, inUse, checkKept, other, free }
        var kind: Kind
        var label: String
        /// The real size shown in the legend.
        var bytes: UInt64
        /// The width it gets in the bar (clamped so the bar sums to the disk).
        var barBytes: UInt64
        var help: String
        var id: String { kind.rawValue }
    }

    var total: UInt64
    var used: UInt64
    var free: UInt64
    var tracked: UInt64
    var segments: [Segment]
    /// Poo tracks more than the volume reports as used (data on other volumes).
    var trackedExceedsUsed: Bool

    init(report r: Report) {
        let notInUse = r.bytes(for: .notInUse)
        let inUse = r.bytes(for: .inUse)
        let checkKept = r.bytes(for: .needsLook)
        let tracked = notInUse + inUse + checkKept
        let total = r.disk.total
        let used = min(r.disk.used, total)
        let other = used > tracked ? used - tracked : 0
        self.total = total
        self.used = used
        self.free = r.disk.free
        self.tracked = tracked
        self.trackedExceedsUsed = tracked > used

        // Bar widths: Poo's parts, then other, then whatever is left is free.
        var left = total
        func take(_ b: UInt64) -> UInt64 { let t = min(b, left); left -= t; return t }
        let bNot = take(notInUse), bIn = take(inUse), bCheck = take(checkKept), bOther = take(other)
        let bFree = left

        let otherHelp = trackedExceedsUsed
            ? "Poo tracks more than this volume reports as used: some of it (often simulator runtimes) lives on another volume, so Other files shows as 0."
            : "System, apps, documents and everything else Poo doesn't track."
        segments = [
            Segment(kind: .notInUse, label: "Not in use", bytes: notInUse, barBytes: bNot,
                    help: "Junk, Unused and Idle: dev data nothing is using right now. What a cleanup frees."),
            Segment(kind: .inUse, label: "In use", bytes: inUse, barBytes: bIn,
                    help: "In use and Active now: builds, dependencies and caches you are working with."),
            Segment(kind: .checkKept, label: "Check first & kept", bytes: checkKept, barBytes: bCheck,
                    help: "Items Poo won't clean on its own: they need a look, or you keep them on purpose."),
            Segment(kind: .other, label: "Other files", bytes: other, barBytes: bOther, help: otherHelp),
            Segment(kind: .free, label: "Free", bytes: free, barBytes: bFree, help: "Free space on this disk."),
        ]
    }

    var caption: String {
        "\(Fmt.bytes(total)) disk · \(Fmt.bytes(used)) used, of which Poo tracks \(Fmt.bytes(tracked)) · \(Fmt.bytes(free)) free"
    }

    static func fill(_ k: Segment.Kind) -> AnyShapeStyle {
        switch k {
        case .notInUse: return AnyShapeStyle(Palette.reclaim)
        case .inUse: return AnyShapeStyle(Color.secondary.opacity(0.6))
        case .checkKept: return AnyShapeStyle(Color.secondary.opacity(0.32))
        case .other: return AnyShapeStyle(Color.secondary.opacity(0.14))
        case .free: return AnyShapeStyle(Color.clear)
        }
    }
}

/// Stripes for "Other files", so it reads as not Poo's.
struct Hatch: View {
    var body: some View {
        Canvas { ctx, size in
            var path = Path()
            var x: CGFloat = -size.height
            while x < size.width {
                path.move(to: CGPoint(x: x, y: size.height))
                path.addLine(to: CGPoint(x: x + size.height, y: 0))
                x += 4
            }
            ctx.stroke(path, with: .color(.secondary.opacity(0.35)), lineWidth: 1)
        }
        .background(Color.secondary.opacity(0.10))
    }
}

struct DiskBreakdownBar: View {
    var breakdown: DiskBreakdown
    var height: CGFloat = 11
    /// Shown as a link in the "Not in use" hover card.
    var onReview: (() -> Void)? = nil

    @State private var hovered: DiskBreakdown.Segment.Kind?
    @State private var cardHovered = false
    @State private var hideTask: Task<Void, Never>?

    struct Span {
        var segment: DiskBreakdown.Segment
        var x: CGFloat
        var width: CGFloat
    }

    /// Lays segments out left to right with 1 pt gaps; non-zero slivers get 3 pt so they stay visible.
    static func spans(_ d: DiskBreakdown, width: CGFloat) -> [Span] {
        let parts = d.segments.filter { $0.barBytes > 0 }
        let gaps = CGFloat(max(parts.count - 1, 0))
        let usable = max(0, width - gaps)
        let total = Double(max(d.total, 1))
        var x: CGFloat = 0
        var out: [Span] = []
        for (i, s) in parts.enumerated() {
            var w = usable * Double(s.barBytes) / total
            if s.kind != .free { w = max(w, 3) }
            if i == parts.count - 1 { w = max(0, width - x) }
            out.append(Span(segment: s, x: x, width: w))
            x += w + 1
        }
        return out
    }

    var body: some View {
        GeometryReader { geo in
            let spans = Self.spans(breakdown, width: geo.size.width)
            ZStack(alignment: .topLeading) {
                Capsule().fill(Color.secondary.opacity(0.07))
                ForEach(spans, id: \.segment.id) { sp in
                    fill(sp.segment)
                        .frame(width: sp.width, height: height)
                        .brightness(hovered == sp.segment.kind ? -0.06 : 0)
                        .offset(x: sp.x)
                }
            }
            .frame(height: height)
            .clipShape(Capsule())
            .overlay(Capsule().strokeBorder(Color.secondary.opacity(0.18), lineWidth: 0.5))
            .contentShape(Rectangle())
            .onContinuousHover { phase in
                switch phase {
                case .active(let p):
                    hideTask?.cancel()
                    hovered = spans.first { p.x >= $0.x && p.x <= $0.x + $0.width + 1 }?.segment.kind
                case .ended:
                    scheduleHide()
                }
            }
            .overlay(alignment: .topLeading) {
                if let k = hovered, let sp = spans.first(where: { $0.segment.kind == k }) {
                    let cardWidth: CGFloat = 250
                    let mid = sp.x + sp.width / 2
                    let x = min(max(0, mid - cardWidth / 2), max(0, geo.size.width - cardWidth))
                    SegmentCard(segment: sp.segment, total: breakdown.total, note: k == .other && breakdown.trackedExceedsUsed ? sp.segment.help : nil,
                                onReview: k == .notInUse ? onReview : nil)
                        .frame(width: cardWidth)
                        .offset(x: x, y: height + 6)
                        .onHover { inside in
                            cardHovered = inside
                            if inside { hideTask?.cancel() } else { scheduleHide() }
                        }
                        .transition(.opacity)
                }
            }
        }
        .frame(height: height)
        .zIndex(10)
        .animation(.easeOut(duration: 0.12), value: hovered)
    }

    private func scheduleHide() {
        hideTask?.cancel()
        hideTask = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(180))
            if !Task.isCancelled && !cardHovered { hovered = nil }
        }
    }

    @ViewBuilder
    private func fill(_ s: DiskBreakdown.Segment) -> some View {
        switch s.kind {
        case .other: Hatch()
        case .free: Color.clear
        default: Rectangle().fill(DiskBreakdown.fill(s.kind))
        }
    }
}

/// The hover card for a bar segment: what it is, how big, what share of the disk.
struct SegmentCard: View {
    var segment: DiskBreakdown.Segment
    var total: UInt64
    var note: String?
    var onReview: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                LegendSwatch(kind: segment.kind)
                Text(segment.label).font(.callout.weight(.semibold))
                Spacer(minLength: 4)
                Text(Fmt.bytes(segment.bytes)).font(.callout.weight(.semibold)).monospacedDigit()
            }
            Text("\(percent) of the disk").font(.caption).foregroundStyle(.secondary).monospacedDigit()
            Text(note ?? segment.help)
                .font(.caption)
                .fixedSize(horizontal: false, vertical: true)
            if let onReview {
                Button("Review & clean…", action: onReview)
                    .buttonStyle(.link)
                    .font(.caption)
                    .padding(.top, 2)
            }
        }
        .padding(10)
        .glassBackground(in: .rect(cornerRadius: 12, style: .continuous))
        .shadow(color: .black.opacity(0.15), radius: 8, y: 3)
    }

    private var percent: String {
        let p = total > 0 ? Double(segment.bytes) / Double(total) : 0
        return p.formatted(.percent.precision(.fractionLength(p < 0.1 ? 1 : 0)))
    }
}

struct LegendSwatch: View {
    var kind: DiskBreakdown.Segment.Kind

    var body: some View {
        Group {
            if kind == .other {
                Hatch()
            } else if kind == .free {
                Color.secondary.opacity(0.07)
            } else {
                Rectangle().fill(DiskBreakdown.fill(kind))
            }
        }
        .frame(width: 10, height: 10)
        .clipShape(RoundedRectangle(cornerRadius: 2.5))
        .overlay(RoundedRectangle(cornerRadius: 2.5).strokeBorder(Color.secondary.opacity(0.25), lineWidth: 0.5))
    }
}
