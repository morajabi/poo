import SwiftUI

/// The color for "can go": calm, never an alarm.
enum Palette {
    static let reclaim = Color.accentColor
    static let neutral = Color.secondary.opacity(0.28)
    static let free = Color.secondary.opacity(0.08)
}

struct TierBadge: View {
    var tier: Tier

    var body: some View {
        HStack(spacing: 3) {
            if let sym = tier.symbol { Image(systemName: sym).imageScale(.small) }
            Text(tier.label)
        }
        .font(.caption2.weight(.medium))
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(tier.color.opacity(0.14), in: Capsule())
        .foregroundStyle(tier.color)
        .help(tier.explanation)
        .fixedSize()
    }
}

/// Disk usage: used (gray), the part nothing is using (accent), free.
struct DiskBar: View {
    var report: Report
    /// Just the bar, for dense layouts that show the numbers elsewhere.
    var compact = false

    var body: some View {
        let total = Double(max(report.disk.total, 1))
        let reclaim = min(report.notInUseBytes, report.disk.used)
        let other = report.disk.used - reclaim
        VStack(alignment: .leading, spacing: 6) {
            GeometryReader { geo in
                HStack(spacing: 0) {
                    Rectangle().fill(Palette.neutral)
                        .frame(width: max(0, geo.size.width * Double(other) / total))
                        .help("In use: \(Fmt.bytes(other))")
                    Rectangle().fill(Palette.reclaim.opacity(0.85))
                        .frame(width: max(0, geo.size.width * Double(reclaim) / total))
                        .help("Not in use: \(Fmt.bytes(reclaim))")
                    Rectangle().fill(Palette.free)
                        .help("Free: \(Fmt.bytes(report.disk.free))")
                }
                .clipShape(RoundedRectangle(cornerRadius: 4))
            }
            .frame(height: compact ? 6 : 10)
            if !compact {
            HStack(spacing: 14) {
                Text("\(Fmt.bytes(report.disk.free)) free")
                    .fontWeight(.semibold)
                    + Text(" of \(Fmt.bytes(report.disk.total))").foregroundColor(.secondary)
                Spacer()
                legend(Palette.reclaim, "\(Fmt.bytes(reclaim)) not in use")
                legend(Palette.neutral, "\(Fmt.bytes(other)) in use")
            }
            .font(.callout)
            .monospacedDigit()
            }
        }
    }

    private func legend(_ c: Color, _ s: String) -> some View {
        HStack(spacing: 5) {
            Circle().fill(c).frame(width: 7, height: 7)
            Text(s).foregroundStyle(.secondary)
        }
    }
}

/// A part-of-whole bar: reclaimable in accent, the rest neutral.
struct PartBar: View {
    var part: UInt64
    var whole: UInt64
    var height: CGFloat = 8

    var body: some View {
        GeometryReader { geo in
            let f = whole > 0 ? min(1, Double(part) / Double(whole)) : 0
            ZStack(alignment: .leading) {
                Capsule().fill(Palette.neutral)
                Capsule().fill(Palette.reclaim)
                    .frame(width: part > 0 ? max(height, geo.size.width * f) : 0)
            }
        }
        .frame(height: height)
    }
}

/// Checkbox bound to the store's selection.
struct SelectBox: View {
    @Environment(Store.self) private var store
    var item: Item

    var body: some View {
        Toggle("", isOn: Binding(
            get: { store.selection.contains(item.id) },
            set: { on in if on { store.selection.insert(item.id) } else { store.selection.remove(item.id) } }
        ))
        .toggleStyle(.checkbox)
        .labelsHidden()
        .accessibilityLabel("Select \(item.name)")
        .disabled(!item.isCleanable)
        .help(item.isCleanable ? "Select to clean" : "Poo leaves this for you to manage")
    }
}

/// One item in Browse, Projects and the like.
struct ItemRow: View {
    var item: Item
    var subtitle: String?
    /// Set by lists that measure their own width once (Browse), so thousands of rows don't
    /// each lay out three variants. Nil picks per row.
    var density: Density? = nil

    enum Density {
        case full, noAge, compact

        /// Narrow (inspector open, small window): the age column goes first, then the badge
        /// shrinks to a dot, so the name keeps its room.
        init(width: CGFloat) {
            self = width >= 600 ? .full : width >= 420 ? .noAge : .compact
        }
    }

    var body: some View {
        Group {
            switch density {
            case .full: row(showsAge: true)
            case .noAge: row(showsAge: false)
            case .compact: row(showsAge: false, compact: true)
            case nil:
                ViewThatFits(in: .horizontal) {
                    row(showsAge: true)
                    row(showsAge: false)
                    row(showsAge: false, compact: true)
                }
            }
        }
        .padding(.vertical, 2)
    }

    private func row(showsAge: Bool, compact: Bool = false) -> some View {
        HStack(spacing: 10) {
            SelectBox(item: item)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    if compact {
                        Circle().fill(item.tier.color).frame(width: 7, height: 7).help(item.tier.label)
                    }
                    Text(item.name).lineLimit(1).truncationMode(.middle).layoutPriority(1)
                    if let a = item.agent {
                        Text(a).font(.caption2).padding(.horizontal, 4).background(.quaternary, in: Capsule()).fixedSize()
                    }
                }
                Text(subtitle ?? item.why)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            // ViewThatFits compares ideal widths: pin the ideal to the minimum, so the choice
            // follows the space there is, not how long this row's subtitle happens to be.
            .frame(minWidth: compact ? 0 : showsAge ? 180 : 140, idealWidth: compact ? 0 : showsAge ? 180 : 140, maxWidth: .infinity, alignment: .leading)
            .layoutPriority(1)
            if !compact {
                TierBadge(tier: item.tier)
                    .fixedSize()
            }
            if showsAge {
                Text(Fmt.ago(item.lastUsedDate))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(width: 64, alignment: .trailing)
            }
            SizeLabel(item: item)
                .frame(width: compact ? 76 : 96, alignment: .trailing)
        }
        .help(showsAge ? "" : "\(item.tier.label) · used \(Fmt.ago(item.lastUsedDate))")
    }
}

/// An item's size, with "frees X" under it when APFS sharing makes a clean return less.
struct SizeLabel: View {
    var item: Item

    var body: some View {
        // Each line stays on one line: a narrow column must never wrap "frees 122.9 MB".
        VStack(alignment: .trailing, spacing: 0) {
            Text(Fmt.bytes(item.total_bytes)).monospacedDigit().lineLimit(1).fixedSize()
            if item.freesDiffers, let f = item.frees {
                Text("frees \(Fmt.bytes(f))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                    .lineLimit(1)
                    .fixedSize()
                    .help("Cleaning returns \(Fmt.bytes(f)). The rest is shared with clones or hard links and stays until they go too.")
            }
        }
    }
}
