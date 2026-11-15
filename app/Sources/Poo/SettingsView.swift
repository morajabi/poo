import AppKit
import ServiceManagement
import SwiftUI

struct SettingsView: View {
    @Environment(Store.self) private var store
    @AppStorage("settingsTab") private var tab = "general"

    var body: some View {
        TabView(selection: $tab) {
            GeneralSettings().tabItem { Label("General", systemImage: "gearshape") }.tag("general")
            MenuBarSettingsView().tabItem { Label("Menu Bar", systemImage: "menubar.rectangle") }.tag("menubar")
            SafetySettings().tabItem { Label("Safety & backups", systemImage: "checkmark.shield") }.tag("safety")
            AutomationSettings().tabItem { Label("Automation", systemImage: "clock.arrow.2.circlepath") }.tag("automation")
            BucketSettings().tabItem { Label("Buckets", systemImage: "square.stack.3d.up") }.tag("buckets")
            RuleSettings().tabItem { Label("Rules", systemImage: "list.bullet.rectangle") }.tag("rules")
            AgentSettings().tabItem { Label("Agents", systemImage: "sparkles") }.tag("agents")
            ModSettings().tabItem { Label("Mods", systemImage: "puzzlepiece.extension") }.tag("mods")
        }
        .task { await store.loadSettings() }
    }
}

struct GeneralSettings: View {
    @Environment(Store.self) private var store
    @Environment(Updater.self) private var updater
    @AppStorage("showInMenuBar") private var showInMenuBar = true

    var body: some View {
        Form {
            Section("Dev folders") {
                if let roots = store.roots {
                    if roots.detected {
                        Text("Detected automatically. Add or remove one to pin the list.").font(.caption).foregroundStyle(.secondary)
                    }
                    ForEach(roots.roots, id: \.self) { r in
                        HStack {
                            Image(systemName: "folder")
                            Text(Fmt.tilde(r))
                            Spacer()
                            Button("Remove") { Task { await store.configure(["roots", "rm", r], rescan: true) } }
                                .buttonStyle(.borderless)
                        }
                    }
                }
                Button("Add folder…") { pickFolder() }
            }
            Section("Updates") {
                @Bindable var updater = updater
                HStack {
                    Text("Poo \(updater.currentVersion)")
                    Spacer()
                    Button(updater.available.map { "Install \($0)…" } ?? "Check for Updates…") { updater.checkForUpdates() }
                        .disabled(!updater.enabled || !updater.canCheck)
                }
                Toggle("Check for updates automatically", isOn: $updater.automaticallyChecks)
                    .disabled(!updater.enabled)
                Toggle("Download and install updates automatically", isOn: $updater.automaticallyDownloads)
                    .disabled(!updater.enabled || !updater.automaticallyChecks)
                Text("Updates the app and its bundled poo CLI together.").font(.caption).foregroundStyle(.secondary)
                if let s = updater.status { Text(s).font(.caption).foregroundStyle(.secondary) }
            }
            Section("App") {
                Toggle("Open at login", isOn: Binding(
                    get: { SMAppService.mainApp.status == .enabled },
                    set: { on in try? on ? SMAppService.mainApp.register() : SMAppService.mainApp.unregister() }
                ))
                .disabled(!Onboarding.isBundled)
                Button("Run setup again…") { AppDelegate.shared?.show("onboarding", title: "Set Up Poo") }
            }
            Section("Data") {
                Button("Rescan now") { Task { await store.scan() } }
                Button("Open config folder") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".config/poo"))
                }
            }
        }
        .formStyle(.grouped)
    }

    private func pickFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        if panel.runModal() == .OK {
            Task {
                for url in panel.urls { await store.configure(["roots", "add", url.path]) }
                await store.scan()
            }
        }
    }
}

struct BucketSettings: View {
    @Environment(Store.self) private var store
    @State private var newID = ""
    @State private var newName = ""

    var body: some View {
        Form {
            Section {
                ForEach(store.catalog?.buckets ?? []) { b in BucketRow(bucket: b) }
            } header: {
                Text("Buckets group items in the menu bar, the window, the CLI and MCP. Changes are saved to config.toml.")
                    .font(.caption)
            }
            Section("New bucket") {
                HStack {
                    TextField("id", text: $newID).frame(width: 120)
                    TextField("Name", text: $newName)
                    Button("Add") {
                        let id = newID.lowercased().replacingOccurrences(of: " ", with: "-")
                        Task { await store.configure(["buckets", "add", id, "--name", newName.isEmpty ? newID : newName]) }
                        newID = ""; newName = ""
                    }
                    .disabled(newID.isEmpty)
                }
                Text("Move rules into it from the Rules tab.").font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

struct BucketRow: View {
    @Environment(Store.self) private var store
    var bucket: CatalogBucket
    @State private var name = ""
    @State private var color = Color.gray

    var body: some View {
        HStack {
            Image(systemName: bucket.symbol).foregroundStyle(Color(hex: bucket.color)).frame(width: 20)
            TextField("Name", text: $name)
                .onSubmit { Task { await store.configure(["buckets", "set", bucket.id, "--name", name], rescan: true) } }
            ColorPicker("", selection: $color, supportsOpacity: false)
                .labelsHidden()
                .onChange(of: color) { _, c in
                    guard let hex = c.hexString, hex.lowercased() != bucket.color.lowercased() else { return }
                    Task { await store.configure(["buckets", "set", bucket.id, "--color", hex], rescan: true) }
                }
            Toggle("Hidden", isOn: Binding(get: { bucket.hidden }, set: { h in
                Task { await store.configure(["buckets", "set", bucket.id, "--hidden", h ? "true" : "false"], rescan: true) }
            }))
            .toggleStyle(.checkbox)
            Text(bucket.id).font(.caption.monospaced()).foregroundStyle(.secondary).frame(width: 90, alignment: .trailing)
        }
        .onAppear {
            name = bucket.name
            color = Color(hex: bucket.color)
        }
    }
}

struct RuleSettings: View {
    @Environment(Store.self) private var store
    @State private var query = ""
    @State private var scope = "mac"

    /// A rule is relevant when it found something here or belongs to a stack this Mac has.
    private func relevant(_ r: CatalogRule) -> Bool {
        if store.report?.rule_hits?[r.id] != nil { return true }
        let stacks = r.stacks ?? []
        return stacks.isEmpty || !store.detected.isDisjoint(with: stacks) || stacks.contains("macos")
    }

    private var packs: [(tag: String, total: Int, enabled: Int)] {
        var counts: [String: (Int, Int)] = [:]
        for r in store.catalog?.rules ?? [] where scope == "all" || relevant(r) {
            for t in r.tags ?? [] {
                var c = counts[t] ?? (0, 0)
                c.0 += 1
                if r.enabled { c.1 += 1 }
                counts[t] = c
            }
        }
        return counts.map { (tag: $0.key, total: $0.value.0, enabled: $0.value.1) }.sorted { $0.total > $1.total }
    }

    var body: some View {
        let rules = (store.catalog?.rules ?? []).filter { r in
            (scope == "all" || relevant(r)) &&
                (query.isEmpty || r.id.localizedCaseInsensitiveContains(query) || r.name.localizedCaseInsensitiveContains(query) || (r.tags ?? []).contains { $0.localizedCaseInsensitiveContains(query) })
        }
        .sorted { a, b in
            let ha = store.report?.rule_hits?[a.id]?.last ?? 0, hb = store.report?.rule_hits?[b.id]?.last ?? 0
            return ha != hb ? ha > hb : a.id < b.id
        }
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Picker("", selection: $scope) {
                    Text("On this Mac").tag("mac")
                    Text("All rules").tag("all")
                }
                .pickerStyle(.segmented)
                .fixedSize()
                TextField("Search rules or tags", text: $query).textFieldStyle(.roundedBorder)
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(packs.prefix(30), id: \.tag) { p in
                        Toggle(isOn: Binding(get: { p.enabled > 0 }, set: { on in
                            Task { await store.configure(["rules", on ? "enable" : "disable", "--tag", p.tag], rescan: true) }
                        })) {
                            Text("\(p.tag) \(p.enabled)/\(p.total)").font(.caption)
                        }
                        .toggleStyle(.button)
                        .controlSize(.small)
                        .help("Turn every rule tagged \(p.tag) on or off")
                    }
                }
            }
            List(rules) { r in
                HStack(alignment: .top) {
                    Toggle("", isOn: Binding(get: { r.enabled }, set: { on in
                        Task { await store.configure(["rules", on ? "enable" : "disable", r.id], rescan: true) }
                    }))
                    .labelsHidden()
                    VStack(alignment: .leading, spacing: 2) {
                        HStack {
                            Text(r.name).font(.callout.weight(.medium))
                            Text(r.id).font(.caption.monospaced()).foregroundStyle(.secondary)
                            if let h = store.report?.rule_hits?[r.id], h.count == 2 {
                                Text("\(Fmt.bytes(h[1])) here").font(.caption).foregroundStyle(.teal)
                            }
                        }
                        Text(r.description).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                    Spacer()
                    Picker("", selection: Binding(get: { r.bucket }, set: { b in
                        Task { await store.configure(["rules", "set", r.id, "bucket=\"\(b)\""], rescan: true) }
                    })) {
                        ForEach(store.catalog?.buckets ?? []) { b in Text(b.name).tag(b.id) }
                    }
                    .labelsHidden()
                    .frame(width: 150)
                    if r.cost != "none" && r.tier == nil {
                        Stepper(value: Binding(get: { r.warm_days }, set: { d in
                            Task { await store.configure(["rules", "set", r.id, "warm_days=\(d)"], rescan: true) }
                        }), in: 0...365) {
                            Text(r.warm_days == 0 ? "default" : "\(r.warm_days)d").font(.caption).monospacedDigit().frame(width: 50)
                        }
                        .help("Unused this many days counts as cold")
                    }
                }
            }
        }
        .padding()
    }
}

struct AutomationSettings: View {
    @Environment(Store.self) private var store

    var body: some View {
        Form {
            GuardSection()
            Section {
                ForEach(store.presets.filter { $0.id != "guard" }) { p in
                    let existing = store.schedules.first { $0.id == p.id }
                    Toggle(isOn: Binding(get: { existing?.schedule.enabled ?? false }, set: { on in
                        Task {
                            if existing == nil {
                                await store.configure(["schedule", "add", p.id])
                            } else {
                                await store.configure(["schedule", on ? "enable" : "disable", p.id])
                            }
                        }
                    })) {
                        VStack(alignment: .leading) {
                            Text(p.id.capitalized)
                            Text(p.description).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            } header: {
                Text("Routine cleanups run on their own, rescan first, and only clean items without warnings.")
            }
            if !store.schedules.isEmpty {
                Section("Schedules") {
                    ForEach(store.schedules) { e in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("\(e.schedule.id) · \(e.schedule.plan) every \(e.schedule.every)\(e.schedule.at.map { " at \($0)" } ?? "")")
                                Text(detail(e)).font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Run now") { Task { await store.configure(["schedule", "run", e.id], rescan: false); await store.loadSchedules(); store.report = PooCLI.cachedReport() ?? store.report } }
                            Button("Remove") { Task { await store.configure(["schedule", "rm", e.id]) } }
                        }
                    }
                }
            }
            Section {
                Text("Agents can manage schedules too: `poo schedule add nightly`, or the MCP `schedules` tool.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .task { await store.loadSchedules() }
    }

    private func detail(_ e: ScheduleEntry) -> String {
        var parts: [String] = []
        if let c = e.schedule.when_free_below { parts.append("when free < \(c)") }
        if let t = e.schedule.target_free { parts.append("free up to \(t)") }
        if let l = e.last_run {
            parts.append("last \(Fmt.ago(Date(timeIntervalSince1970: TimeInterval(l.at)))): \(l.status) \(Fmt.bytes(l.freed_bytes))")
        } else {
            parts.append("never ran")
        }
        if e.schedule.enabled, let n = e.next_run {
            parts.append("next \(Fmt.ago(Date(timeIntervalSince1970: TimeInterval(n))))")
        }
        return parts.joined(separator: " · ")
    }
}

struct AgentSettings: View {
    @Environment(Store.self) private var store
    @State private var copied = false

    static let setupPrompt = "Set up poo for me: run `poo setup --agent`, read the brief, combine it with what you know about my projects and workflows, ask me only what you can't infer, then apply the configuration."

    var body: some View {
        Form {
            Section("Coding agents") {
                ForEach(store.integrations.filter(\.detected)) { t in
                    HStack {
                        Text(t.name)
                        Spacer()
                        Label(t.skill_current ? "Skill" : "No skill", systemImage: t.skill_current ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(t.skill_current ? .green : .secondary)
                        Label(t.mcp_registered ? "MCP" : "No MCP", systemImage: t.mcp_registered ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(t.mcp_registered ? .green : .secondary)
                        Button(t.skill_current && t.mcp_registered ? "Reinstall" : "Install") {
                            Task { await store.configure(["integration", "install", t.id]) }
                        }
                    }
                    .font(.callout)
                }
                Button("Install for all detected agents") { Task { await store.configure(["integration", "install", "all"]) } }
            }
            HooksSection()
            Section("Let your agent configure Poo") {
                Text(Self.setupPrompt).font(.callout).textSelection(.enabled)
                Button(copied ? "Copied" : "Copy prompt") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(Self.setupPrompt, forType: .string)
                    copied = true
                }
            }
        }
        .formStyle(.grouped)
        .task { await store.loadHooks() }
    }
}

struct ModSettings: View {
    @Environment(Store.self) private var store

    var body: some View {
        Form {
            Section("Mods") {
                ForEach(store.catalog?.mods ?? []) { m in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(m.name)
                            Text(m.builtin ? "built in · \(m.rules) rules" : "\(m.path.map(Fmt.tilde) ?? "") · \(m.rules) rules")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Toggle("", isOn: Binding(get: { m.enabled }, set: { on in
                            Task { await store.configure(["mods", on ? "enable" : "disable", m.id], rescan: true) }
                        }))
                        .labelsHidden()
                    }
                }
                if let errors = store.catalog?.errors, !errors.isEmpty {
                    ForEach(errors, id: \.self) { Text($0).font(.caption).foregroundStyle(.red) }
                }
            }
            Section {
                Button("Open mods folder") {
                    let url = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".config/poo/mods")
                    try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
                    NSWorkspace.shared.open(url)
                }
                Text("Mods teach Poo new folders, tweak or disable rules, add buckets, plans and guards. Ask your coding agent to write one: it can run `poo mods docs`.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

extension Color {
    var hexString: String? {
        guard let c = NSColor(self).usingColorSpace(.sRGB) else { return nil }
        return String(format: "#%02X%02X%02X", Int(c.redComponent * 255), Int(c.greenComponent * 255), Int(c.blueComponent * 255))
    }
}

// MARK: - Safety & backups

struct SafetySettings: View {
    @Environment(Store.self) private var store

    private var cfg: BackupConfig { store.backupConfig ?? BackupConfig() }

    var body: some View {
        Form {
            if store.backupConfig == nil {
                Section {
                    Label("These settings need a newer poo command-line tool. Defaults are shown.", systemImage: "info.circle")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
            Section {
                toggle("config_files", cfg.config_files ?? true,
                       "Copy small ignored files like .env and Local.xcconfig before removing a worktree",
                       "They are easy to lose and hard to recreate. Copies go to Backups.")
                Picker(selection: Binding(get: { cfg.max_file_bytes ?? 1_000_000 }, set: { set("max_file_bytes", "\($0)") })) {
                    Text("1 MB").tag(UInt64(1_000_000))
                    Text("5 MB").tag(UInt64(5_000_000))
                    Text("10 MB").tag(UInt64(10_000_000))
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Largest file to copy")
                        Text("Bigger ignored files are usually build output or data that can be regenerated.").font(.caption).foregroundStyle(.secondary)
                    }
                }
                .disabled(cfg.config_files == false)

                toggle("stash_dirty", cfg.stash_dirty ?? true,
                       "Stash uncommitted work before a forced worktree removal",
                       "Find it later with git stash list in the main checkout.")
                toggle("rescue_branches", cfg.rescue_branches ?? true,
                       "Save detached commits to a poo/ branch",
                       "Commits not on any branch would otherwise be lost when the worktree goes.")
            } header: {
                Text("Before cleaning")
            }
            Section("Deleting") {
                toggle("use_trash", cfg.use_trash ?? false,
                       "Move deleted folders to the Trash instead of deleting",
                       "Safer, but the space only comes back when you empty the Trash.")
            }
            Section("Keeping backups") {
                Picker(selection: Binding(get: { cfg.max_backup_bytes ?? 2_000_000_000 }, set: { set("max_backup_bytes", "\($0)") })) {
                    Text("500 MB").tag(UInt64(500_000_000))
                    Text("2 GB").tag(UInt64(2_000_000_000))
                    Text("10 GB").tag(UInt64(10_000_000_000))
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Backup budget")
                        Text("When backups grow past this, Poo drops the oldest file backups first.").font(.caption).foregroundStyle(.secondary)
                    }
                }

                Picker(selection: Binding(get: { cfg.auto_prune_days ?? 90 }, set: { set("auto_prune_days", "\($0)") })) {
                    Text("Never").tag(0)
                    Text("30 days").tag(30)
                    Text("90 days").tag(90)
                    Text("180 days").tag(180)
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Prune backups after")
                        Text("Never keeps every backup until you prune it in Backups.").font(.caption).foregroundStyle(.secondary)
                    }
                }

                toggle("auto_prune_git", cfg.auto_prune_git ?? false,
                       "Also drop poo's stashes and rescue branches after that many days",
                       "Off keeps them in git until you delete them yourself.")
                    .disabled((cfg.auto_prune_days ?? 90) == 0)
            }
        }
        .formStyle(.grouped)
        .disabled(store.backupConfig == nil)
        .task { await store.loadBackupConfig() }
    }

    private func set(_ key: String, _ value: String) {
        Task { await store.setBackup(key, value) }
    }

    private func toggle(_ key: String, _ on: Bool, _ title: String, _ detail: String) -> some View {
        Toggle(isOn: Binding(get: { on }, set: { set(key, $0 ? "true" : "false") })) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func help(_ s: String) -> some View {
        Text(s).font(.caption).foregroundStyle(.secondary)
    }
}

// MARK: - Low-disk guard

struct GuardSection: View {
    @Environment(Store.self) private var store
    @State private var below = "20G"
    @State private var target = "50G"

    private var entry: ScheduleEntry? { store.schedules.first { $0.id == "guard" } }

    var body: some View {
        Section {
            Toggle(isOn: Binding(get: { entry?.schedule.enabled ?? false }, set: { on in
                Task {
                    if entry == nil {
                        await store.setGuard(below: below, target: target, enabled: on)
                    } else {
                        await store.configure(["schedule", on ? "enable" : "disable", "guard"])
                    }
                }
            })) {
                Text("Clean up when the disk runs low")
            }
            HStack(spacing: 6) {
                Text("When free space drops below")
                sizePicker($below, ["10G", "20G", "30G", "50G"], current: entry?.schedule.when_free_below)
                Text(", clean what's not in use until")
                sizePicker($target, ["30G", "50G", "75G", "100G"], current: entry?.schedule.target_free)
                Text("is free.")
            }
            .disabled(entry?.schedule.enabled == false)
            if let l = entry?.last_run {
                Text("Last ran \(Fmt.ago(Date(timeIntervalSince1970: TimeInterval(l.at)))): \(l.status), freed \(Fmt.bytes(l.freed_bytes))")
                    .font(.caption).foregroundStyle(.secondary)
            }
        } header: {
            Text("Low-disk guard")
        } footer: {
            Text("Checks every minute while Poo runs (and every 30 minutes in the background). Only cleans items nothing is using, oldest and cheapest first.")
                .font(.caption).foregroundStyle(.secondary)
        }
        .onAppear {
            if let b = entry?.schedule.when_free_below { below = b }
            if let t = entry?.schedule.target_free { target = t }
        }
    }

    private func sizePicker(_ value: Binding<String>, _ options: [String], current: String?) -> some View {
        Picker("", selection: Binding(get: { value.wrappedValue }, set: { new in
            value.wrappedValue = new
            Task { await store.setGuard(below: below, target: target, enabled: entry?.schedule.enabled ?? true) }
        })) {
            ForEach(Array(Set(options + [value.wrappedValue])).sorted { (LowDiskGuard.parseThreshold($0, total: 1_000_000_000_000) ?? 0) < (LowDiskGuard.parseThreshold($1, total: 1_000_000_000_000) ?? 0) }, id: \.self) { o in
                Text(o.hasSuffix("G") ? o.dropLast() + " GB" : o).tag(o)
            }
        }
        .labelsHidden()
        .fixedSize()
    }
}

// MARK: - Agent hooks

struct HooksSection: View {
    @Environment(Store.self) private var store

    var body: some View {
        Section {
            if let h = store.hooks {
                ForEach(h.targets.filter { $0.detected ?? true }) { t in
                    HStack {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(t.name)
                            if let f = t.file { Text(f).font(.caption).foregroundStyle(.secondary) }
                        }
                        Spacer()
                        if t.installed == true {
                            Label("Installed", systemImage: "checkmark.circle.fill").foregroundStyle(.green).font(.caption)
                        }
                        Button(t.installed == true ? "Remove" : "Install") {
                            Task { await store.setHook(t.id, installed: t.installed != true) }
                        }
                    }
                }
                Picker("Clean up", selection: Binding(get: { h.on_agent_done ?? "landed" }, set: { v in Task { await store.setHooksMode(v) } })) {
                    Text("Only when the worktree's work landed in main").tag("landed")
                    Text("Whenever an agent session ends in a worktree").tag("always")
                    Text("Off").tag("off")
                }
            } else {
                Text(store.hooksAvailable ? "Loading…" : "Agent hooks need a newer poo command-line tool.")
                    .font(.callout).foregroundStyle(.secondary)
            }
        } header: {
            Text("Clean up after agents")
        } footer: {
            Text("Optional. When an agent finishes in a worktree, Poo cleans that worktree's build output. It never touches the code.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}
