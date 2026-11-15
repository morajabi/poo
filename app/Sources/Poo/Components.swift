import Charts
import SwiftUI

struct TierBadge: View {
    var tier: Tier

    var body: some View {
        Text(tier.label)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(tier.color.opacity(0.18), in: Capsule())
            .foregroundStyle(tier.color)
            .help(tier.explanation)
    }
}

struct DiskBar: View {
    var report: Report

    var body: some View {
        let total = Double(max(report.disk.total, 1))
        VStack(alignment: .leading, spacing: 6) {
            GeometryReader { geo in
                HStack(spacing: 1) {
                    ForEach(report.visibleBuckets) { b in
                        Rectangle()
                            .fill(b.swiftColor)
                            .frame(width: max(0, geo.size.width * Double(b.bytes) / total))
                            .help("\(b.name): \(Fmt.bytes(b.bytes))")
                    }
                    let known = report.visibleBuckets.reduce(UInt64(0)) { $0 + $1.bytes }
                    let other = report.disk.used > known ? report.disk.used - known : 0
                    Rectangle()
                        .fill(Color.secondary.opacity(0.35))
                        .frame(width: max(0, geo.size.width * Double(other) / total))
                        .help("Everything else: \(Fmt.bytes(other))")
                    Rectangle().fill(Color.secondary.opacity(0.08))
                }
                .clipShape(RoundedRectangle(cornerRadius: 4))
            }
            .frame(height: 10)
            HStack {
                Text("\(Fmt.bytes(report.disk.free)) free")
                    .font(.callout.weight(.semibold))
                Text("of \(Fmt.bytes(report.disk.total))")
                    .foregroundStyle(.secondary)
                Spacer()
                Text("Dev stuff \(Fmt.bytes(report.total_bytes))")
                    .foregroundStyle(.secondary)
            }
            .font(.callout)
        }
    }
}

struct BucketDonut: View {
    var buckets: [BucketSummary]
    var total: UInt64
    var onSelect: ((BucketSummary) -> Void)?
    @State private var angle: Double?

    private var hovered: BucketSummary? {
        guard let angle else { return nil }
        var acc = 0.0
        for b in buckets {
            acc += Double(b.bytes)
            if angle <= acc { return b }
        }
        return nil
    }

    var body: some View {
        Chart(buckets) { b in
            SectorMark(
                angle: .value("Size", Double(b.bytes)),
                innerRadius: .ratio(0.64),
                angularInset: 1.2
            )
            .cornerRadius(3)
            .foregroundStyle(b.swiftColor)
            .opacity(hovered == nil || hovered?.id == b.id ? 1 : 0.35)
        }
        .chartLegend(.hidden)
        .chartAngleSelection(value: $angle)
        .chartBackground { _ in
            VStack(spacing: 2) {
                if let b = hovered {
                    Text(Fmt.bytes(b.bytes)).font(.title3.weight(.semibold)).monospacedDigit()
                    Text(b.name).font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
                } else {
                    Text(Fmt.bytes(total)).font(.title3.weight(.semibold)).monospacedDigit()
                    Text("dev stuff").font(.caption).foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: 110)
        }
        .onTapGesture {
            if let b = hovered { onSelect?(b) }
        }
    }
}

struct BucketLegendRow: View {
    var bucket: BucketSummary

    var body: some View {
        let unused = bucket.bytes(for: .junk) + bucket.bytes(for: .cold) + bucket.bytes(for: .idle)
        HStack(spacing: 8) {
            Image(systemName: bucket.symbol)
                .foregroundStyle(bucket.swiftColor)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(bucket.name).lineLimit(1)
                if unused > 0 {
                    Text("\(Fmt.bytes(unused)) not in use").font(.caption2).foregroundStyle(.teal)
                }
            }
            .layoutPriority(1)
            Spacer(minLength: 4)
            Text(Fmt.bytes(bucket.bytes))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .fixedSize()
        }
        .contentShape(Rectangle())
    }
}

/// Stacked bars by tier for a set of buckets.
struct TierStackChart: View {
    var buckets: [BucketSummary]

    struct Row: Identifiable {
        var id: String { bucket + tier.rawValue }
        var bucket: String
        var tier: Tier
        var bytes: Double
    }

    var body: some View {
        let rows = buckets.flatMap { b in
            Tier.allCases.map { Row(bucket: b.name, tier: $0, bytes: Double(b.bytes(for: $0)) / 1e9) }
        }.filter { $0.bytes > 0 }
        Chart(rows) { r in
            BarMark(x: .value("GB", r.bytes), y: .value("Bucket", r.bucket))
                .foregroundStyle(by: .value("Tier", r.tier.label))
        }
        .chartForegroundStyleScale(
            domain: Tier.allCases.map(\.label),
            range: Tier.allCases.map(\.color)
        )
        .chartXAxisLabel("GB")
        .frame(height: CGFloat(max(buckets.count, 1)) * 30 + 40)
    }
}

struct GroupBarChart: View {
    var groups: [GroupSummary]
    var color: Color

    var body: some View {
        Chart(groups.prefix(10)) { g in
            BarMark(x: .value("GB", Double(g.bytes) / 1e9), y: .value("Group", g.name))
                .foregroundStyle(color.gradient)
                .annotation(position: .trailing) {
                    Text(Fmt.bytes(g.bytes)).font(.caption2).foregroundStyle(.secondary)
                }
        }
        .chartXAxis(.hidden)
        .frame(height: CGFloat(min(groups.count, 10)) * 26 + 16)
    }
}

struct ItemRow: View {
    var item: Item
    var selected: Bool
    var onToggle: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Toggle("", isOn: Binding(get: { selected }, set: { _ in onToggle() }))
                .toggleStyle(.checkbox)
                .labelsHidden()
                .disabled(item.action.kind == "none")
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(item.name).lineLimit(1).truncationMode(.middle)
                    if let a = item.agent {
                        Text(a).font(.caption2).padding(.horizontal, 4).background(.quaternary, in: Capsule())
                    }
                }
                Text(item.reasons.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            if let r = item.risks?.first(where: { $0.level != "standard" }) {
                Image(systemName: r.level == "danger" ? "exclamationmark.octagon" : "exclamationmark.triangle")
                    .foregroundStyle(r.level == "danger" ? .red : .orange)
                    .help((item.risks ?? []).map(\.message).joined(separator: "\n"))
            }
            TierBadge(tier: item.tier)
            Text(Fmt.ago(item.lastUsedDate))
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 70, alignment: .trailing)
            Text(Fmt.bytes(item.total_bytes))
                .monospacedDigit()
                .frame(width: 76, alignment: .trailing)
        }
        .padding(.vertical, 2)
    }
}
