import Foundation
import UserNotifications

/// Watches free space while the app runs. When the "guard" schedule is on and free space
/// drops below its threshold, it asks the CLI to run that schedule (at most every 15 minutes)
/// and posts a friendly notification about what it freed.
@MainActor
final class LowDiskGuard {
    private unowned let store: Store
    private var task: Task<Void, Never>?
    private var lastRun: Date?
    /// The low-space notification went out; it re-arms once space recovers.
    private var notifiedLow = false
    private static let minInterval: TimeInterval = 15 * 60

    init(store: Store) { self.store = store }

    func start() {
        guard task == nil, ProcessInfo.processInfo.environment["POO_NO_GUARD"] == nil else { return }
        task = Task { [weak self] in
            while !Task.isCancelled {
                await self?.tick()
                try? await Task.sleep(for: .seconds(60))
            }
        }
    }

    /// Free bytes on the home volume, read off the main thread.
    nonisolated static func freeSpace() async -> (free: UInt64, total: UInt64)? {
        await Task.detached(priority: .utility) {
            let url = URL(fileURLWithPath: NSHomeDirectory())
            guard let v = try? url.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey, .volumeTotalCapacityKey]),
                  let free = v.volumeAvailableCapacityForImportantUsage else { return nil }
            return (UInt64(max(0, free)), UInt64(max(0, v.volumeTotalCapacity ?? 0)))
        }.value
    }

    /// Parses "20G", "500M", "1.5T" or "10%" (of `total`) into bytes.
    nonisolated static func parseThreshold(_ s: String, total: UInt64) -> UInt64? {
        var t = s.trimmingCharacters(in: .whitespaces).uppercased()
        if t.hasSuffix("%") {
            guard let p = Double(t.dropLast()) else { return nil }
            return UInt64(Double(total) * p / 100)
        }
        if t.hasSuffix("B") { t.removeLast() }
        if t.hasSuffix("I") { t.removeLast() }
        let units: [Character: Double] = ["K": 1e3, "M": 1e6, "G": 1e9, "T": 1e12]
        var mult = 1.0
        if let last = t.last, let u = units[last] { mult = u; t.removeLast() }
        guard let n = Double(t.trimmingCharacters(in: .whitespaces)) else { return nil }
        return UInt64(n * mult)
    }

    private func tick() async {
        guard let (free, total) = await Self.freeSpace() else { return }
        notifyIfLow(free: free, total: total)
        if store.schedules.isEmpty { await store.loadSchedules() }
        guard let g = store.schedules.first(where: { $0.id == "guard" }), g.schedule.enabled,
              let below = g.schedule.when_free_below,
              let threshold = Self.parseThreshold(below, total: total) else {
            store.lowDisk = false
            return
        }
        store.lowDisk = free < threshold
        guard free < threshold, !store.busy, !store.scanning else { return }
        if let lastRun, Date().timeIntervalSince(lastRun) < Self.minInterval { return }
        lastRun = Date()
        do {
            _ = try await PooCLI.run(["schedule", "run", "guard"])
        } catch {
            store.error = error.localizedDescription
            return
        }
        await store.loadSchedules()
        await store.reloadCachedReport()
        await store.loadHistory()
        await store.scan()
        let freedBytes = store.schedules.first { $0.id == "guard" }?.last_run?.freed_bytes ?? 0
        let after = await Self.freeSpace()?.free ?? free
        store.lowDisk = after < threshold
        if freedBytes > 0 {
            Self.notify(title: "Freed \(Fmt.bytes(freedBytes)) of old builds",
                        body: "\(Fmt.bytes(after)) free now. Nothing in use was touched.")
        }
    }

    /// `[menubar] notify`: one calm note per crossing below the threshold, no alarms.
    private func notifyIfLow(free: UInt64, total: UInt64) {
        guard store.menubar.notify, let t = store.menubar.threshold(total: total) else { return }
        if free < t, !notifiedLow {
            notifiedLow = true
            let safe = store.report.map { $0.plan("stale")?.bytes ?? $0.notInUseBytes } ?? 0
            Self.notify(title: "Space is getting low",
                        body: safe > 0
                            ? "\(Fmt.bytes(free)) free. \(Fmt.bytes(safe)) of builds and caches nothing is using can go safely."
                            : "\(Fmt.bytes(free)) free. Nothing Poo tracks is safe to clean right now.")
        } else if free > t + 2_000_000_000 {
            notifiedLow = false
        }
    }

    /// Posts a local notification. Only works from a bundled app (not `swift run`).
    static func notify(title: String, body: String) {
        guard Bundle.main.bundleIdentifier != nil else { return }
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            guard granted else { return }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
    }
}
