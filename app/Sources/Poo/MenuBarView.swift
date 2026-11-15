import SwiftUI

struct MenuBarView: View {
    @Environment(Store.self) private var store
    @Environment(Updater.self) private var updater
    @Environment(Navigation.self) private var nav
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @State private var confirming: PlanSummary?
    @State private var emergencyGB: Double = 20

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if let r = store.report {
                DiskBar(report: r)
                HStack(alignment: .center, spacing: 14) {
                    BucketDonut(buckets: r.visibleBuckets, total: r.total_bytes) { b in
                        open(.bucket(b.id))
                    }
                    .frame(width: 128, height: 128)
                    VStack(spacing: 5) {
                        ForEach(r.visibleBuckets.sorted { $0.bytes > $1.bytes }.prefix(6)) { b in
                            Button { open(.bucket(b.id)) } label: { BucketLegendRow(bucket: b) }
                                .buttonStyle(.plain)
                                .font(.callout)
                        }
                    }
                }
                Divider()
                plans(r)
                if let last = store.lastClean, !last.dry_run {
                    Label("Freed \(Fmt.bytes(last.freed_bytes)) · \(last.cleaned) cleaned, \(last.skipped) skipped", systemImage: "checkmark.circle.fill")
                        .font(.caption)
                        .foregroundStyle(.green)
                }
            } else if store.scanning {
                scanningView
            } else {
                ContentUnavailableView("No scan yet", systemImage: "internaldrive", description: Text("Scan to see what takes space."))
            }
            if let e = store.error {
                Text(e).font(.caption).foregroundStyle(.red).lineLimit(3)
            }
            Divider()
            footer
        }
        .padding(16)
        .frame(width: 380)
        .sheet(item: $confirming) { p in
            CleanConfirmView(planID: p.id, free: p.id == "emergency" ? UInt64(emergencyGB * 1e9) : nil)
                .environment(store)
        }
    }

    private var header: some View {
        HStack {
            Text("poo").font(.headline)
            if let r = store.report {
                Text("scanned \(Fmt.ago(r.scannedAt))").font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if store.scanning {
                ProgressView().controlSize(.small)
                if let p = store.progress {
                    Text(Fmt.bytes(p.bytes)).font(.caption).monospacedDigit().foregroundStyle(.secondary)
                }
            } else {
                Button { Task { await store.scan() } } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.borderless)
                    .help("Rescan")
            }
        }
    }

    private var scanningView: some View {
        VStack(alignment: .leading, spacing: 6) {
            ProgressView()
            if let p = store.progress {
                Text("\(p.phase.capitalized)… \(Fmt.bytes(p.bytes)) in \(p.files) files")
                    .font(.callout)
                Text(p.detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 120)
    }

    @ViewBuilder
    private func plans(_ r: Report) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Recommended").font(.subheadline.weight(.semibold))
            ForEach(r.plans) { p in
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(p.name).font(.callout.weight(.medium))
                        Text(p.description).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                    Spacer()
                    if p.id == "emergency" {
                        Stepper(value: $emergencyGB, in: 5...500, step: 5) {
                            Text("\(Int(emergencyGB)) GB").monospacedDigit().font(.callout)
                        }
                        .fixedSize()
                    } else {
                        Text(Fmt.bytes(p.bytes)).monospacedDigit().font(.callout)
                    }
                    Button("Clean…") { confirming = p }
                        .disabled(store.busy || (p.id != "emergency" && p.count == 0))
                }
            }
        }
    }

    private var footer: some View {
        HStack {
            Button("Open Poo") { open(.overview) }
            Button("Settings…") {
                NSApp.activate(ignoringOtherApps: true)
                openSettings()
            }
            if updater.available != nil {
                Button("Update") { Task { await updater.install() } }
                    .foregroundStyle(.blue)
            }
            Spacer()
            Button("Quit") { NSApp.terminate(nil) }
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

/// Shows exactly what a plan or a selection will clean before doing it.
struct CleanConfirmView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    var planID: String?
    var free: UInt64?
    var items: [Item]? = nil
    @State private var plan: PlanResult?
    @State private var loading = true

    private var list: [Item] { items ?? plan?.items ?? [] }
    private var total: UInt64 { items.map { $0.reduce(0) { $0 + $1.total_bytes } } ?? plan?.bytes ?? 0 }
    private var risky: Bool { list.contains { $0.tier == .hot || $0.tier == .review } }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(plan?.name ?? "Clean selected items").font(.title3.weight(.semibold))
            if let d = plan?.description { Text(d).foregroundStyle(.secondary) }
            if loading && items == nil {
                ProgressView().frame(maxWidth: .infinity, minHeight: 200)
            } else if list.isEmpty {
                Text("Nothing to clean.").foregroundStyle(.secondary).frame(maxWidth: .infinity, minHeight: 120)
            } else {
                List(list) { i in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(i.name).lineLimit(1).truncationMode(.middle)
                            Text(i.reasons.joined(separator: " · ")).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                        Spacer()
                        TierBadge(tier: i.tier)
                        Text(Fmt.bytes(i.total_bytes)).monospacedDigit().frame(width: 72, alignment: .trailing)
                    }
                }
                .frame(minHeight: 240)
                if risky {
                    Label("Includes hot or review items. Poo still skips running builds and uncommitted worktrees.", systemImage: "exclamationmark.triangle")
                        .font(.caption).foregroundStyle(.orange)
                }
            }
            HStack {
                Text("\(list.count) items · \(Fmt.bytes(total))").foregroundStyle(.secondary)
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Clean \(Fmt.bytes(total))", role: .destructive) {
                    Task {
                        if let items {
                            await store.clean(ids: items.map(\.id), force: risky)
                        } else if let planID {
                            await store.clean(plan: planID, free: free)
                        }
                        dismiss()
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(list.isEmpty || store.busy)
            }
        }
        .padding(20)
        .frame(width: 560)
        .task {
            if items == nil, let planID {
                plan = await store.plan(planID, free: free)
            }
            loading = false
        }
    }
}
