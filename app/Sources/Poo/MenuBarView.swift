import SwiftUI

/// The menu bar panel: free space, one Clean button, the areas behind it on demand.
/// `[menubar] sections` in config.toml adds or reorders the rest (growth, disk bar, buckets,
/// freed this week, next auto-clean).
struct MenuBarView: View {
    @Environment(Store.self) private var store
    @Environment(Updater.self) private var updater
    @Environment(Navigation.self) private var nav
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    private var config: MenubarSettings { store.menubar }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 10)
            if let r = store.report {
                if let s = store.menuSession {
                    Divider()
                    MenuCleaningView(session: s) {
                        store.menuSession = nil
                        open(.backups)
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                }
                ForEach(config.sections.compactMap(MenubarSettings.Section.init(rawValue:))) { sec in
                    section(sec, r)
                }
            } else if store.scanning {
                Divider()
                scanningView
            } else {
                Divider()
                VStack(spacing: 10) {
                    Label(store.error == nil ? "No scan yet" : "The scan didn't finish",
                          systemImage: store.error == nil ? "internaldrive" : "exclamationmark.triangle")
                        .font(.headline)
                        .foregroundStyle(.secondary)
                    Button("Scan Now") { Task { await store.scan() } }
                        .buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity, minHeight: 110)
            }
            if let e = store.error {
                Divider()
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange)
                    Text(e).lineLimit(2).truncationMode(.middle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Button { store.error = nil } label: { Image(systemName: "xmark") }
                        .buttonStyle(.borderless)
                        .accessibilityLabel("Dismiss")
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .help(e)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
            }
            Divider()
            footer
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
        }
        .frame(width: 320)
        .onAppear {
            store.live?.panelOpened()
            updater.isIdle = { [store] in !store.busy && !store.scanning && store.menuSession == nil }
        }
        .task {
            await store.loadLight()
            // `--preview-menu-cleaning`: show the panel's progress with a simulated clean (nothing
            // is deleted); `=freed` finishes it as a real clean would, with some still erasing.
            let args = CommandLine.arguments
            if args.contains(where: { $0.hasPrefix("--preview-menu-cleaning") }), store.menuSession == nil, let r = store.report {
                let items = r.items.filter { $0.status == .notInUse }.sorted { $0.total_bytes > $1.total_bytes }
                let s = CleanSession.fake(items: items, freeBefore: r.disk.free, speed: 0.3)
                if args.contains("--preview-menu-cleaning=freed") { s.dryRun = false }
                store.menuSession = s
            }
        }
    }

    // MARK: Header

    /// Free space, and nothing else: the Clean button below says what Poo can add to it.
    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            if let r = store.report {
                Text(Fmt.bytes(r.disk.free))
                    .font(.title2.weight(.semibold))
                    .monospacedDigit()
                    .contentTransition(.numericText())
                    .foregroundStyle(store.menubarWarns ? Color.orange : Color.primary)
                    .animation(.default, value: r.disk.free)
                Text("free of \(Fmt.bytes(r.disk.total))")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            } else {
                Text("Poo").font(.title2.weight(.semibold))
            }
            Spacer(minLength: 8)
            switch store.spaceLevel {
            case .critical: Text("Very low").font(.caption.weight(.medium)).foregroundStyle(.red)
            case .low: Text("Low").font(.caption.weight(.medium)).foregroundStyle(.orange)
            case .ok: EmptyView()
            }
        }
        .contextMenu {
            Button("Open Poo") { open(.overview) }
            Button("Copy Free Space") {
                guard let r = store.report else { return }
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString("\(Fmt.bytes(r.disk.free)) free of \(Fmt.bytes(r.disk.total))", forType: .string)
            }
        }
    }

    // MARK: Sections

    @ViewBuilder
    private func section(_ sec: MenubarSettings.Section, _ r: Report) -> some View {
        switch sec {
        case .health:
            HealthLine(report: r)
                .padding(.horizontal, 14)
                .padding(.bottom, 8)
                .padding(.top, -4)
        case .disk:
            Divider()
            DiskBreakdownBar(breakdown: DiskBreakdown(report: r), height: 6, onReview: { openReview() })
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .zIndex(2)
        case .areas:
            let areas = config.order(store.areas)
            if store.menuSession == nil {
                AreaPicker(areas: areas, limit: max(config.max_areas, 1), disabled: store.busy,
                           onReview: { openReview(areas: $0) },
                           onOpen: { a in
                               nav.browse(bucket: a.bucket.id, group: a.group, statuses: [.notInUse])
                               open(.browse)
                           },
                           onOpenAll: {
                               nav.browse(statuses: [.notInUse])
                               open(.browse)
                           })
            }
            if store.menuSession == nil, store.spaceLevel != .ok, let extra = inUseExtra(r) {
                Button {
                    nav.review(plan: "deep")
                    open(nav.section ?? .overview)
                } label: {
                    Text("\(Fmt.bytes(extra)) more in builds you'd rebuild…")
                        .monospacedDigit()
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .disabled(store.busy)
                .padding(.horizontal, 14)
                .padding(.bottom, 10)
                .help("Space is low. Also cleans builds and caches still in use but not running; they come back when rebuilt or redownloaded. Opens a review first.")
            }
        case .buckets:
            Divider()
            buckets(r)
        case .freed:
            if store.freedThisWeek > 0 {
                Divider()
                Label("\(Fmt.bytes(store.freedThisWeek)) freed this week", systemImage: "leaf")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
            }
        case .schedules:
            if let next = store.schedules.filter({ $0.schedule.enabled }).compactMap({ e in e.next_run.map { (e, $0) } }).min(by: { $0.1 < $1.1 }) {
                Divider()
                Label("Auto-clean \(Fmt.ago(Date(timeIntervalSince1970: TimeInterval(next.1))))", systemImage: "clock.arrow.2.circlepath")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .help("Next run of “\(next.0.id)”\(next.0.last_run.map { ". Last run freed \(Fmt.bytes($0.freed_bytes))." } ?? "")")
            }
        }
    }

    private func buckets(_ r: Report) -> some View {
        VStack(spacing: 1) {
            ForEach(r.visibleBuckets.sorted { $0.bytes > $1.bytes }) { b in
                MenuBucketRow(
                    bucket: b,
                    onOpen: { open(.bucket(b.id)) },
                    onClean: b.notInUseBytes > 0 && store.menuSession == nil && !store.busy ? { openReview(bucket: b.id) } : nil
                )
            }
        }
        .padding(.horizontal, 5)
        .padding(.vertical, 5)
    }

    private var scanningView: some View {
        ScanProgressView()
            .frame(height: 230)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Button("Open Poo") { open(.overview) }
                .keyboardShortcut("o")
            UpdateButton(compact: true)
                .labelStyle(.titleAndIcon)
                .tint(.accentColor)
            Spacer()
            refreshButton
            Menu {
                Button("Settings…") {
                    NSApp.activate(ignoringOtherApps: true)
                    openSettings()
                }
                .keyboardShortcut(",")
                Button("Rescan Everything") { Task { await store.scan(full: true) } }
                    .disabled(store.scanning)
                Divider()
                Button("Elsewhere on This Mac") { open(.elsewhere) }
                Button("Backups") { open(.backups) }
                Divider()
                Button("Report a Problem…") { ProblemReport.start() }
                Button("Quit Poo") { NSApp.terminate(nil) }
                    .keyboardShortcut("q")
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .accessibilityLabel("More")
        }
        .buttonStyle(.borderless)
        .font(.callout)
    }

    /// Refresh lives quietly at the bottom; how fresh the numbers are is in its tooltip.
    private var refreshButton: some View {
        ZStack {
            if store.refreshing || store.scanning {
                ProgressView().controlSize(.small)
            } else {
                Button {
                    // ⌥-click walks everything instead of only what changed.
                    Task { await store.scan(full: NSEvent.modifierFlags.contains(.option)) }
                } label: { Image(systemName: "arrow.clockwise") }
                    .keyboardShortcut("r")
                    .accessibilityLabel("Refresh")
            }
        }
        .frame(width: 18, height: 18)
        .help(store.report.map { "Updated \(FreshnessLabel.age($0.scannedAt, now: .now)). ⌘R to refresh, ⌥-click to rescan everything." } ?? "Refresh (⌘R)")
    }

    /// What the deep plan frees beyond what nothing uses, when that is worth a second button.
    private func inUseExtra(_ r: Report) -> UInt64? {
        guard let deep = r.plan("deep")?.bytes else { return nil }
        let base = r.plan("stale")?.bytes ?? 0
        let extra = deep > base ? deep - base : 0
        return extra >= 2_000_000_000 ? extra : nil
    }

    /// Reviewing and cleaning happen in the window: a menu bar panel closes on the first
    /// click elsewhere, so it only ever hands off with one click.
    private func openReview(bucket: String? = nil, areas: Set<String>? = nil) {
        guard let r = store.report else { return }
        let notInUse = store.notInUse
        if let bucket {
            nav.review(items: notInUse.filter { $0.bucket == bucket })
        } else if let areas {
            nav.review(items: notInUse.filter { areas.contains(AreaSummary.key($0)) })
        } else {
            nav.review(plan: "stale")
        }
        open(nav.section ?? .overview)
    }

    private func open(_ section: Navigation.Section) {
        nav.section = section
        openWindow(id: "main")
        NSApp.activate(ignoringOtherApps: true)
    }
}

struct MenuBucketRow: View {
    var bucket: BucketSummary
    var onOpen: () -> Void
    var onClean: (() -> Void)?
    @State private var hovering = false

    var body: some View {
        let unused = bucket.notInUseBytes
        HStack(alignment: .center, spacing: 8) {
            Button(action: onOpen) {
                HStack(alignment: .center, spacing: 10) {
                    Image(systemName: bucket.symbol)
                        .foregroundStyle(bucket.swiftColor)
                        .frame(width: 20)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(bucket.name)
                            .fixedSize(horizontal: false, vertical: true)
                        Text(unused > 0 && onClean == nil ? "\(Fmt.bytes(bucket.bytes)) · \(Fmt.bytes(unused)) not in use" : Fmt.bytes(bucket.bytes))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .monospacedDigit()
                    }
                    .layoutPriority(1)
                    Spacer(minLength: 4)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if let onClean {
                Button("Clean \(Fmt.bytes(unused))", action: onClean)
                    .controlSize(.small)
                    .fixedSize()
                    .help("Review and clean what's not in use in \(bucket.name)")
            }
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(hovering ? 0.07 : 0)))
        .onHover { hovering = $0 }
    }
}

/// The panel's compact progress: a small broom, the bar, Stop; then "Freed X" for a few seconds.
struct MenuCleaningView: View {
    @Bindable var session: CleanSession
    var onShowBackups: () -> Void
    @Environment(Store.self) private var store

    var body: some View {
        Group {
            switch session.phase {
            case .running:
                HStack(spacing: 10) {
                    SweepAnimation()
                        .frame(width: 72, height: 40)
                        .clipped()
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(session.cancelling ? "Stopping…" : "Cleaning \(min(session.done + 1, max(session.total, 1))) of \(session.total)")
                            .font(.callout.weight(.medium)).monospacedDigit()
                        if session.streaming {
                            ProgressView(value: session.fraction).controlSize(.small)
                        } else {
                            ProgressView().progressViewStyle(.linear).controlSize(.small)
                        }
                        Text("Freed \(Fmt.bytes(session.freedBytes))\(session.current.flatMap { $0.isEmpty ? nil : " · \($0)" } ?? "")")
                            .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                            .lineLimit(1).truncationMode(.middle)
                    }
                    Button { session.cancel() } label: { Image(systemName: "stop.circle") }
                        .buttonStyle(.borderless)
                        .help("Stop after this item")
                        .disabled(session.cancelling)
                }
            case .finished:
                let r = session.result
                let backups = (r?.results ?? []).contains { !($0.notes ?? []).isEmpty }
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).font(.title3)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("\(session.dryRun ? "Would free" : "Freed") \(Fmt.bytes(r?.freed_bytes ?? 0))").font(.callout.weight(.medium))
                        Text([r.map { "\($0.cleaned) cleaned" }, (r?.skipped ?? 0) > 0 ? "\(r!.skipped) skipped" : nil].compactMap { $0 }.joined(separator: " · "))
                            .font(.caption).foregroundStyle(.secondary)
                        if session.erasingBytes > 0 {
                            Text("\(Fmt.bytes(session.erasingBytes)) still erasing")
                                .font(.caption).foregroundStyle(.tertiary).monospacedDigit()
                                .contentTransition(.numericText())
                                .help("Erasing in the background. Free space catches up on its own.")
                                .transition(.opacity)
                        }
                    }
                    .animation(.smooth, value: session.erasingBytes)
                    Spacer()
                    if backups { Button("Backups", action: onShowBackups).controlSize(.small) }
                }
                .task {
                    try? await Task.sleep(for: .seconds(6))
                    if session.erasingBytes > 0 {
                        // Stay until the background erase settles, then show the final number briefly.
                        await session.settled()
                        try? await Task.sleep(for: .seconds(2))
                    }
                    if store.menuSession === session { store.menuSession = nil }
                }
            case .failed(let m):
                HStack {
                    Label(m, systemImage: "exclamationmark.triangle").font(.caption).lineLimit(3)
                    Spacer()
                    Button("OK") { store.menuSession = nil }.controlSize(.small)
                }
            }
        }
        .animation(.smooth, value: session.phase)
    }
}

/// Hover highlight like the rows in Control Center and other menu bar panels.
struct MenuRowStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        MenuRow(configuration: configuration)
    }

    private struct MenuRow: View {
        var configuration: Configuration
        @State private var hovering = false

        var body: some View {
            configuration.label
                .background(
                    RoundedRectangle(cornerRadius: 6)
                        .fill(Color.primary.opacity(configuration.isPressed ? 0.12 : hovering ? 0.07 : 0))
                )
                .onHover { hovering = $0 }
        }
    }
}

/// One calm line under the free space: what can go safely, and how fast the disk is filling.
struct HealthLine: View {
    @Environment(Store.self) private var store
    var report: Report

    var body: some View {
        // The same total the area picker offers, so the numbers agree.
        let safe = store.areas.reduce(0) { $0 + $1.reclaim }
        HStack(spacing: 5) {
            switch store.spaceLevel {
            case .critical: Label("Very low", systemImage: "circle.fill").foregroundStyle(.red)
            case .low: Label("Low", systemImage: "circle.fill").foregroundStyle(.orange)
            case .ok: EmptyView()
            }
            if store.spaceLevel != .ok { Text("·").foregroundStyle(.tertiary) }
            Text(safe > 0 ? "\(Fmt.bytes(safe)) safe to free" : "Nothing to clean")
            if let g = report.growth?.day_bytes, abs(g) >= 1_000_000_000 {
                Text("·").foregroundStyle(.tertiary)
                Text(g > 0 ? "+\(Fmt.bytes(UInt64(g))) today" : "−\(Fmt.bytes(UInt64(-g))) today")
                    .help(g > 0 ? "Used space grew this much in the last day" : "Used space shrank this much in the last day")
            }
            Spacer(minLength: 0)
        }
        .labelStyle(HealthDotLabel())
        .font(.caption)
        .foregroundStyle(.secondary)
        .monospacedDigit()
        .lineLimit(1)
    }
}

/// A small dot before a short status word.
private struct HealthDotLabel: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) {
            configuration.icon.font(.system(size: 6))
            configuration.title
        }
    }
}
