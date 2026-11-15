import Charts
import SwiftUI

struct MainWindow: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav

    var body: some View {
        @Bindable var nav = nav
        NavigationSplitView {
            List(selection: $nav.section) {
                Label("Overview", systemImage: "chart.pie").tag(Navigation.Section.overview)
                Section("Buckets") {
                    ForEach(store.report?.visibleBuckets ?? []) { b in
                        HStack {
                            Label(b.name, systemImage: b.symbol)
                                .foregroundStyle(.primary)
                            Spacer()
                            Text(Fmt.bytes(b.bytes)).font(.caption).monospacedDigit().foregroundStyle(.secondary)
                        }
                        .tag(Navigation.Section.bucket(b.id))
                        .listItemTint(b.swiftColor)
                    }
                }
                Section("Workflows") {
                    Label("Projects", systemImage: "folder").tag(Navigation.Section.projects)
                    Label("Worktrees", systemImage: "arrow.triangle.branch").tag(Navigation.Section.worktrees)
                }
            }
            .navigationSplitViewColumnWidth(min: 220, ideal: 250)
        } detail: {
            Group {
                switch nav.section ?? .overview {
                case .overview: OverviewView()
                case .bucket(let id): BucketView(bucketID: id)
                case .projects: ProjectsView()
                case .worktrees: WorktreesView()
                case .history: OverviewView()
                }
            }
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    if store.scanning {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            if let p = store.progress {
                                Text("\(p.phase) · \(Fmt.bytes(p.bytes))").font(.caption).monospacedDigit()
                            }
                        }
                    } else {
                        Button { Task { await store.scan() } } label: { Label("Rescan", systemImage: "arrow.clockwise") }
                    }
                }
            }
        }
    }
}

struct OverviewView: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    @State private var confirming: PlanSummary?
    @State private var emergencyGB: Double = 20

    var body: some View {
        ScrollView {
            if let r = store.report {
                VStack(alignment: .leading, spacing: 24) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Disk").font(.title2.weight(.semibold))
                        DiskBar(report: r)
                        if let d = r.detected, !d.isEmpty {
                            HStack(spacing: 6) {
                                Text("Found on this Mac").font(.caption).foregroundStyle(.secondary)
                                ForEach(d, id: \.self) { s in
                                    Text(s).font(.caption).padding(.horizontal, 6).padding(.vertical, 2).background(.quaternary, in: Capsule())
                                }
                            }
                        }
                        if let note = store.lastClean?.snapshot_note {
                            Label(note, systemImage: "clock.arrow.circlepath").font(.caption).foregroundStyle(.orange)
                        }
                    }
                    HStack(alignment: .top, spacing: 28) {
                        BucketDonut(buckets: r.visibleBuckets, total: r.total_bytes) { b in nav.section = .bucket(b.id) }
                            .frame(width: 220, height: 220)
                        VStack(alignment: .leading, spacing: 8) {
                            Text("By bucket and tier").font(.headline)
                            TierStackChart(buckets: r.visibleBuckets.sorted { $0.bytes > $1.bytes })
                        }
                    }
                    VStack(alignment: .leading, spacing: 10) {
                        Text("Recommended plans").font(.title2.weight(.semibold))
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 240), spacing: 12)], spacing: 12) {
                            ForEach(r.plans) { p in planCard(p) }
                        }
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Biggest items").font(.title2.weight(.semibold))
                        ForEach(r.items.prefix(12)) { i in
                            ItemRow(item: i, selected: store.selection.contains(i.id)) { toggle(i) }
                            Divider()
                        }
                    }
                    if !r.warnings.isEmpty {
                        VStack(alignment: .leading) {
                            ForEach(r.warnings, id: \.self) { Text($0).font(.caption).foregroundStyle(.secondary) }
                        }
                    }
                }
                .padding(24)
            } else {
                ContentUnavailableView("Scanning…", systemImage: "internaldrive")
                    .frame(maxWidth: .infinity, minHeight: 400)
            }
        }
        .navigationTitle("Overview")
        .sheet(item: $confirming) { p in
            CleanConfirmView(planID: p.id, free: p.id == "emergency" ? UInt64(emergencyGB * 1e9) : nil)
        }
        .safeAreaInset(edge: .bottom) { SelectionBar() }
    }

    private func toggle(_ i: Item) {
        if store.selection.contains(i.id) { store.selection.remove(i.id) } else { store.selection.insert(i.id) }
    }

    private func planCard(_ p: PlanSummary) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(p.name).font(.headline)
                Spacer()
                if p.id == "emergency" {
                    Stepper(value: $emergencyGB, in: 5...500, step: 5) { Text("\(Int(emergencyGB)) GB").monospacedDigit() }
                        .fixedSize()
                } else {
                    Text(Fmt.bytes(p.bytes)).font(.title3.weight(.semibold)).monospacedDigit()
                }
            }
            Text(p.description).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            HStack {
                Text("\(p.count) items").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Review & clean…") { confirming = p }
                    .disabled(store.busy || (p.id != "emergency" && p.count == 0))
            }
        }
        .padding(14)
        .frame(minHeight: 130)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10))
    }
}

struct SelectionBar: View {
    @Environment(Store.self) private var store
    @State private var confirming = false

    var body: some View {
        let items = store.report?.items.filter { store.selection.contains($0.id) } ?? []
        if !items.isEmpty {
            HStack {
                Text("\(items.count) selected · \(Fmt.bytes(items.reduce(0) { $0 + $1.total_bytes }))")
                Spacer()
                Button("Clear") { store.selection.removeAll() }
                Button("Clean selected…") { confirming = true }
                    .buttonStyle(.borderedProminent)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
            .background(.bar)
            .sheet(isPresented: $confirming) {
                CleanConfirmView(planID: nil, free: nil, items: items)
            }
        }
    }
}

struct BucketView: View {
    @Environment(Store.self) private var store
    var bucketID: String
    @State private var tierFilter: Tier?
    @State private var inspected: Item?
    @State private var query = ""

    var body: some View {
        let bucket = store.report?.buckets.first { $0.id == bucketID }
        let items = store.items(in: bucketID).filter { i in
            (tierFilter == nil || i.tier == tierFilter) &&
                (query.isEmpty || i.name.localizedCaseInsensitiveContains(query) || i.path.localizedCaseInsensitiveContains(query))
        }
        let groups = Dictionary(grouping: items, by: \.group)
            .map { (name: $0.key, items: $0.value.sorted { $0.total_bytes > $1.total_bytes }) }
            .sorted { a, b in a.items.reduce(0) { $0 + $1.bytes } > b.items.reduce(0) { $0 + $1.bytes } }

        HSplitView {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if let bucket {
                        HStack(alignment: .firstTextBaseline) {
                            Label(bucket.name, systemImage: bucket.symbol).font(.title2.weight(.semibold)).foregroundStyle(bucket.swiftColor)
                            Spacer()
                            Text(Fmt.bytes(bucket.bytes)).font(.title2.weight(.semibold)).monospacedDigit()
                        }
                        Text(bucket.description).foregroundStyle(.secondary)
                        GroupBarChart(groups: bucket.groups, color: bucket.swiftColor)
                    }
                    HStack {
                        Picker("Tier", selection: $tierFilter) {
                            Text("All tiers").tag(Tier?.none)
                            ForEach(Tier.allCases) { t in Text(t.label).tag(Tier?.some(t)) }
                        }
                        .fixedSize()
                        TextField("Filter", text: $query).textFieldStyle(.roundedBorder).frame(maxWidth: 220)
                        Spacer()
                        Button("Select stale") {
                            for i in items where i.tier == .junk || i.tier == .cold { store.selection.insert(i.id) }
                        }
                    }
                    ForEach(groups, id: \.name) { g in
                        DisclosureGroup {
                            ForEach(g.items) { i in
                                ItemRow(item: i, selected: store.selection.contains(i.id)) {
                                    if store.selection.contains(i.id) { store.selection.remove(i.id) } else { store.selection.insert(i.id) }
                                }
                                .contentShape(Rectangle())
                                .onTapGesture { inspected = i }
                                .background(inspected?.id == i.id ? Color.accentColor.opacity(0.12) : .clear, in: RoundedRectangle(cornerRadius: 6))
                                Divider()
                            }
                        } label: {
                            HStack {
                                Text(g.name).font(.headline)
                                Text("\(g.items.count)").foregroundStyle(.secondary)
                                Spacer()
                                Text(Fmt.bytes(g.items.reduce(0) { $0 + $1.bytes })).monospacedDigit()
                            }
                        }
                    }
                }
                .padding(20)
            }
            .frame(minWidth: 520)
            if let i = inspected {
                ItemInspector(item: i)
                    .frame(minWidth: 280, idealWidth: 320, maxWidth: 420)
            }
        }
        .navigationTitle(bucket?.name ?? "Bucket")
        .safeAreaInset(edge: .bottom) { SelectionBar() }
    }
}

struct ItemInspector: View {
    @Environment(Store.self) private var store
    var item: Item

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text(item.name).font(.headline).textSelection(.enabled)
                HStack {
                    TierBadge(tier: item.tier)
                    Text(item.tier.explanation).font(.caption).foregroundStyle(.secondary)
                }
                Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 6) {
                    row("Size", Fmt.bytes(item.total_bytes) + (item.total_bytes > item.bytes ? " (incl. nested)" : ""))
                    row("Files", "\(item.files)")
                    row("Used", Fmt.ago(item.lastUsedDate))
                    row("Cost", item.cost)
                    row("Rule", item.rule)
                    if let p = item.project { row("Project", Fmt.tilde(p)) }
                    if let a = item.agent { row("Agent", a) }
                    if let rb = item.rebuild { row("Rebuild", rb) }
                    row("Clean", actionText)
                }
                .font(.callout)
                Text(Fmt.tilde(item.path)).font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                if !item.reasons.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Why").font(.subheadline.weight(.semibold))
                        ForEach(item.reasons, id: \.self) { Text("• \($0)").font(.callout) }
                    }
                }
                if let kids = item.children, !kids.isEmpty {
                    Text("Inside").font(.subheadline.weight(.semibold))
                    Chart(kids.prefix(10), id: \.name) { c in
                        BarMark(x: .value("MB", Double(c.bytes) / 1e6), y: .value("Name", c.name))
                            .annotation(position: .trailing) { Text(Fmt.bytes(c.bytes)).font(.caption2).foregroundStyle(.secondary) }
                    }
                    .chartXAxis(.hidden)
                    .frame(height: CGFloat(min(kids.count, 10)) * 24 + 10)
                }
                HStack {
                    Button("Reveal in Finder") { store.reveal(item) }.disabled(item.isVirtual)
                    Spacer()
                    Button(store.selection.contains(item.id) ? "Deselect" : "Select") {
                        if store.selection.contains(item.id) { store.selection.remove(item.id) } else { store.selection.insert(item.id) }
                    }
                    .disabled(item.action.kind == "none")
                }
            }
            .padding(16)
        }
    }

    private var actionText: String {
        switch item.action.kind {
        case "command": return (item.action.command ?? []).joined(separator: " ")
        case "worktree-remove": return "git worktree remove"
        case "trash": return "Move to Trash"
        case "none": return "Manage it yourself"
        default: return "Delete"
        }
    }

    private func row(_ k: String, _ v: String) -> some View {
        GridRow {
            Text(k).foregroundStyle(.secondary)
            Text(v).textSelection(.enabled).lineLimit(3)
        }
    }
}

struct ProjectsView: View {
    @Environment(Store.self) private var store
    @State private var sortOrder = [KeyPathComparator(\Project.artifact_bytes, order: .reverse)]

    var body: some View {
        let projects = (store.report?.projects ?? []).sorted(using: sortOrder)
        Table(projects, sortOrder: $sortOrder) {
            TableColumn("Project", value: \.name) { p in
                VStack(alignment: .leading) {
                    Text(p.name)
                    Text(Fmt.tilde(p.path)).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
            }
            TableColumn("Size", value: \.artifact_bytes) { p in Text(Fmt.bytes(p.artifact_bytes)).monospacedDigit() }
                .width(80)
            TableColumn("Kind", value: \.kind) { p in Text(p.agent.map { "\(p.kind) · \($0)" } ?? p.kind) }
                .width(110)
            TableColumn("Status", value: \.status) { p in Text(p.status) }
                .width(60)
            TableColumn("Active") { p in Text(Fmt.ago(p.last_activity.map { Date(timeIntervalSince1970: TimeInterval($0)) })) }
                .width(80)
            TableColumn("Stacks") { p in Text(p.stacks.joined(separator: ", ")).foregroundStyle(.secondary) }
            TableColumn("Policy") { p in
                Picker("", selection: Binding(get: { p.policy }, set: { new in Task { await store.setPolicy(p.path, new) } })) {
                    Text("Auto").tag("auto")
                    Text("Active").tag("active")
                    Text("Stale").tag("stale")
                    Text("Protected").tag("protected")
                }
                .labelsHidden()
            }
            .width(110)
        }
        .navigationTitle("Projects")
    }
}

struct WorktreesView: View {
    @Environment(Store.self) private var store

    private struct RepoGroup: Identifiable {
        var id: String
        var checkouts: [Project]
        var bytes: UInt64
    }

    private var groups: [RepoGroup] {
        let projects = store.report?.projects.filter { $0.kind == "worktree" || $0.kind == "repo" } ?? []
        let byRepo = Dictionary(grouping: projects, by: { $0.repo ?? $0.path })
        return byRepo
            .filter { $0.value.count > 1 }
            .map { key, value in
                RepoGroup(id: key, checkouts: value.sorted { $0.artifact_bytes > $1.artifact_bytes }, bytes: value.reduce(0) { $0 + $1.artifact_bytes })
            }
            .sorted { $0.bytes > $1.bytes }
    }

    var body: some View {
        let wtItems = store.report?.items.filter(\.isWorktree) ?? []
        List {
            ForEach(groups) { g in
                Section {
                    ForEach(g.checkouts) { p in
                        WorktreeRow(project: p, item: wtItems.first { $0.path == p.path })
                    }
                } header: {
                    HStack {
                        Text(Fmt.tilde(g.id)).font(.headline)
                        Spacer()
                        Text("\(Fmt.bytes(g.bytes)) across \(g.checkouts.count) checkouts")
                    }
                }
            }
        }
        .navigationTitle("Worktrees")
        .safeAreaInset(edge: .bottom) { SelectionBar() }
    }
}

struct WorktreeRow: View {
    @Environment(Store.self) private var store
    var project: Project
    var item: Item?

    private var title: String {
        project.kind == "repo" ? "\(project.name) (main checkout)" : (item?.name ?? project.name)
    }

    private var subtitle: String {
        var parts: [String] = [project.branch ?? "detached"]
        if project.dirty == true { parts.append("uncommitted changes") }
        if project.merged == true { parts.append("merged") }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        HStack {
            Image(systemName: project.kind == "repo" ? "house" : "arrow.triangle.branch").foregroundStyle(.secondary)
            VStack(alignment: .leading) {
                HStack {
                    Text(title)
                    if let a = project.agent {
                        Text(a).font(.caption2).padding(.horizontal, 4).background(.quaternary, in: Capsule())
                    }
                }
                Text(subtitle).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if let item { TierBadge(tier: item.tier) }
            Text(Fmt.bytes(item?.total_bytes ?? project.artifact_bytes)).monospacedDigit().frame(width: 80, alignment: .trailing)
            if let item {
                Toggle("", isOn: Binding(
                    get: { store.selection.contains(item.id) },
                    set: { on in
                        if on { store.selection.insert(item.id) } else { store.selection.remove(item.id) }
                    }
                ))
                .toggleStyle(.checkbox)
                .labelsHidden()
                .disabled(item.action.kind == "none")
            }
        }
    }
}
