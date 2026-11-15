import SwiftUI

/// The menu bar panel: free space, a collapsible disk breakdown, buckets with one-click
/// reviewed cleans, and a way into the window for detailed work.
struct MenuBarView: View {
    @Environment(Store.self) private var store
    @Environment(Updater.self) private var updater
    @Environment(Navigation.self) private var nav
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @AppStorage("menuBreakdownExpanded") private var expanded = false
    /// "picker" (tick areas, clean once) or "buckets" (a row and a Clean button per bucket).
    @AppStorage("menuAreaStyle") private var areaStyle = "picker"
    @State private var review: PanelReview?

    struct PanelReview: Identifiable {
        var bucket: String?
        var areas: Set<String>?
        var id: String { (bucket ?? "all") + (areas?.sorted().joined(separator: ",") ?? "") }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 10)
                .zIndex(2)
            Divider()
            if let r = store.report {
                let areas = areaStyle == "picker" ? AreaSummary.notInUse(r) : []
                if !areas.isEmpty, store.menuSession == nil {
                    AreaPicker(areas: areas, disabled: store.busy,
                               onReview: { review = PanelReview(bucket: nil, areas: $0) },
                               onOpen: { a in
                                   nav.browse(bucket: a.bucket.id, group: a.group, statuses: [.notInUse])
                                   open(.browse)
                               })
                } else {
                    action(r)
                    buckets(r)
                }
            } else if store.scanning {
                scanningView
            } else {
                ContentUnavailableView("No scan yet", systemImage: "internaldrive", description: Text("Scan to see what takes space."))
                    .frame(maxWidth: .infinity, minHeight: 140)
            }
            Divider()
            VStack(alignment: .leading, spacing: 6) {
                Button { open(.overview) } label: {
                    Text("Open Poo").frame(maxWidth: .infinity)
                }
                .controlSize(.large)
                .keyboardShortcut("o")
                status
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            Divider()
            footer
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
        }
        .frame(width: 340)
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
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                if let r = store.report {
                    Text("\(Fmt.bytes(r.disk.free)) free")
                        .font(.title3.weight(.semibold))
                        .monospacedDigit()
                        .contentTransition(.numericText())
                        .animation(.default, value: r.disk.free)
                } else {
                    Text("Poo").font(.title3.weight(.semibold))
                }
                Spacer(minLength: 8)
                Button {
                    // ⌥-click walks everything instead of only what changed.
                    let full = NSEvent.modifierFlags.contains(.option)
                    Task { await store.scan(full: full) }
                } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.borderless)
                    .keyboardShortcut("r")
                    .disabled(store.scanning)
                    .help("Refresh (⌘R). ⌥-click to rescan everything.")
                    .contextMenu {
                        Button("Refresh") { Task { await store.scan() } }
                        Button("Rescan Everything") { Task { await store.scan(full: true) } }
                    }
            }
            if let r = store.report {
                let d = DiskBreakdown(report: r)
                Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                    HStack(spacing: 5) {
                        if r.notInUseBytes > 0 {
                            Circle().fill(Palette.reclaim).frame(width: 6, height: 6)
                            Text("\(Fmt.bytes(r.notInUseBytes)) not in use")
                        } else {
                            Text("Nothing to clean right now")
                        }
                        Text("·").foregroundStyle(.tertiary)
                        FreshnessLabel(prefix: "updated").foregroundStyle(.tertiary)
                        Spacer(minLength: 4)
                        Image(systemName: "chevron.right")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.tertiary)
                            .rotationEffect(.degrees(expanded ? 90 : 0))
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(expanded ? "Hide the breakdown" : "Show the breakdown")
                DiskBreakdownBar(breakdown: d, height: 6, onReview: { review = PanelReview(bucket: nil) })
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
                    .padding(.top, 2)
                    .transition(.opacity.combined(with: .move(edge: .top)))
                }
            }
        }
    }

    // MARK: Clean action / progress

    @ViewBuilder
    private func action(_ r: Report) -> some View {
        if let s = store.menuSession {
            MenuCleaningView(session: s) {
                store.menuSession = nil
                open(.backups)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            Divider()
        } else if r.notInUseBytes > 0 {
            Button { review = PanelReview(bucket: nil) } label: {
                Text("Clean everything not in use · \(Fmt.bytes(r.plan("stale")?.bytes ?? r.notInUseBytes))")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(store.busy)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .help("Review first. Only items nothing is using are included.")
            Divider()
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
        .frame(maxWidth: .infinity, minHeight: 120)
        .padding(.horizontal, 14)
    }

    @ViewBuilder
    private var status: some View {
        if store.freedThisWeek > 0 {
            Label("Freed with Poo: \(Fmt.bytes(store.freedThisWeek)) this week", systemImage: "leaf")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        if let e = store.error {
            Label(e, systemImage: "exclamationmark.triangle")
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(3)
        }
        if let next = store.schedules.filter({ $0.schedule.enabled }).compactMap({ e in e.next_run.map { (e, $0) } }).min(by: { $0.1 < $1.1 }) {
            Label("Auto-clean \(next.0.id) \(Fmt.ago(Date(timeIntervalSince1970: TimeInterval(next.1))))\(next.0.last_run.map { " · last freed \(Fmt.bytes($0.freed_bytes))" } ?? "")", systemImage: "clock.arrow.2.circlepath")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var footer: some View {
        HStack {
            Button("Settings…") {
                NSApp.activate(ignoringOtherApps: true)
                openSettings()
            }
            .keyboardShortcut(",")
            if updater.available != nil {
                Button("Update") { Task { await updater.install() } }
            }
            Spacer()
            Button("Quit") { NSApp.terminate(nil) }
                .keyboardShortcut("q")
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
                        Text(unused > 0 ? "\(Fmt.bytes(bucket.bytes)) · \(Fmt.bytes(unused)) not in use" : Fmt.bytes(bucket.bytes))
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
                        .frame(width: 112, height: 56)
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
