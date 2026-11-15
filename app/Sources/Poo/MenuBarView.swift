import SwiftUI

/// The menu bar panel: free space, buckets by size, and a way into the window.
/// Cleaning happens in the window, where there is room to review it.
struct MenuBarView: View {
    @Environment(Store.self) private var store
    @Environment(Updater.self) private var updater
    @Environment(Navigation.self) private var nav
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 14)
                .padding(.top, 12)
                .padding(.bottom, 10)
            Divider()
            if let r = store.report {
                buckets(r)
            } else if store.scanning {
                scanningView
            } else {
                ContentUnavailableView("No scan yet", systemImage: "internaldrive", description: Text("Scan to see what takes space."))
                    .frame(maxWidth: .infinity, minHeight: 140)
            }
            Divider()
            VStack(alignment: .leading, spacing: 8) {
                Button { open(.overview) } label: {
                    Text("Open Poo").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .keyboardShortcut(.defaultAction)
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
        .task { await store.loadSchedules() }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                if let r = store.report {
                    Text("\(Fmt.bytes(r.disk.free)) free")
                        .font(.title3.weight(.semibold))
                        .monospacedDigit()
                    HStack(spacing: 4) {
                        if r.notInUseBytes > 0 {
                            Text("\(Fmt.bytes(r.notInUseBytes)) not in use")
                            Text("·").foregroundStyle(.tertiary)
                        }
                        Text("scanned \(Fmt.ago(r.scannedAt))").foregroundStyle(.tertiary)
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                } else {
                    Text("Poo").font(.title3.weight(.semibold))
                }
            }
            Spacer(minLength: 8)
            if store.scanning {
                HStack(spacing: 6) {
                    if let p = store.progress {
                        Text(Fmt.bytes(p.bytes)).font(.caption).monospacedDigit().foregroundStyle(.secondary)
                    }
                    ProgressView().controlSize(.small)
                }
                .help("Scanning…")
            } else {
                Button { Task { await store.scan() } } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .help("Rescan")
            }
        }
    }

    private func buckets(_ r: Report) -> some View {
        VStack(spacing: 1) {
            ForEach(r.visibleBuckets.sorted { $0.bytes > $1.bytes }) { b in
                Button { open(.bucket(b.id)) } label: { MenuBucketRow(bucket: b) }
                    .buttonStyle(MenuRowStyle())
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
        if let last = store.lastClean, !last.dry_run {
            Label("Freed \(Fmt.bytes(last.freed_bytes)) · \(last.cleaned) cleaned", systemImage: "checkmark.circle")
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

    var body: some View {
        let unused = bucket.notInUseBytes
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: bucket.symbol)
                .foregroundStyle(bucket.swiftColor)
                .font(.body)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(bucket.name)
                    .fixedSize(horizontal: false, vertical: true)
                if unused > 0 {
                    Text("\(Fmt.bytes(unused)) not in use")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .layoutPriority(1)
            Spacer(minLength: 6)
            Text(Fmt.bytes(bucket.bytes))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .fixedSize()
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .contentShape(Rectangle())
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
