import AppKit
import Foundation
import Observation

@MainActor
@Observable
final class Store {
    var report: Report?
    var scanning = false
    var progress: ScanProgress?
    var error: String?
    var lastClean: CleanResult?
    var busy = false
    var catalog: Catalog?
    var integrations: [IntegrationTarget] = []
    var roots: RootsInfo?
    var schedules: [ScheduleEntry] = []
    var presets: [SchedulePreset] = []
    var backups: BackupList?
    /// False when the CLI does not know `poo backups` yet.
    var backupsAvailable = true
    var backupConfig: BackupConfig?
    var backupNote: String?
    var hooks: HookStatus?
    var hooksAvailable = true
    /// Free space is below the low-disk guard's threshold.
    var lowDisk = false
    @ObservationIgnored private var diskGuard: LowDiskGuard?
    var launchers: LauncherList?
    /// Bytes poo freed in the last 7 days (`poo history`).
    var freedThisWeek: UInt64 = 0
    /// A clean started from the menu bar panel, shown in the panel.
    var menuSession: CleanSession?
    /// Shown briefly after "Copy prompt".
    var toast: String?

    /// Item ids the user ticked in the window.
    var selection: Set<String> = []

    init() {
        // First frame from the cached report; rescan in the background when stale.
        report = PooCLI.cachedReport()
        // No scan before setup: the first scan waits for dev folders and permissions.
        if Onboarding.isDone {
            Task { await refreshIfStale() }
            diskGuard = LowDiskGuard(store: self)
            diskGuard?.start()
        }
    }

    var freeText: String {
        guard let r = report else { return "poo" }
        return Fmt.bytes(r.disk.free)
    }

    func refreshIfStale() async {
        let age = report.map { Date().timeIntervalSince($0.scannedAt) } ?? .infinity
        if age > 30 * 60 {
            await scan()
        }
    }

    func scan() async {
        guard !scanning else { return }
        scanning = true
        error = nil
        progress = nil
        defer { scanning = false; progress = nil }
        do {
            let data = try await PooCLI.run(["scan", "--progress"]) { line in
                guard let d = line.data(using: .utf8),
                      let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
                      obj["event"] as? String == "progress",
                      let p = obj["progress"],
                      let pd = try? JSONSerialization.data(withJSONObject: p),
                      let prog = try? JSONDecoder().decode(ScanProgress.self, from: pd) else { return }
                Task { @MainActor in self.progress = prog }
            }
            report = try JSONDecoder().decode(Report.self, from: data)
            selection = selection.filter { id in report?.items.contains { $0.id == id } ?? false }
        } catch {
            self.error = error.localizedDescription
        }
    }

    func items(in bucket: String) -> [Item] {
        report?.items.filter { $0.bucket == bucket } ?? []
    }

    func loadHistory() async {
        guard let h = try? await PooCLI.json(HistoryList.self, ["history"]) else { return }
        let since = Int64(Date().timeIntervalSince1970) - 7 * 86_400
        freedThisWeek = h.history.filter { $0.at >= since }.reduce(0) { $0 + $1.bytes }
    }

    func plan(_ id: String, free: UInt64? = nil, bucket: String? = nil) async -> PlanResult? {
        var args = ["plan", id]
        if let bucket { args.append(bucket) }
        if let free { args += ["--free", "\(free)"] }
        do { return try await PooCLI.json(PlanResult.self, args) } catch {
            self.error = error.localizedDescription
            return nil
        }
    }

    func clean(ids: [String], mode: CleanMode = .safe) async {
        guard !ids.isEmpty else { return }
        busy = true
        defer { busy = false }
        var args = ["clean"] + ids + ["--yes"]
        if let flag = mode.flag { args.append(flag) }
        await runClean(args)
    }

    /// Starts a clean with live progress. The sheet shows the returned session.
    func startClean(_ items: [Item], mode: CleanMode, dryRun: Bool = false) -> CleanSession {
        let session = CleanSession(items: items, freeBefore: report?.disk.free)
        session.dryRun = dryRun
        busy = true
        Task {
            var args = ["clean"] + items.map(\.id) + ["--yes"]
            if let flag = mode.flag { args.append(flag) }
            if dryRun { args.append("--dry-run") }
            do {
                let data: Data
                do {
                    data = try await runStreaming(args + ["--progress"], session)
                } catch let f as PooCLI.Failure where f.kind == "usage" && f.message.contains("--progress") {
                    // Older CLI without progress: same clean, no stream (it never started).
                    data = try await runStreaming(args, session)
                }
                let r = try JSONDecoder().decode(CleanResult.self, from: data)
                if !r.dry_run { lastClean = r }
                session.finish(r)
            } catch {
                session.phase = .failed(error.localizedDescription)
            }
            busy = false
            if !dryRun {
                report = PooCLI.cachedReport() ?? report
                selection.subtract(items.map(\.id))
                await loadHistory()
                await scan()
            }
        }
        return session
    }

    private func runStreaming(_ args: [String], _ session: CleanSession) async throws -> Data {
        try await PooCLI.run(args, onStderrLine: { line in
            guard let d = line.data(using: .utf8),
                  let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
                  obj["event"] as? String == "clean-progress",
                  let e = try? JSONDecoder().decode(CleanProgressEvent.self, from: d) else { return }
            Task { @MainActor in session.apply(e) }
        }, onStart: { p in
            Task { @MainActor in session.attach(p) }
        })
    }

    func clean(plan: String, free: UInt64? = nil) async {
        busy = true
        defer { busy = false }
        var args = ["clean", "--plan", plan, "--yes"]
        if let free { args += ["--free", "\(free)"] }
        await runClean(args)
    }

    private func runClean(_ args: [String]) async {
        do {
            let data = try await PooCLI.run(args)
            lastClean = try JSONDecoder().decode(CleanResult.self, from: data)
        } catch let f as PooCLI.Failure where f.kind == "partial" {
            self.error = f.message
        } catch {
            self.error = error.localizedDescription
        }
        // The CLI already removed cleaned items from the cached report.
        report = PooCLI.cachedReport() ?? report
        selection.removeAll()
    }

    /// Below the low-disk guard's threshold, or under 10% free.
    var isLowOnSpace: Bool {
        if lowDisk { return true }
        guard let d = report?.disk, d.total > 0 else { return false }
        return d.free < d.total / 10
    }

    var selectedItems: [Item] {
        selection.isEmpty ? [] : (report?.items.filter { selection.contains($0.id) } ?? [])
    }

    func toggle(_ id: String) {
        if selection.contains(id) { selection.remove(id) } else { selection.insert(id) }
    }

    func select(_ items: [Item], _ on: Bool) {
        let ids = items.filter(\.isCleanable).map(\.id)
        if on { selection.formUnion(ids) } else { selection.subtract(ids) }
    }

    // MARK: Backups (made by the CLI before it removes worktrees, stashes, branches)

    func loadBackups() async {
        do {
            backups = try await PooCLI.json(BackupList.self, ["backups", "ls"])
            backupsAvailable = true
        } catch {
            backups = nil
            backupsAvailable = false
        }
    }

    func pruneBackups(ids: [String]) async {
        guard !ids.isEmpty else { return }
        await backupCommand(["backups", "prune"] + ids + ["--yes"], done: "Pruned \(ids.count) backup\(ids.count == 1 ? "" : "s")")
    }

    func pruneBackups(olderThanDays days: Int) async {
        await backupCommand(["backups", "prune", "--older-than", "\(days)d", "--yes"], done: "Pruned backups older than \(days) days")
    }

    func restoreBackup(_ id: String) async {
        await backupCommand(["backups", "restore", id, "--yes"], done: "Restored backup \(id)")
    }

    private func backupCommand(_ args: [String], done: String) async {
        busy = true
        defer { busy = false }
        do {
            _ = try await PooCLI.run(args)
            backupNote = done
        } catch {
            self.error = error.localizedDescription
        }
        await loadBackups()
    }

    func loadBackupConfig() async {
        backupConfig = try? await PooCLI.json(BackupConfig.self, ["config", "get", "backup"])
    }

    func setBackup(_ key: String, _ value: String) async {
        do {
            _ = try await PooCLI.run(["config", "set", "backup.\(key)", value])
        } catch {
            self.error = error.localizedDescription
        }
        await loadBackupConfig()
    }

    func startGuard() {
        guard diskGuard == nil else { return }
        diskGuard = LowDiskGuard(store: self)
        diskGuard?.start()
    }

    // MARK: Ask your agent

    func loadLaunchers(force: Bool = false) async {
        guard force || launchers == nil else { return }
        launchers = try? await PooCLI.json(LauncherList.self, ["ask", "--list"])
    }

    /// Opens an agent with poo's brief about `target` (item id, project:<path>, bucket:<id>, overview).
    func ask(_ target: String, with launcher: String) async {
        do {
            _ = try await PooCLI.run(["ask", target, "--with", launcher])
        } catch let f as PooCLI.Failure where f.kind == "not_found" || f.kind == "usage" {
            // An older CLI that doesn't know this target: hand over a prompt built here.
            copy(localPrompt(target))
            let name = launchers?.launchers.first { $0.id == launcher }?.name ?? "your agent"
            flash("Prompt copied. Paste it into \(name).")
        } catch {
            self.error = error.localizedDescription
        }
    }

    func copyPrompt(_ target: String) async {
        let text = (try? await PooCLI.json(AgentPrompt.self, ["prompt", target]))?.prompt ?? localPrompt(target)
        copy(text)
        flash("Prompt copied. Paste it into your agent.")
    }

    private func copy(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    /// A brief built from the cached report, for targets the CLI can't describe yet.
    func localPrompt(_ target: String) -> String {
        guard let r = report else { return "Help me review my disk use with poo (run `poo overview`)." }
        let outro = "\n\nUse `poo show <id>` for details and look at the folders yourself. Tell me what each one really is and what's safe to clean. Don't delete anything; I'll clean through Poo after reviewing."
        func line(_ i: Item) -> String {
            "- \(Fmt.bytes(i.total_bytes)) \(i.name) [\(i.tier.label.lowercased())] \(Fmt.tilde(i.path)): \(i.reasons.joined(separator: "; ")) (poo id \(i.id))"
        }
        if target.hasPrefix("items:") {
            let ids = Set(target.dropFirst(6).split(separator: ",").map(String.init))
            let items = r.items.filter { ids.contains($0.id) }.sorted { $0.total_bytes > $1.total_bytes }
            return "poo, my disk-space tool, found these \(items.count) items (\(Fmt.bytes(items.reduce(0) { $0 + $1.total_bytes }))). Help me review them.\n\n" + items.map(line).joined(separator: "\n") + outro
        }
        if target == "projects" {
            let ps = r.projects.sorted { $0.artifact_bytes > $1.artifact_bytes }.prefix(60)
            return "poo, my disk-space tool, found these projects with build output and caches. Help me decide which I still need.\n\n" + ps.map { p in
                "- \(Fmt.bytes(p.artifact_bytes)) \(p.name) (\(Fmt.tilde(p.path))) \(p.kind), \(p.status), last active \(Fmt.ago(p.last_activity.map { Date(timeIntervalSince1970: TimeInterval($0)) })), stacks: \(p.stacks.joined(separator: ", "))"
            }.joined(separator: "\n") + outro
        }
        if target == "worktrees" {
            let ws = r.projects.filter { $0.kind == "worktree" }.sorted { $0.artifact_bytes > $1.artifact_bytes }
            return "poo, my disk-space tool, found these git worktrees. I don't remember what all of them are. Check each one (branch, uncommitted work, whether it landed in main) and tell me which I can remove.\n\n" + ws.map { p in
                var flags: [String] = [p.branch ?? "detached"]
                if p.dirty == true { flags.append("uncommitted changes") }
                if p.merged == true { flags.append("merged") }
                if let a = p.agent { flags.append("made by \(a)") }
                return "- \(Fmt.bytes(p.artifact_bytes)) \(Fmt.tilde(p.path)) (\(flags.joined(separator: ", "))), repo \(p.repo.map(Fmt.tilde) ?? "?")"
            }.joined(separator: "\n") + outro
        }
        return "Help me review my disk use with poo. Run `poo overview` and `poo prompt \(target)` for details." + outro
    }

    func flash(_ message: String) {
        toast = message
        Task {
            try? await Task.sleep(for: .seconds(2.5))
            if toast == message { toast = nil }
        }
    }

    // MARK: Agent hooks

    func loadHooks() async {
        do {
            hooks = try await PooCLI.json(HookStatus.self, ["hooks", "status"])
            hooksAvailable = true
        } catch {
            hooks = nil
            hooksAvailable = false
        }
    }

    func setHook(_ id: String, installed: Bool) async {
        await configure(["hooks", installed ? "install" : "uninstall", id])
        await loadHooks()
    }

    func setHooksMode(_ value: String) async {
        await configure(["config", "set", "hooks.on_agent_done", value])
        await loadHooks()
    }

    /// Adds or replaces the low-disk guard schedule.
    func setGuard(below: String, target: String, enabled: Bool) async {
        if schedules.contains(where: { $0.id == "guard" }) {
            _ = try? await PooCLI.run(["schedule", "rm", "guard"])
        }
        await configure(["schedule", "add", "guard", "--preset", "guard", "--when-free-below", below, "--target-free", target])
        if !enabled { await configure(["schedule", "disable", "guard"]) }
        await loadSchedules()
    }

    // MARK: Configuration (all through the CLI so config.toml stays the source of truth)

    func configure(_ args: [String], rescan: Bool = false) async {
        do {
            _ = try await PooCLI.run(args)
            await loadSettings()
            if rescan { await scan() }
        } catch {
            self.error = error.localizedDescription
        }
    }

    func setPolicy(_ project: String, _ policy: String) async {
        await configure(["projects", "set", project, policy], rescan: true)
    }

    func loadSettings() async {
        catalog = try? await PooCLI.json(Catalog.self, ["catalog"])
        integrations = (try? await PooCLI.json(IntegrationStatus.self, ["integration", "status"]))?.targets ?? []
        roots = try? await PooCLI.json(RootsInfo.self, ["roots"])
        await loadSchedules()
    }

    func loadSchedules() async {
        schedules = (try? await PooCLI.json(ScheduleList.self, ["schedule", "ls"]))?.schedules ?? []
        if presets.isEmpty {
            presets = (try? await PooCLI.json(PresetList.self, ["schedule", "presets"]))?.presets ?? []
        }
    }

    /// Stacks and tools poo detected, for adapting what it shows.
    var detected: Set<String> { Set(report?.detected ?? []) }

    func reveal(_ item: Item) {
        guard !item.isVirtual else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: item.path)])
    }
}
