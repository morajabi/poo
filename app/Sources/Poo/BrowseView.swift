import SwiftUI

/// Every item, grouped and sorted, with checkboxes for cleaning by hand.
struct BrowseView: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    /// Set when opened from a bucket in the sidebar.
    var lockedBucket: String?
    @State private var inspected: Item.ID?
    @State private var showInspector = false
    @State private var collapsed: Set<String> = []

    struct ItemGroup: Identifiable {
        var id: String
        var title: String
        var subtitle: String?
        var symbol: String?
        var color: Color?
        var items: [Item]
        var bytes: UInt64
        var notInUse: [Item]
    }

    var body: some View {
        @Bindable var nav = nav
        let filter = nav.browse
        let report = store.report
        let bucket = (lockedBucket ?? filter.bucket).flatMap { id in report?.buckets.first { $0.id == id } }
        let groups = Self.groups(report: report, filter: filter, bucket: lockedBucket ?? filter.bucket)
        let byID = inspected.flatMap { id in report?.items.first { $0.id == id } }

        List(selection: $inspected) {
            ForEach(groups) { g in
                Section {
                    if !collapsed.contains(g.id) {
                        ForEach(g.items) { i in
                            ItemRow(item: i, subtitle: subtitle(i, filter.grouping))
                                .tag(i.id as String?)
                        }
                    }
                } header: {
                    header(g, grouped: filter.grouping != .none)
                }
            }
        }
        .listStyle(.inset)
        .overlay {
            if report == nil {
                ContentUnavailableView("Scanning…", systemImage: "internaldrive")
            } else if groups.isEmpty {
                ContentUnavailableView.search(text: filter.query)
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) { filterBar(bucket: bucket) }
        .safeAreaInset(edge: .bottom, spacing: 0) { SelectionBar() }
        .searchable(text: $nav.browse.query, placement: .toolbar, prompt: "Name or path")
        .navigationTitle(lockedBucket != nil ? (bucket?.name ?? "Bucket") : "Browse")
        .navigationSubtitle(subtitle(groups))
        .toolbar {
            ToolbarItemGroup(placement: .automatic) {
                Picker("Group by", selection: $nav.browse.grouping) {
                    ForEach(BrowseFilter.Grouping.allCases) { Text($0.label).tag($0) }
                }
                .help("Group by")
                Picker("Sort by", selection: $nav.browse.sorting) {
                    ForEach(BrowseFilter.Sorting.allCases) { Text($0.label).tag($0) }
                }
                .help("Sort by")
                Button { showInspector.toggle() } label: { Label("Inspector", systemImage: "sidebar.right") }
                    .help("Show the selected item's details")
            }
        }
        .inspector(isPresented: $showInspector) {
            Group {
                if let i = byID {
                    ItemInspector(item: i)
                } else {
                    ContentUnavailableView("No item selected", systemImage: "sidebar.right", description: Text("Select a row to see where it is, why Poo rated it so, and what cleaning does."))
                }
            }
            .inspectorColumnWidth(min: 260, ideal: 320, max: 440)
        }
        .onChange(of: inspected) { _, new in if new != nil { showInspector = true } }
    }

    private func subtitle(_ groups: [ItemGroup]) -> String {
        let n = groups.reduce(0) { $0 + $1.items.count }
        let b = groups.reduce(UInt64(0)) { $0 + $1.bytes }
        return "\(n) items · \(Fmt.bytes(b))"
    }

    private func subtitle(_ i: Item, _ g: BrowseFilter.Grouping) -> String {
        switch g {
        case .project: return "\(i.group) · \(i.reasons.first ?? Fmt.tilde(i.path))"
        default:
            let why = i.reasons.first ?? Fmt.tilde(i.path)
            if let p = i.project {
                let name = URL(fileURLWithPath: p).lastPathComponent
                if !i.name.hasPrefix(name) { return "\(name) · \(why)" }
            }
            return why
        }
    }

    private func header(_ g: ItemGroup, grouped: Bool) -> some View {
        let isCollapsed = collapsed.contains(g.id)
        return HStack(spacing: 8) {
            if grouped {
                Button {
                    if isCollapsed { collapsed.remove(g.id) } else { collapsed.insert(g.id) }
                } label: {
                    Image(systemName: "chevron.right")
                        .rotationEffect(.degrees(isCollapsed ? 0 : 90))
                        .frame(width: 12)
                }
                .buttonStyle(.borderless)
            }
            if let sym = g.symbol {
                Image(systemName: sym).foregroundStyle(g.color ?? .secondary)
            }
            Text(g.title).font(.headline).foregroundStyle(.primary)
            if let s = g.subtitle { Text(s).foregroundStyle(.secondary) }
            Text("\(g.items.count)").foregroundStyle(.tertiary)
            Spacer()
            if !g.notInUse.isEmpty {
                Button("Select not in use") { store.select(g.notInUse, true) }
                    .buttonStyle(.borderless)
                    .font(.callout)
                    .help("Tick the \(g.notInUse.count) items nothing is using (\(Fmt.bytes(g.notInUse.reduce(0) { $0 + $1.total_bytes })))")
            }
            Text(Fmt.bytes(g.bytes)).font(.headline).monospacedDigit().foregroundStyle(.primary)
        }
        .contentShape(Rectangle())
        .onTapGesture(count: 2) {
            if isCollapsed { collapsed.remove(g.id) } else { collapsed.insert(g.id) }
        }
    }

    private func filterBar(bucket: BucketSummary?) -> some View {
        @Bindable var nav = nav
        return HStack(spacing: 6) {
            ForEach(ItemStatus.allCases) { s in
                Toggle(isOn: Binding(
                    get: { nav.browse.statuses.contains(s) },
                    set: { on in if on { nav.browse.statuses.insert(s) } else { nav.browse.statuses.remove(s) } }
                )) {
                    Label(s.label, systemImage: s.symbol)
                }
                .toggleStyle(.button)
                .controlSize(.small)
                .help(statusHelp(s))
            }
            Divider().frame(height: 16)
            if lockedBucket == nil {
                Picker("Bucket", selection: Binding(
                    get: { nav.browse.bucket },
                    set: { nav.browse.bucket = $0; nav.browse.group = nil }
                )) {
                    Text("All buckets").tag(String?.none)
                    Divider()
                    ForEach(store.report?.visibleBuckets ?? []) { b in Text(b.name).tag(String?.some(b.id)) }
                }
                .labelsHidden()
                .controlSize(.small)
                .fixedSize()
            } else if let bucket {
                Text(bucket.description).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            if let g = nav.browse.group {
                Button { nav.browse.group = nil } label: {
                    Label(g, systemImage: "xmark.circle.fill").labelStyle(.titleAndIcon)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .help("Show every group")
            }
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }

    private func statusHelp(_ s: ItemStatus) -> String {
        switch s {
        case .notInUse: return "Junk, Unused and Idle: nothing is using these right now"
        case .inUse: return "In use and Active now: you are working with these"
        case .needsLook: return "Check first and Kept: Poo won't clean these on its own"
        }
    }

    // MARK: Grouping

    static func groups(report: Report?, filter: BrowseFilter, bucket: String?) -> [ItemGroup] {
        guard let r = report else { return [] }
        let buckets = Dictionary(uniqueKeysWithValues: r.buckets.map { ($0.id, $0) })
        let q = filter.query.trimmingCharacters(in: .whitespaces)
        let items = r.items.filter { i in
            if let bucket, i.bucket != bucket { return false }
            if bucket == nil, buckets[i.bucket]?.hidden == true { return false }
            if let g = filter.group, i.group != g { return false }
            if !filter.statuses.isEmpty, !filter.statuses.contains(i.status) { return false }
            if !q.isEmpty, !(i.name.localizedCaseInsensitiveContains(q) || i.path.localizedCaseInsensitiveContains(q)) { return false }
            return true
        }
        let sorted = sort(items, filter.sorting)

        func make(_ id: String, _ title: String, _ sub: String?, _ sym: String?, _ color: Color?, _ its: [Item]) -> ItemGroup {
            ItemGroup(id: id, title: title, subtitle: sub, symbol: sym, color: color, items: its,
                  // Disjoint bytes, so nested items (inside worktrees) are not counted twice.
                  bytes: its.reduce(0) { $0 + $1.bytes },
                  notInUse: its.filter { $0.status == .notInUse && $0.isCleanable })
        }

        var out: [ItemGroup]
        switch filter.grouping {
        case .none:
            return sorted.isEmpty ? [] : [make("all", "All items", nil, nil, nil, sorted)]
        case .type:
            var order: [String] = []
            var acc: [String: [Item]] = [:]
            for i in sorted {
                let k = i.bucket + "/" + i.group
                if acc[k] == nil { order.append(k) }
                acc[k, default: []].append(i)
            }
            out = order.map { k in
                let its = acc[k]!
                let b = buckets[its[0].bucket]
                return make(k, its[0].group, b?.name, b?.symbol, b.map(\.swiftColor), its)
            }
        case .project:
            var order: [String] = []
            var acc: [String: [Item]] = [:]
            for i in sorted {
                let k = i.project ?? ""
                if acc[k] == nil { order.append(k) }
                acc[k, default: []].append(i)
            }
            out = order.map { k in
                k.isEmpty
                    ? make("~none", "Not in a project", "caches, tools and system", "tray.2", nil, acc[k]!)
                    : make(k, URL(fileURLWithPath: k).lastPathComponent, Fmt.tilde(k), "folder", nil, acc[k]!)
            }
        case .status:
            out = ItemStatus.allCases.compactMap { s in
                let its = sorted.filter { $0.status == s }
                return its.isEmpty ? nil : make(s.rawValue, s.label, nil, s.symbol, nil, its)
            }
            return out
        }
        switch filter.sorting {
        case .name: return out.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
        case .lastUsed:
            func newest(_ g: ItemGroup) -> Int64 { g.items.compactMap(\.last_used).max() ?? 0 }
            return out.sorted { newest($0) > newest($1) }
        case .size: return out.sorted { $0.bytes > $1.bytes }
        }
    }

    static func sort(_ items: [Item], _ s: BrowseFilter.Sorting) -> [Item] {
        switch s {
        case .size: return items.sorted { $0.total_bytes > $1.total_bytes }
        case .lastUsed: return items.sorted { ($0.last_used ?? 0) > ($1.last_used ?? 0) }
        case .name: return items.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        }
    }
}

/// Bottom bar for whatever is ticked, shared by Browse, Projects and Worktrees.
struct SelectionBar: View {
    @Environment(Store.self) private var store
    @State private var confirming = false

    var body: some View {
        let items = store.selection.isEmpty ? [] : (store.report?.items.filter { store.selection.contains($0.id) } ?? [])
        if !items.isEmpty {
            HStack {
                Text("\(items.count) selected").foregroundStyle(.secondary)
                Button("Deselect all") { store.selection.removeAll() }
                    .buttonStyle(.link)
                Spacer()
                Button("Clean \(items.count) selected (\(Fmt.bytes(items.reduce(0) { $0 + $1.total_bytes })))…") { confirming = true }
                    .buttonStyle(.borderedProminent)
                    .disabled(store.busy)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(.bar)
            .overlay(alignment: .top) { Divider() }
            .sheet(isPresented: $confirming) {
                CleanConfirmView(planID: nil, free: nil, items: items)
            }
        }
    }
}
