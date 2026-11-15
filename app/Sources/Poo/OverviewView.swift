import SwiftUI

/// A reassuring overview: what can go without touching anything in use, where it comes from,
/// bigger options when you need them, and what Poo keeps safe.
struct OverviewView: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    @State private var confirming: PlanSummary?
    @State private var emergencyGB: Double = 20

    struct Area: Identifiable {
        var bucket: BucketSummary
        var group: String
        var reclaim: UInt64
        var total: UInt64
        var id: String { bucket.id + "/" + group }
    }

    var body: some View {
        ScrollView {
            if let r = store.report {
                VStack(alignment: .leading, spacing: 28) {
                    DiskBar(report: r)
                    headline(r)
                    areas(r)
                    morePlans(r)
                    keptSafe(r)
                    footnotes(r)
                }
                .padding(24)
                .frame(maxWidth: 920, alignment: .leading)
                .frame(maxWidth: .infinity)
            } else {
                ContentUnavailableView("Scanning…", systemImage: "internaldrive")
                    .frame(maxWidth: .infinity, minHeight: 400)
            }
        }
        .navigationTitle("Overview")
        .sheet(item: $confirming) { p in
            CleanConfirmView(planID: p.id, free: p.id == "emergency" ? UInt64(emergencyGB * 1e9) : nil)
        }
    }

    // MARK: Headline

    @ViewBuilder
    private func headline(_ r: Report) -> some View {
        let stale = r.plan("stale")
        let bytes = stale?.bytes ?? 0
        HStack(alignment: .top, spacing: 16) {
            Image(systemName: bytes > 0 ? "leaf" : "checkmark.circle")
                .font(.system(size: 28, weight: .regular))
                .foregroundStyle(Palette.reclaim)
                .frame(width: 36)
            VStack(alignment: .leading, spacing: 8) {
                if bytes > 0 {
                    Text("You can free \(Fmt.bytes(bytes)) without touching anything in use")
                        .font(.title2.weight(.semibold))
                        .fixedSize(horizontal: false, vertical: true)
                    Text("Build outputs, caches and checkouts nothing has used for a while. Poo checks each one again right before cleaning, skips anything that came back to life, and backs up what can't be rebuilt.")
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 12) {
                        Button("Review & clean…") { if let stale { confirming = stale } }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.large)
                            .disabled(store.busy)
                        Button("Browse what's not in use") { nav.browse(statuses: [.notInUse]) }
                            .buttonStyle(.link)
                        Spacer()
                        Text("\(stale?.count ?? 0) items").font(.callout).foregroundStyle(.secondary)
                    }
                    .padding(.top, 4)
                } else {
                    Text("Nothing needs cleaning right now")
                        .font(.title2.weight(.semibold))
                    Text("Everything Poo found is in use or needs your call. Check back after you wrap up a project, or look through Browse.")
                        .foregroundStyle(.secondary)
                }
                if let last = store.lastClean, !last.dry_run {
                    Label("Last clean freed \(Fmt.bytes(last.freed_bytes)) · \(last.cleaned) cleaned, \(last.skipped) skipped", systemImage: "checkmark.circle")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 12))
    }

    // MARK: Where it's coming from

    static func areas(_ r: Report, limit: Int = 6) -> [Area] {
        let visible = Dictionary(uniqueKeysWithValues: r.visibleBuckets.map { ($0.id, $0) })
        var acc: [String: Area] = [:]
        for i in r.items {
            guard let b = visible[i.bucket] else { continue }
            let key = i.bucket + "/" + i.group
            var a = acc[key] ?? Area(bucket: b, group: i.group, reclaim: 0, total: 0)
            a.total += i.bytes
            if i.status == .notInUse { a.reclaim += i.bytes }
            acc[key] = a
        }
        return acc.values.filter { $0.reclaim > 0 }.sorted { $0.reclaim > $1.reclaim }.prefix(limit).map { $0 }
    }

    @ViewBuilder
    private func areas(_ r: Report) -> some View {
        let areas = Self.areas(r)
        if !areas.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .firstTextBaseline) {
                    Text("Where it's coming from").font(.title3.weight(.semibold))
                    Spacer()
                    Button("Show all") { nav.browse(statuses: [.notInUse]) }.buttonStyle(.link)
                }
                VStack(spacing: 2) {
                    ForEach(areas) { a in
                        Button { nav.browse(bucket: a.bucket.id, group: a.group) } label: { AreaRow(area: a) }
                            .buttonStyle(MenuRowStyle())
                    }
                }
                .padding(6)
                .background(.background.secondary, in: RoundedRectangle(cornerRadius: 12))
            }
        }
    }

    // MARK: More

    @ViewBuilder
    private func morePlans(_ r: Report) -> some View {
        let plans = r.plans.filter { $0.id != "stale" }
        if !plans.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                Text("If you need more").font(.title3.weight(.semibold))
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 230), spacing: 12)], spacing: 12) {
                    ForEach(plans) { p in planCard(p) }
                }
            }
        }
    }

    private func planCard(_ p: PlanSummary) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(p.name).font(.headline)
                Spacer()
                if p.id == "emergency" {
                    Stepper(value: $emergencyGB, in: 5...500, step: 5) { Text("\(Int(emergencyGB)) GB").monospacedDigit() }
                        .fixedSize()
                } else {
                    Text(Fmt.bytes(p.bytes)).font(.headline).monospacedDigit().foregroundStyle(.secondary)
                }
            }
            Text(p.description).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 4)
            HStack {
                Text(p.id == "emergency" ? "Frees the target with the least regret" : "\(p.count) items")
                    .font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Review & clean…") { confirming = p }
                    .disabled(store.busy || (p.id != "emergency" && p.count == 0))
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 120, alignment: .topLeading)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
    }

    // MARK: Kept safe

    private func keptSafe(_ r: Report) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Kept safe").font(.title3.weight(.semibold))
            Text("Poo doesn't clean these on its own. Routine cleanups skip them, and every risky step asks first.")
                .font(.callout).foregroundStyle(.secondary)
            HStack(spacing: 12) {
                safeTile("In use right now", r.bytes(for: .inUse), "hammer",
                         "Builds, dependencies and caches you are working with.") { nav.browse(statuses: [.inUse]) }
                safeTile("Needs a look", r.visibleBuckets.reduce(0) { $0 + $1.bytes(for: .review) }, "eye",
                         "Poo isn't sure; you decide.") { nav.browse(statuses: [.needsLook]) }
                safeTile("Protected", r.visibleBuckets.reduce(0) { $0 + $1.bytes(for: .protected) }, "lock",
                         "Never cleaned unless you pick Danger.") { nav.browse(statuses: [.needsLook]) }
            }
        }
    }

    private func safeTile(_ title: String, _ bytes: UInt64, _ symbol: String, _ detail: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 4) {
                Label(title, systemImage: symbol).font(.callout).foregroundStyle(.secondary)
                Text(Fmt.bytes(bytes)).font(.title3.weight(.semibold)).monospacedDigit()
                Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            .padding(12)
            .frame(maxWidth: .infinity, minHeight: 92, alignment: .topLeading)
            .contentShape(Rectangle())
        }
        .buttonStyle(MenuRowStyle())
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
        .help("Show in Browse")
    }

    @ViewBuilder
    private func footnotes(_ r: Report) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if let d = r.detected, !d.isEmpty {
                Text("Found on this Mac: \(d.joined(separator: ", "))")
            }
            if let note = store.lastClean?.snapshot_note {
                Label(note, systemImage: "clock.arrow.circlepath")
            }
            ForEach(r.warnings, id: \.self) { Text($0) }
            Text("Scanned \(Fmt.ago(r.scannedAt)) · \(Fmt.bytes(r.total_bytes)) of dev data across \(r.items.count) items")
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }
}

struct AreaRow: View {
    var area: OverviewView.Area

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: area.bucket.symbol)
                .foregroundStyle(area.bucket.swiftColor)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(area.group)
                Text(area.bucket.name).font(.caption).foregroundStyle(.secondary)
            }
            .frame(width: 200, alignment: .leading)
            PartBar(part: area.reclaim, whole: area.total)
            Text("\(Fmt.bytes(area.reclaim)) of \(Fmt.bytes(area.total))")
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(width: 150, alignment: .trailing)
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .contentShape(Rectangle())
    }
}
