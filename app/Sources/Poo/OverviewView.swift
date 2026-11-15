import SwiftUI

/// Dense overview: the numbers, the one action, then where the space is and the biggest items.
struct OverviewView: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    @AppStorage("overviewItems") private var itemScope = "notInUse"

    struct Area: Identifiable {
        var bucket: BucketSummary
        var group: String
        var reclaim: UInt64
        var total: UInt64
        var count: Int
        var id: String { bucket.id + "/" + group }
    }

    var body: some View {
        ScrollView {
            if let r = store.report {
                VStack(alignment: .leading, spacing: 18) {
                    summary(r)
                    areas(r)
                    topItems(r)
                    footnotes(r)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
            } else {
                ContentUnavailableView("No scan yet", systemImage: "internaldrive", description: Text("Rescan to see what takes space."))
                    .frame(maxWidth: .infinity, minHeight: 400)
            }
        }
        .navigationTitle("Overview")
        .navigationSubtitle(store.report.map { "Scanned \(Fmt.ago($0.scannedAt)) · \($0.items.count) items" } ?? "")
    }

    // MARK: Summary

    private func summary(_ r: Report) -> some View {
        let stale = r.plan("stale")
        return VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 22) {
                stat("Free", r.disk.free, of: r.disk.total) { nil }
                stat("Not in use", r.bytes(for: .notInUse), color: Palette.reclaim) { [.notInUse] }
                stat("In use", r.bytes(for: .inUse)) { [.inUse] }
                stat("Check first", r.visibleBuckets.reduce(0) { $0 + $1.bytes(for: .review) }) { [.needsLook] }
                stat("Kept", r.visibleBuckets.reduce(0) { $0 + $1.bytes(for: .protected) }) { [.needsLook] }
                Spacer(minLength: 8)
                AskAgentButton(target: "overview", title: "Ask")
                plansMenu(r)
                Button("Review & clean \(Fmt.bytes(stale?.bytes ?? 0))…") { nav.review(plan: "stale") }
                    .buttonStyle(.borderedProminent)
                    .disabled(store.busy || (stale?.count ?? 0) == 0)
                    .help(stale?.description ?? "")
            }
            DiskBar(report: r, compact: true)
        }
        .padding(12)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
    }

    private func stat(_ label: String, _ bytes: UInt64, of total: UInt64? = nil, color: Color? = nil, filter: @escaping () -> Set<ItemStatus>?) -> some View {
        Button {
            if let f = filter() { nav.browse(statuses: f) }
        } label: {
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 5) {
                    if let color { Circle().fill(color).frame(width: 7, height: 7) }
                    Text(Fmt.bytes(bytes)).font(.body.weight(.semibold)).monospacedDigit()
                }
                Text(total.map { "\(label) of \(Fmt.bytes($0))" } ?? label)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(filter() == nil ? "" : "Show in Browse")
    }

    private func plansMenu(_ r: Report) -> some View {
        Menu("More plans") {
            ForEach(r.plans.filter { $0.id != "stale" && $0.id != "emergency" }) { p in
                Button("\(p.name) · \(Fmt.bytes(p.bytes))…") { nav.review(plan: p.id) }
                    .disabled(p.count == 0)
            }
            Menu("Emergency: free up to…") {
                ForEach([10, 20, 50, 100], id: \.self) { gb in
                    Button("\(gb) GB") { nav.review(plan: "emergency", free: UInt64(gb) * 1_000_000_000) }
                }
            }
        }
        .fixedSize()
        .help("Bigger cleanups that also take things you use. Each one is reviewed first.")
    }

    // MARK: Areas

    static func areas(_ r: Report, limit: Int = 8) -> [Area] {
        let visible = Dictionary(uniqueKeysWithValues: r.visibleBuckets.map { ($0.id, $0) })
        var acc: [String: Area] = [:]
        for i in r.items {
            guard let b = visible[i.bucket] else { continue }
            let key = i.bucket + "/" + i.group
            var a = acc[key] ?? Area(bucket: b, group: i.group, reclaim: 0, total: 0, count: 0)
            a.total += i.bytes
            if i.status == .notInUse { a.reclaim += i.bytes; a.count += 1 }
            acc[key] = a
        }
        return acc.values.filter { $0.reclaim > 0 }.sorted { $0.reclaim > $1.reclaim }.prefix(limit).map { $0 }
    }

    @ViewBuilder
    private func areas(_ r: Report) -> some View {
        let areas = Self.areas(r)
        if !areas.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                header("Not in use, by area", action: "Browse all") { nav.browse(statuses: [.notInUse]) }
                DenseList(areas) { a in
                    AreaRow(area: a)
                        .contentShape(Rectangle())
                        .onTapGesture { nav.browse(bucket: a.bucket.id, group: a.group, statuses: [.notInUse]) }
                        .contextMenu {
                            Button("Show in Browse") { nav.browse(bucket: a.bucket.id, group: a.group) }
                            AskAgentItems(target: "bucket:\(a.bucket.id)")
                        }
                }
            }
        }
    }

    // MARK: Items

    @ViewBuilder
    private func topItems(_ r: Report) -> some View {
        let pool = itemScope == "all" ? r.items : r.items.filter { $0.status == .notInUse }
        let items = pool.sorted { $0.total_bytes > $1.total_bytes }.prefix(12).map { $0 }
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Largest items").font(.headline)
                Picker("", selection: $itemScope) {
                    Text("Not in use").tag("notInUse")
                    Text("All").tag("all")
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .controlSize(.small)
                .fixedSize()
                Spacer()
                Button("Browse all") { nav.browse(statuses: itemScope == "all" ? [] : [.notInUse]) }
                    .buttonStyle(.link)
            }
            if items.isEmpty {
                Text("Nothing here right now.").font(.callout).foregroundStyle(.secondary).padding(.vertical, 6)
            } else {
                DenseList(items) { i in
                    DenseItemRow(item: i)
                }
            }
        }
    }

    private func header(_ title: String, action: String, perform: @escaping () -> Void) -> some View {
        HStack {
            Text(title).font(.headline)
            Spacer()
            Button(action, action: perform).buttonStyle(.link)
        }
    }

    @ViewBuilder
    private func footnotes(_ r: Report) -> some View {
        let lines = (r.warnings + [store.lastClean?.snapshot_note].compactMap { $0 })
        if !lines.isEmpty || store.lastClean != nil {
            VStack(alignment: .leading, spacing: 3) {
                if let last = store.lastClean, !last.dry_run {
                    Text("Last clean freed \(Fmt.bytes(last.freed_bytes)): \(last.cleaned) cleaned, \(last.skipped) skipped")
                }
                ForEach(lines, id: \.self) { Text($0) }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }
}

/// Rows in a rounded box with hairline separators, like a grouped table.
struct DenseList<Data: RandomAccessCollection, Row: View>: View where Data.Element: Identifiable {
    var data: Data
    var row: (Data.Element) -> Row

    init(_ data: Data, @ViewBuilder row: @escaping (Data.Element) -> Row) {
        self.data = data
        self.row = row
    }

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(data.enumerated()), id: \.element.id) { idx, e in
                if idx > 0 { Divider().padding(.leading, 10) }
                row(e)
                    .padding(.horizontal, 10)
                    .frame(minHeight: 30)
            }
        }
        .padding(.vertical, 2)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
    }
}

struct AreaRow: View {
    var area: OverviewView.Area

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: area.bucket.symbol)
                .foregroundStyle(area.bucket.swiftColor)
                .frame(width: 18)
            Text(area.group)
                .lineLimit(1)
                .frame(width: 190, alignment: .leading)
            Text(area.bucket.name)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .frame(width: 170, alignment: .leading)
            PartBar(part: area.reclaim, whole: area.total, height: 5)
                .frame(minWidth: 80, maxWidth: 240)
            Text("\(area.count) item\(area.count == 1 ? "" : "s")")
                .foregroundStyle(.secondary)
                .frame(width: 64, alignment: .trailing)
            Text(Fmt.bytes(area.reclaim))
                .monospacedDigit()
                .frame(width: 72, alignment: .trailing)
            Text("of \(Fmt.bytes(area.total))")
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(width: 84, alignment: .leading)
            Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
        }
        .font(.callout)
    }
}

/// A one-line item row: checkbox, name, project, status, used, size.
struct DenseItemRow: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    var item: Item

    var body: some View {
        HStack(spacing: 10) {
            SelectBox(item: item)
            Text(item.name)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
                .help(Fmt.tilde(item.path))
            Text(item.project.map { URL(fileURLWithPath: $0).lastPathComponent } ?? item.group)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(width: 170, alignment: .leading)
            TierBadge(tier: item.tier)
                .frame(width: 86, alignment: .leading)
            Text(Fmt.ago(item.lastUsedDate))
                .foregroundStyle(.secondary)
                .frame(width: 64, alignment: .trailing)
            Text(Fmt.bytes(item.total_bytes))
                .monospacedDigit()
                .frame(width: 72, alignment: .trailing)
        }
        .font(.callout)
        .contentShape(Rectangle())
        .contextMenu { ItemContextMenu(item: item) }
    }
}

/// Shared context menu for an item.
struct ItemContextMenu: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    var item: Item

    var body: some View {
        Button(store.selection.contains(item.id) ? "Deselect" : "Select") { store.toggle(item.id) }
            .disabled(!item.isCleanable)
        Button("Review & clean…") { nav.review(items: [item]) }
            .disabled(!item.isCleanable)
        Divider()
        Button("Reveal in Finder") { store.reveal(item) }.disabled(item.isVirtual)
        Button("Copy path") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(item.path, forType: .string)
        }
        Divider()
        AskAgentItems(target: item.id)
    }
}
