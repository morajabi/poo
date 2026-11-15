import AppKit
import Darwin
import Foundation

/// Keeps the store in step with the disk while Poo is on screen:
/// - reloads `report.json` whenever the CLI rewrites it (scan, clean, `poo watch`)
/// - reads free space with `statfs` every few seconds and on mount/unmount
/// - runs `poo watch` as a child while a window or the panel is visible
/// - runs incremental scans on activation, panel open, after cleans and every 10 minutes
@MainActor
final class LiveSync {
    private unowned let store: Store
    private let reportWatcher: ReportFileWatcher
    private var watch: WatchProcess?
    private var freeTimer: Timer?
    private var periodic: Task<Void, Never>?
    private var hideStop: Task<Void, Never>?
    private var observers: [NSObjectProtocol] = []
    private var panelWasVisible = false
    /// `poo watch` is missing from this CLI; don't try again until this date.
    private var watchRetryAfter: Date?

    static let freeInterval: TimeInterval = 3
    static let periodicInterval: TimeInterval = 10 * 60
    /// A report younger than this is fresh enough for panel open / activation.
    static let freshFor: TimeInterval = 30

    private(set) var visible = false

    init(store: Store) {
        self.store = store
        reportWatcher = ReportFileWatcher(url: PooCLI.reportURL)
        reportWatcher.onChange = { [weak store] in
            Task { @MainActor in await store?.reloadCachedReport() }
        }
    }

    func start() {
        reportWatcher.start()
        let nc = NotificationCenter.default
        let ws = NSWorkspace.shared.notificationCenter
        let recheck: (Notification) -> Void = { [weak self] _ in
            Task { @MainActor in
                self?.recomputeVisibility()
                // Occlusion state lags unhide/activation by a moment; look again.
                try? await Task.sleep(for: .milliseconds(500))
                self?.recomputeVisibility()
            }
        }
        for name in [NSWindow.didChangeOcclusionStateNotification, NSWindow.willCloseNotification,
                     NSWindow.didBecomeKeyNotification, NSApplication.didHideNotification,
                     NSApplication.didUnhideNotification] {
            observers.append(nc.addObserver(forName: name, object: nil, queue: .main, using: recheck))
        }
        observers.append(nc.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in
                self?.recomputeVisibility()
                try? await Task.sleep(for: .milliseconds(500))
                self?.recomputeVisibility()
                await self?.refreshIfOlder(than: Self.freshFor)
            }
        })
        observers.append(nc.addObserver(forName: NSApplication.willTerminateNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.watch?.stop() }
        })
        for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification, NSWorkspace.didWakeNotification] {
            observers.append(ws.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.store.updateFreeSpace() }
            })
        }
        observers.append(ws.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in if self?.visible == true { await self?.refreshIfOlder(than: Self.freshFor) } }
        })
        store.updateFreeSpace()
        recomputeVisibility()
    }

    func stop() {
        setVisible(false, immediate: true)
        for o in observers {
            NotificationCenter.default.removeObserver(o)
            NSWorkspace.shared.notificationCenter.removeObserver(o)
        }
        observers.removeAll()
        reportWatcher.stop()
    }

    /// The menu bar panel appeared (also detected through window occlusion).
    func panelOpened() {
        store.updateFreeSpace()
        Task { await refreshIfOlder(than: Self.freshFor) }
    }

    func refreshIfOlder(than seconds: TimeInterval) async {
        guard Onboarding.isDone, let r = store.report else { return }
        if Date().timeIntervalSince(r.scannedAt) > seconds { await store.scan() }
    }

    // MARK: Visibility

    /// Visible = a Poo window or the menu bar panel is on screen (not just open behind others).
    private func recomputeVisibility() {
        var panel = false
        var any = false
        for w in NSApp.windows where w.isVisible && w.occlusionState.contains(.visible) {
            let cls = String(describing: type(of: w))
            if cls.contains("StatusBarWindow") { continue } // the menu bar item itself
            if cls.contains("MenuBarExtra") { panel = true }
            any = true
        }
        if ProcessInfo.processInfo.environment["POO_DEBUG_LIVE"] != nil {
            FileHandle.standardError.write(Data("live: visible=\(any) panel=\(panel) windows=\(NSApp.windows.map { "\(type(of: $0)):\($0.isVisible):\($0.occlusionState.contains(.visible))" })\n".utf8))
        }
        if panel && !panelWasVisible { panelOpened() }
        panelWasVisible = panel
        setVisible(any, immediate: false)
    }

    private func setVisible(_ on: Bool, immediate: Bool) {
        if on {
            hideStop?.cancel()
            hideStop = nil
            guard !visible else { return }
            visible = true
            store.updateFreeSpace()
            freeTimer = Timer.scheduledTimer(withTimeInterval: Self.freeInterval, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.store.updateFreeSpace() }
            }
            freeTimer?.tolerance = 1
            startWatch()
            periodic = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(60))
                    guard !Task.isCancelled else { return }
                    // Tiers like "idle for 6h" move with time even when nothing changes on disk.
                    // `poo watch` already updates at least every 10 minutes on its own.
                    if self?.store.watching == true { continue }
                    await self?.refreshIfOlder(than: Self.periodicInterval)
                }
            }
        } else {
            guard visible, hideStop == nil else { return }
            // Closing and reopening the panel quickly shouldn't restart the watcher.
            let delay: Double = immediate ? 0 : 20
            hideStop = Task { [weak self] in
                if delay > 0 { try? await Task.sleep(for: .seconds(delay)) }
                guard !Task.isCancelled else { return }
                self?.goHidden()
            }
        }
    }

    private func goHidden() {
        visible = false
        hideStop = nil
        freeTimer?.invalidate()
        freeTimer = nil
        periodic?.cancel()
        periodic = nil
        watch?.stop()
        watch = nil
        store.watching = false
    }

    // MARK: poo watch

    private func startWatch() {
        guard watch == nil, Onboarding.isDone,
              ProcessInfo.processInfo.environment["POO_NO_WATCH"] == nil else { return }
        if let until = watchRetryAfter, Date() < until { return }
        let w = WatchProcess()
        w.onEvent = { [weak self] e in self?.handle(e) }
        w.onExit = { [weak self, weak w] unsupported in
            guard let self, self.watch === w else { return }
            self.watch = nil
            self.store.watching = false
            self.store.pendingChanges = false
            // An older CLI without `poo watch`: scans on the triggers above still keep things fresh.
            self.watchRetryAfter = Date().addingTimeInterval(unsupported ? 30 * 60 : 60)
            if self.visible, !unsupported {
                Task { @MainActor [weak self] in
                    try? await Task.sleep(for: .seconds(60))
                    if self?.visible == true { self?.startWatch() }
                }
            }
        }
        watch = w
        w.start()
    }

    private func handle(_ e: WatchProcess.Event) {
        switch e {
        case .ready:
            store.watching = true
        case .changed:
            store.pendingChanges = true
        case .report:
            store.pendingChanges = false
            Task { await store.reloadCachedReport() }
        case .error(let m):
            store.pendingChanges = false
            store.watchNote = m
        }
    }
}

/// Watches `report.json`. The CLI writes a temp file and renames it over the report, so
/// a vnode source on the file alone would go deaf after the first write; the directory
/// source notices the rename and re-arms the file source.
final class ReportFileWatcher: @unchecked Sendable {
    let url: URL
    var onChange: (@Sendable () -> Void)?
    private let queue = DispatchQueue(label: "poo.report-watch", qos: .utility)
    private var fileSource: DispatchSourceFileSystemObject?
    private var dirSource: DispatchSourceFileSystemObject?
    private var pending: DispatchWorkItem?
    private var lastStamp: (Date, Int)?

    init(url: URL) { self.url = url }

    func start() {
        queue.async { [self] in
            try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            lastStamp = stamp()
            armDirectory()
            armFile()
        }
    }

    func stop() {
        queue.async { [self] in
            fileSource?.cancel(); fileSource = nil
            dirSource?.cancel(); dirSource = nil
        }
    }

    private func armDirectory() {
        let fd = open(url.deletingLastPathComponent().path, O_EVTONLY)
        guard fd >= 0 else { return }
        let s = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .rename, .delete, .link], queue: queue)
        s.setEventHandler { [weak self] in self?.armFile(); self?.changed() }
        s.setCancelHandler { close(fd) }
        s.resume()
        dirSource = s
    }

    private func armFile() {
        fileSource?.cancel()
        fileSource = nil
        let fd = open(url.path, O_EVTONLY)
        guard fd >= 0 else { return }
        let s = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .extend, .rename, .delete, .attrib], queue: queue)
        s.setEventHandler { [weak self, weak s] in
            guard let self else { return }
            if let ev = s?.data, !ev.intersection([.rename, .delete]).isEmpty { self.armFile() }
            self.changed()
        }
        s.setCancelHandler { close(fd) }
        s.resume()
        fileSource = s
    }

    /// The modification date and size, to ignore events that didn't change the file.
    private func stamp() -> (Date, Int)? {
        guard let a = try? FileManager.default.attributesOfItem(atPath: url.path),
              let m = a[.modificationDate] as? Date, let n = a[.size] as? Int else { return nil }
        return (m, n)
    }

    /// Coalesces the burst of events a write + rename makes into one reload.
    private func changed() {
        pending?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            let s = self.stamp()
            if let s, let l = self.lastStamp, s.0 == l.0, s.1 == l.1 { return }
            self.lastStamp = s
            guard s != nil else { return }
            self.onChange?()
        }
        pending = work
        queue.asyncAfter(deadline: .now() + 0.15, execute: work)
    }
}

/// `poo watch --json` as a child process, read one JSON object per line.
@MainActor
final class WatchProcess {
    enum Event {
        case ready([String])
        case changed(Int)
        case report(generatedAt: Int64, tookMs: Int)
        case error(String)
    }

    var onEvent: ((Event) -> Void)?
    /// Called once when the process ends; `true` when the CLI doesn't know `poo watch`.
    var onExit: ((Bool) -> Void)?
    private var process: Process?
    private var stopping = false

    func start() {
        guard process == nil, let bin = PooCLI.binary else { onExit?(true); return }
        let p = Process()
        p.executableURL = bin
        p.arguments = ["watch", "--json"]
        let out = Pipe(), err = Pipe(), input = Pipe()
        p.standardOutput = out
        p.standardError = err
        // Kept open while we run: a CLI that watches stdin can exit when Poo goes away.
        p.standardInput = input
        let lines = LineSplitter { [weak self] line in
            Task { @MainActor in self?.handle(line) }
        }
        let errText = DataBox()
        out.fileHandleForReading.readabilityHandler = { h in lines.append(h.availableData) }
        err.fileHandleForReading.readabilityHandler = { h in
            let d = h.availableData
            errText.append(d)
            lines.append(d)
        }
        let started = Date()
        p.terminationHandler = { proc in
            out.fileHandleForReading.readabilityHandler = nil
            err.fileHandleForReading.readabilityHandler = nil
            let text = String(decoding: errText.data, as: UTF8.self)
            let quick = Date().timeIntervalSince(started) < 5
            let unsupported = quick && proc.terminationStatus != 0 &&
                (text.contains("unrecognized subcommand") || text.contains("unexpected argument") ||
                 text.contains("invalid value") || text.contains("usage"))
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.process = nil
                if !self.stopping { self.onExit?(unsupported || quick) }
            }
        }
        do {
            try p.run()
            process = p
        } catch {
            onExit?(true)
        }
    }

    func stop() {
        stopping = true
        guard let p = process, p.isRunning else { return }
        p.terminate()
        process = nil
    }

    private func handle(_ line: String) {
        if ProcessInfo.processInfo.environment["POO_DEBUG_LIVE"] != nil {
            FileHandle.standardError.write(Data("watch: \(line.prefix(200))\n".utf8))
        }
        guard let d = line.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
              let kind = obj["event"] as? String else { return }
        switch kind {
        case "ready": onEvent?(.ready(obj["roots"] as? [String] ?? []))
        case "changed": onEvent?(.changed(obj["paths"] as? Int ?? 0))
        case "report":
            onEvent?(.report(generatedAt: (obj["generated_at"] as? NSNumber)?.int64Value ?? 0,
                             tookMs: obj["took_ms"] as? Int ?? 0))
        case "error": onEvent?(.error(obj["message"] as? String ?? "poo watch failed"))
        default: break
        }
    }
}

/// Splits a byte stream into lines on a background queue.
final class LineSplitter: @unchecked Sendable {
    private var buffer = Data()
    private let lock = NSLock()
    private let onLine: @Sendable (String) -> Void

    init(onLine: @escaping @Sendable (String) -> Void) { self.onLine = onLine }

    func append(_ d: Data) {
        guard !d.isEmpty else { return }
        lock.lock()
        buffer.append(d)
        var lines: [String] = []
        while let nl = buffer.firstIndex(of: 0x0A) {
            lines.append(String(decoding: buffer[buffer.startIndex..<nl], as: UTF8.self))
            buffer.removeSubrange(buffer.startIndex...nl)
        }
        lock.unlock()
        lines.forEach(onLine)
    }
}

enum FreeSpace {
    /// Free bytes as the CLI computes them: `statfs` `f_bavail * f_bsize` on the home volume
    /// (`/` and the home folder share one APFS container, so they report the same).
    nonisolated static func read() -> (free: UInt64, total: UInt64)? {
        var s = statfs()
        guard statfs(NSHomeDirectory(), &s) == 0 else { return nil }
        let bsize = UInt64(s.f_bsize)
        return (UInt64(s.f_bavail) * bsize, UInt64(s.f_blocks) * bsize)
    }
}
