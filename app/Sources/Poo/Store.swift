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

    /// Item ids the user ticked in the window.
    var selection: Set<String> = []

    init() {
        // First frame from the cached report; rescan in the background when stale.
        report = PooCLI.cachedReport()
        Task { await refreshIfStale() }
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

    func plan(_ id: String, free: UInt64? = nil) async -> PlanResult? {
        var args = ["plan", id]
        if let free { args += ["--free", "\(free)"] }
        do { return try await PooCLI.json(PlanResult.self, args) } catch {
            self.error = error.localizedDescription
            return nil
        }
    }

    func clean(ids: [String], force: Bool = false) async {
        guard !ids.isEmpty else { return }
        busy = true
        defer { busy = false }
        var args = ["clean"] + ids + ["--yes"]
        if force { args += ["--force", "--max-tier", "review"] }
        await runClean(args)
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
    }

    func reveal(_ item: Item) {
        guard !item.isVirtual else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: item.path)])
    }
}
