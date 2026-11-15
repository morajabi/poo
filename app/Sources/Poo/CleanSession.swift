import Foundation
import Observation

/// A clean in flight: progress from `poo clean --progress`, cancel via SIGTERM, and the result.
@MainActor
@Observable
final class CleanSession: Identifiable {
    enum Phase: Equatable { case running, finished, failed(String) }

    let id = UUID()
    var phase: Phase = .running
    var done = 0
    var total: Int
    var freedBytes: UInt64 = 0
    var totalBytes: UInt64
    var current: String?
    /// False when the CLI doesn't stream progress (older versions): the bar is indeterminate.
    var streaming = false
    var cancelling = false
    var result: CleanResult?
    var freeBefore: UInt64?
    /// A rehearsal: poo reports what it would do and deletes nothing.
    var dryRun = false

    @ObservationIgnored private var process: Process?
    @ObservationIgnored private var fakeTask: Task<Void, Never>?

    init(items: [Item], freeBefore: UInt64?) {
        total = items.count
        totalBytes = items.reduce(0) { $0 + $1.total_bytes }
        self.freeBefore = freeBefore
    }

    var fraction: Double {
        if totalBytes > 0, freedBytes > 0 { return min(1, Double(freedBytes) / Double(totalBytes)) }
        return total > 0 ? Double(done) / Double(total) : 0
    }

    func apply(_ e: CleanProgressEvent) {
        streaming = true
        done = e.done
        total = max(total, e.total)
        freedBytes = e.freed_bytes
        if let t = e.total_bytes, t > 0 { totalBytes = t }
        current = e.current
    }

    func attach(_ p: Process) { process = p }

    /// Asks poo to stop after the item in flight; it still reports what it did.
    func cancel() {
        guard phase == .running, !cancelling else { return }
        cancelling = true
        if let fakeTask {
            fakeTask.cancel()
            finishFake(cancelled: true)
        } else {
            process?.terminate()
        }
    }

    func finish(_ r: CleanResult) {
        result = r
        if let b = r.disk_free_before { freeBefore = b }
        freedBytes = r.freed_bytes
        done = r.cleaned + r.skipped + r.failed
        current = nil
        phase = .finished
    }

    // MARK: Preview / tests

    /// Simulates a clean without touching anything (`Poo --preview-cleaning`).
    static func fake(items: [Item], freeBefore: UInt64, speed: Double = 1) -> CleanSession {
        let s = CleanSession(items: items, freeBefore: freeBefore)
        s.dryRun = true
        s.fakeTask = Task { @MainActor [weak s] in
            var freed: UInt64 = 0
            for (i, item) in items.enumerated() {
                try? await Task.sleep(for: .milliseconds(Int(220 / speed)))
                guard let s, !Task.isCancelled else { return }
                freed += item.total_bytes
                s.apply(CleanProgressEvent(done: i + 1, total: items.count, freed_bytes: freed, total_bytes: s.totalBytes, current: item.name, status: "cleaned"))
            }
            try? await Task.sleep(for: .milliseconds(300))
            s?.finishFake(cancelled: false)
        }
        return s
    }

    private func finishFake(cancelled: Bool) {
        // Neutral sample names: this only feeds previews and screenshots.
        let skipped = CleanItemResult(id: "x", path: "/tmp/x", name: "web-dash/.next", status: "skipped", bytes: 0,
                                      reason: "a build started in this checkout", warnings: nil, notes: nil)
        let backup = CleanItemResult(id: "y", path: "/tmp/y", name: "atlas-api-search", status: "cleaned", bytes: 0, reason: nil,
                                     warnings: nil, notes: ["Backed up 2 ignored config files to Backups", "Branch feature/search stays"])
        finish(CleanResult(dry_run: dryRun, freed_bytes: freedBytes, cleaned: done, skipped: 1, failed: 0,
                           disk_free_after: (freeBefore ?? 0) + freedBytes, results: [backup, skipped],
                           snapshot_note: nil, cancelled: cancelled, disk_free_before: freeBefore))
    }
}
