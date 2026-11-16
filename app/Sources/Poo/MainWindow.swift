import SwiftUI

struct MainWindow: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        @Bindable var nav = nav
        NavigationSplitView {
            List(selection: $nav.section) {
                ForEach(Array(Navigation.menuSections.enumerated()), id: \.offset) { _, s in
                    HStack {
                        Label(s.title, systemImage: s.symbol)
                        Spacer()
                        if s.section == .elsewhere, let r = store.report, case let b = ElsewhereView.bytes(r, purgeable: store.purgeable), b > 0 {
                            Text(Fmt.bytes(b)).font(.caption).monospacedDigit().foregroundStyle(.secondary)
                        }
                    }
                    .tag(s.section)
                }
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
                        .contextMenu { BucketContextMenu(bucket: b) }
                    }
                }
            }
            .navigationSplitViewColumnWidth(min: 240, ideal: 270)
        } detail: {
            Group {
                switch nav.section ?? .overview {
                case .overview: OverviewView()
                case .browse: BrowseView()
                case .bucket(let id): BrowseView(lockedBucket: id).id(id)
                case .projects: ProjectsView()
                case .worktrees: WorktreesView()
                case .elsewhere: ElsewhereView()
                case .backups: BackupsView()
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if nav.section != .backups { SelectionBar() }
            }
            .overlay(alignment: .bottom) { ToastOverlay().animation(.default, value: store.toast) }
            .onDeleteCommand {
                if nav.section != .backups { nav.review(items: store.selectedItems) }
            }
            .sheet(item: $nav.review) { r in
                CleanConfirmView(planID: r.planID, free: r.free, items: r.items, onShowBackups: {
                    nav.review = nil
                    nav.section = .backups
                })
            }
            .task {
                LaunchFlow.run(openWindow)
                if let p = AppDelegate.shared?.previewSection {
                    AppDelegate.shared?.previewSection = nil
                    switch p {
                    case "browse": nav.section = .browse
                    case "projects": nav.section = .projects
                    case "worktrees": nav.section = .worktrees
                    case "backups": nav.section = .backups
                    case "elsewhere": nav.section = .elsewhere
                    default: if p.hasPrefix("bucket:") { nav.section = .bucket(String(p.dropFirst(7))) }
                    }
                }
                await store.loadLaunchers()
            }
            .toolbar {
                ToolbarItem(placement: .primaryAction) { RefreshToolbarButton() }
            }
        }
    }
}

struct ItemInspector: View {
    @Environment(Store.self) private var store
    @Environment(Navigation.self) private var nav
    var item: Item

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(item.name).font(.headline).textSelection(.enabled)
                    Text(Fmt.tilde(item.path)).font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                    HStack(spacing: 6) {
                        TierBadge(tier: item.tier)
                        Text(item.tier.explanation).font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                HStack(spacing: 8) {
                    Button(store.selection.contains(item.id) ? "Deselect" : "Select") { store.toggle(item.id) }
                        .disabled(!item.isCleanable)
                    Button("Reveal") { store.reveal(item) }.disabled(item.isVirtual)
                    Spacer()
                }
                .controlSize(.small)
                Divider()
                Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 4) {
                    row("Size", Fmt.bytes(item.total_bytes) + (item.total_bytes > item.bytes ? " (incl. nested)" : ""))
                    if item.freesDiffers, let f = item.frees {
                        row("Frees", Fmt.bytes(f) + " (the rest is shared with clones or hard links)")
                    }
                    row("Comes back", item.comesBack)
                    row("Files", item.files.formatted())
                    row("Used", Fmt.ago(item.lastUsedDate))
                    row("Cost", item.cost)
                    if let p = item.project { row("Project", Fmt.tilde(p)) }
                    if let a = item.agent { row("Agent", a) }
                    if let rb = item.rebuild { row("Rebuild", rb) }
                    row("Rule", item.rule)
                }
                .font(.callout)
                if !item.reasons.isEmpty {
                    section("Why") {
                        ForEach(item.reasons, id: \.self) { Text($0).font(.callout).fixedSize(horizontal: false, vertical: true) }
                    }
                }
                section("When cleaned") {
                    ForEach(SafeguardSummary.sentences(for: item), id: \.self) { s in
                        Label(s, systemImage: SafeguardSummary.symbol(for: s, rebuild: item.rebuild))
                            .font(.callout)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                if let risks = item.risks?.filter(\.isWarning), !risks.isEmpty {
                    section("Skipped unless you include it") {
                        ForEach(risks, id: \.tag) { r in
                            Text(r.message).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                if let kids = item.children, !kids.isEmpty {
                    let biggest = kids.map(\.bytes).max() ?? 1
                    section("Inside") {
                        ForEach(kids.prefix(10), id: \.name) { c in
                            HStack(spacing: 8) {
                                Text(c.name).lineLimit(1).truncationMode(.middle).frame(maxWidth: .infinity, alignment: .leading)
                                PartBar(part: c.bytes, whole: biggest, height: 4).frame(width: 60)
                                Text(Fmt.bytes(c.bytes)).monospacedDigit().foregroundStyle(.secondary).frame(width: 64, alignment: .trailing)
                            }
                            .font(.caption)
                        }
                    }
                }
            }
            .padding(14)
        }
    }

    private func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.subheadline.weight(.semibold))
            content()
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
                        ForEach(items) { i in
                            ItemRow(item: i, subtitle: "\(i.group) · \(i.why)")
                                .contextMenu { ItemContextMenu(item: i) }
                        }
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
        .toolbar { ToolbarItem { AskAgentButton(target: "projects", title: "Ask") } }
        .freshSubtitle("\(projects.count) projects")
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
            Text(p.stacks.joined(separator: ", ")).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
                .frame(width: 140, alignment: .trailing)
            Text(statusText(p))
                .lineLimit(1)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 92, alignment: .trailing)
                .help("Last activity \(Fmt.ago(p.last_activity.map { Date(timeIntervalSince1970: TimeInterval($0)) }))")
            // Only a policy someone chose is worth a column; Auto is the quiet default.
            Group {
                if p.policy != "auto" {
                    Label(Self.policyLabel(p.policy), systemImage: p.policy == "protected" ? "lock.fill" : p.policy == "active" ? "pin.fill" : "moon.zzz")
                        .labelStyle(.titleAndIcon)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(width: 92, alignment: .trailing)
            Text(Fmt.bytes(p.artifact_bytes))
                .monospacedDigit()
                .frame(width: 72, alignment: .trailing)
        }
        .padding(.vertical, 2)
        .contextMenu {
            if !notInUse.isEmpty {
                Button("Select \(notInUse.count) Not in Use (\(Fmt.bytes(notInUse.reduce(0) { $0 + $1.total_bytes })))") { store.select(notInUse, true) }
                Divider()
            }
            Picker("Policy", selection: Binding(get: { p.policy }, set: { new in Task { await store.setPolicy(p.path, new) } })) {
                ForEach(["auto", "active", "stale", "protected"], id: \.self) { Text(Self.policyLabel($0)).tag($0) }
            }
            Divider()
            ProjectRow.menu(p.path)
        }
    }

    static func policyLabel(_ p: String) -> String {
        switch p {
        case "auto": return "Auto"
        case "active": return "Keep warm"
        case "stale": return "Treat as unused"
        case "protected": return "Protected"
        default: return p.capitalized
        }
    }

    @ViewBuilder
    static func menu(_ path: String) -> some View {
        Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
        Button("Copy Path") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(path, forType: .string)
        }
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
        .toolbar { ToolbarItem { AskAgentButton(target: "worktrees", title: "Ask") } }
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
            Group {
                if let item { SelectBox(item: item) } else { Color.clear }
            }
            .frame(width: 18)
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
            Group {
                if let item { TierBadge(tier: item.tier) } else { Color.clear }
            }
            .frame(width: 92, height: 16, alignment: .leading)
            Text(Fmt.bytes(item?.total_bytes ?? project.artifact_bytes)).monospacedDigit().frame(width: 80, alignment: .trailing)
        }
        .contextMenu {
            if let item { ItemContextMenu(item: item) } else { ProjectRow.menu(project.path) }
        }
    }
}
