import SwiftUI

/// The menu bar panel. `[menubar] sections` in config.toml picks and orders what it shows:
/// disk (bar and breakdown), areas (tick and clean once), buckets, freed, schedules.
struct MenuBarView: View {
    @Environment(Store.self) private var store
    @Environment(Updater.self) private var updater
    @Environment(Navigation.self) private var nav
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @AppStorage("menuBreakdownExpanded") private var expanded = false
    @State private var review: PanelReview?

    struct PanelReview: Identifiable {
        var bucket: String?
        var areas: Set<String>?
        var id: String { (bucket ?? "all") + (areas?.sorted().joined(separator: ",") ?? "") }
    }

    private var config: MenubarSettings { store.menubar }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 10)
                .zIndex(2)
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
                ContentUnavailableView("No scan yet", systemImage: "internaldrive")
                    .frame(maxWidth: .infinity, minHeight: 120)
            }
            if let e = store.error {
                Divider()
                Label(e, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
            }
            Divider()
            footer
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
        }
        .frame(width: 320)
        .onAppear { store.live?.panelOpened() }
        .task {
            await store.loadSchedules()
            await store.loadHistory()
            // `--preview-menu-cleaning`: show the panel's progress with a simulated clean.
            if CommandLine.arguments.contains("--preview-menu-cleaning"), store.menuSession == nil, let r = store.report {
                let items = r.items.filter { $0.status == .notInUse }.sorted { $0.total_bytes > $1.total_bytes }
                store.menuSession = CleanSession.fake(items: items, freeBefore: r.disk.free, speed: 0.3)
            }
        }
        .sheet(item: $review) { rv in
            CleanConfirmView(planID: "stale", free: nil, bucket: rv.bucket, areas: rv.areas, onStart: { store.menuSession = $0 })
                .environment(store)
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            if let r = store.report {
                Text("\(Fmt.bytes(r.disk.free)) free")
                    .font(.title3.weight(.semibold))
                    .monospacedDigit()
                    .contentTransition(.numericText())
                    .foregroundStyle(store.menubarWarns ? Color.orange : Color.primary)
                    .animation(.default, value: r.disk.free)
            } else {
                Text("Poo").font(.title3.weight(.semibold))
            }
            Spacer(minLength: 8)
            FreshnessLabel(prefix: "updated")
                .font(.caption)
                .foregroundStyle(.tertiary)
            Button {
                // ⌥-click walks everything instead of only what changed.
                Task { await store.scan(full: NSEvent.modifierFlags.contains(.option)) }
            } label: { Image(systemName: "arrow.clockwise") }
                .buttonStyle(.borderless)
                .keyboardShortcut("r")
                .disabled(store.scanning)
                .help("Refresh (⌘R). ⌥-click to rescan everything.")
                .accessibilityLabel("Refresh")
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
            disk(r)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
        case .areas:
            let areas = config.order(AreaSummary.notInUse(r))
            if !areas.isEmpty, store.menuSession == nil {
                Divider()
                AreaPicker(areas: areas, limit: max(config.max_areas, 1), disabled: store.busy,
                           onReview: { review = PanelReview(bucket: nil, areas: $0) },
                           onOpen: { a in
                               nav.browse(bucket: a.bucket.id, group: a.group, statuses: [.notInUse])
                               open(.browse)
                           })
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
                Label("Auto-clean \(Fmt.ago(Date(timeIntervalSince1970: TimeInterval(next.1))))\(next.0.last_run.map { " · last freed \(Fmt.bytes($0.freed_bytes))" } ?? "")", systemImage: "clock.arrow.2.circlepath")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .help("Next run of “\(next.0.id)”")
            }
        }
    }

    private func disk(_ r: Report) -> some View {
        let d = DiskBreakdown(report: r)
        return VStack(alignment: .leading, spacing: 6) {
            DiskBreakdownBar(breakdown: d, height: 6, onReview: { review = PanelReview(bucket: nil) })
            Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                HStack(spacing: 5) {
                    Text("\(Fmt.bytes(d.used)) of \(Fmt.bytes(d.total)) used")
                    Spacer(minLength: 4)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .monospacedDigit()
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(expanded ? "Hide the breakdown" : "Show the breakdown")
            if expanded {
                Grid(alignment: .leading, horizontalSpacing: 8, verticalSpacing: 3) {
                    ForEach(d.segments) { s in
                        GridRow {
                            LegendSwatch(kind: s.kind)
                            Text(s.label).foregroundStyle(.secondary)
                            Text(Fmt.bytes(s.bytes)).monospacedDigit().gridColumnAlignment(.trailing)
                        }
                    }
                }
                .font(.caption)
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
    }

    private func buckets(_ r: Report) -> some View {
        VStack(spacing: 1) {
            ForEach(r.visibleBuckets.sorted { $0.bytes > $1.bytes }) { b in
                MenuBucketRow(
                    bucket: b,
                    onOpen: { open(.bucket(b.id)) },
                    onClean: b.notInUseBytes > 0 && store.menuSession == nil && !store.busy ? { review = PanelReview(bucket: b.id) } : nil
                )
            }
        }
        .padding(.horizontal, 5)
        .padding(.vertical, 5)
    }

    private var scanningView: some View {
        VStack(alignment: .leading, spacing: 6) {
            ProgressView().controlSize(.small)
            if let p = store.progress {
                Text("\(p.phase.capitalized)… \(Fmt.bytes(p.bytes)) in \(p.files) files")
                    .font(.callout)
                Text(p.detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 100)
        .padding(.horizontal, 14)
    }

    private var footer: some View {
        HStack {
            Button("Open Poo") { open(.overview) }
                .keyboardShortcut("o")
            if updater.available != nil {
                Button("Update") { Task { await updater.install() } }
            }
            Spacer()
            Menu {
                Button("Settings…") {
                    NSApp.activate(ignoringOtherApps: true)
                    openSettings()
                }
                .keyboardShortcut(",")
                Button("Rescan Everything") { Task { await store.scan(full: true) } }
                Divider()
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
                    }
                    Spacer()
                    if backups { Button("Backups", action: onShowBackups).controlSize(.small) }
                }
                .task {
                    try? await Task.sleep(for: .seconds(6))
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
        let safe = AreaSummary.notInUse(report).reduce(0) { $0 + $1.reclaim }
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
