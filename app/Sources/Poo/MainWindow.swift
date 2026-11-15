import Charts
import SwiftUI

struct MainWindow: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav

    var body: some View {
        @Bindable var nav = nav
        NavigationSplitView {
            List(selection: $nav.section) {
                Label("Overview", systemImage: "gauge.with.dots.needle.33percent").tag(Navigation.Section.overview)
                Label("Browse", systemImage: "list.bullet.indent").tag(Navigation.Section.browse)
                Label("Projects", systemImage: "folder").tag(Navigation.Section.projects)
                Label("Worktrees", systemImage: "arrow.triangle.branch").tag(Navigation.Section.worktrees)
                Label("Backups", systemImage: "archivebox").tag(Navigation.Section.backups)
                Section("Buckets") {
                    ForEach(store.report?.visibleBuckets.sorted { $0.bytes > $1.bytes } ?? []) { b in
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
            }
            .navigationSplitViewColumnWidth(min: 220, ideal: 250)
        } detail: {
            Group {
                switch nav.section ?? .overview {
                case .overview: OverviewView()
                case .browse: BrowseView()
                case .bucket(let id): BrowseView(lockedBucket: id).id(id)
                case .projects: ProjectsView()
                case .worktrees: WorktreesView()
                case .backups: BackupsView()
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
                VStack(alignment: .leading, spacing: 4) {
                    Text("When cleaned").font(.subheadline.weight(.semibold))
                    ForEach(SafeguardSummary.sentences(for: item), id: \.self) { s in
                        Label(s, systemImage: "checkmark.shield").font(.callout)
                    }
                }
                if let risks = item.risks?.filter(\.isWarning), !risks.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Skipped unless you include it").font(.subheadline.weight(.semibold))
                        ForEach(risks, id: \.tag) { r in
                            Label(r.message, systemImage: "info.circle").font(.callout).foregroundStyle(.secondary)
                        }
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

    var body: some View {
        let projects = (store.report?.projects ?? []).sorted { $0.artifact_bytes > $1.artifact_bytes }
        let byProject = Dictionary(grouping: store.report?.items.filter { $0.project != nil } ?? [], by: { $0.project! })
        List {
            ForEach(projects) { p in
                let items = (byProject[p.path] ?? []).sorted { $0.total_bytes > $1.total_bytes }
                if items.isEmpty {
                    ProjectRow(project: p, items: items)
                } else {
                    DisclosureGroup {
                        ForEach(items) { i in ItemRow(item: i, subtitle: "\(i.group) · \(i.reasons.first ?? Fmt.tilde(i.path))") }
                    } label: {
                        ProjectRow(project: p, items: items)
                    }
                }
            }
        }
        .listStyle(.inset)
        .overlay {
            if projects.isEmpty { ContentUnavailableView("No projects yet", systemImage: "folder", description: Text("Add your dev folders in Settings.")) }
        }
        .navigationTitle("Projects")
        .navigationSubtitle("\(projects.count) projects")
        .safeAreaInset(edge: .bottom, spacing: 0) { SelectionBar() }
    }
}

struct ProjectRow: View {
    @Environment(Store.self) private var store
    var project: Project
    var items: [Item]

    var body: some View {
        let p = project
        let notInUse = items.filter { $0.status == .notInUse && $0.isCleanable }
        HStack(spacing: 10) {
            Image(systemName: p.kind == "worktree" ? "arrow.triangle.branch" : "folder")
                .foregroundStyle(.secondary)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(p.name).fontWeight(.medium)
                    if let a = p.agent {
                        Text(a).font(.caption2).padding(.horizontal, 4).background(.quaternary, in: Capsule())
                    }
                }
                Text(Fmt.tilde(p.path)).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if !p.stacks.isEmpty {
                Text(p.stacks.joined(separator: ", ")).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Text(statusText(p))
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 92, alignment: .trailing)
                .help("Last activity \(Fmt.ago(p.last_activity.map { Date(timeIntervalSince1970: TimeInterval($0)) }))")
            if !notInUse.isEmpty {
                Button { store.select(notInUse, true) } label: { Image(systemName: "checklist") }
                    .buttonStyle(.borderless)
                    .help("Select the \(notInUse.count) items not in use (\(Fmt.bytes(notInUse.reduce(0) { $0 + $1.total_bytes })))")
            }
            Picker("", selection: Binding(get: { p.policy }, set: { new in Task { await store.setPolicy(p.path, new) } })) {
                Text("Auto").tag("auto")
                Text("Active").tag("active")
                Text("Stale").tag("stale")
                Text("Protected").tag("protected")
            }
            .labelsHidden()
            .controlSize(.small)
            .frame(width: 100)
            .help("Auto lets Poo decide from activity; Protected keeps everything here")
            Text(Fmt.bytes(p.artifact_bytes))
                .monospacedDigit()
                .frame(width: 72, alignment: .trailing)
        }
        .padding(.vertical, 2)
    }

    private func statusText(_ p: Project) -> String {
        switch p.status {
        case "active": return "Active"
        case "idle": return "Idle"
        case "stale": return "Not touched lately"
        default: return p.status.capitalized
        }
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
        .safeAreaInset(edge: .bottom, spacing: 0) { SelectionBar() }
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
