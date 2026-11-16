import SwiftUI

/// Elsewhere on This Mac: space Poo explains but leaves to its owner (macOS, other apps),
/// and the folders it can't see yet. Shown so the numbers add up; nothing here is cleaned.
struct ElsewhereView: View {
    @Environment(Store.self) private var store

    var body: some View {
        let r = store.report
        let items = store.reportOnly
        let snaps = r?.snapshots.flatMap { $0.count > 0 ? $0 : nil }
        let purgeable = store.purgeable >= 1_000_000_000 ? store.purgeable : 0
        let access = r?.warnings.filter { $0.contains("Full Disk Access") } ?? []
        List {
            if purgeable > 0 || snaps != nil {
                Section("macOS") {
                    if purgeable > 0 {
                        ElsewhereRow(symbol: "arrow.3.trianglepath", title: "Purgeable",
                                     detail: "macOS frees this on its own when it needs room", bytes: purgeable)
                    }
                    if let snaps {
                        ElsewhereRow(symbol: "clock.arrow.circlepath",
                                     title: "\(snaps.count) local Time Machine snapshot\(snaps.count == 1 ? "" : "s")",
                                     detail: "Kept until the next backup or until space runs low", bytes: snaps.bytes,
                                     settings: "x-apple.systempreferences:com.apple.Time-Machine-Settings.extension")
                    }
                }
            }
            if !items.isEmpty {
                Section("Apps and tools") {
                    ForEach(items) { i in
                        ElsewhereRow(symbol: r?.buckets.first { $0.id == i.bucket }?.symbol ?? "folder",
                                     title: i.name, detail: i.reasons.first ?? Fmt.tilde(i.path), bytes: i.total_bytes,
                                     settings: i.manage_url, path: i.isVirtual ? nil : i.path)
                    }
                }
            }
            if !access.isEmpty {
                Section("Not looked at") {
                    HStack(spacing: 10) {
                        Image(systemName: "lock.shield").foregroundStyle(.secondary).frame(width: 20)
                        VStack(alignment: .leading, spacing: 1) {
                            Text("Folders macOS guards")
                            Text("Downloads, other apps' data and more, until Poo has Full Disk Access")
                                .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                        Spacer(minLength: 8)
                        Button("Open Privacy Settings") { PrivacySettings.openFullDiskAccess() }
                            .controlSize(.small)
                    }
                    .help(access.joined(separator: "\n"))
                    .contextMenu {
                        Button("Open Privacy Settings") { PrivacySettings.openFullDiskAccess() }
                        Button("Copy Details") { Pasteboard.copy(access.joined(separator: "\n")) }
                    }
                }
            }
        }
        .listStyle(.inset)
        .overlay {
            if r != nil, items.isEmpty, snaps == nil, purgeable == 0, access.isEmpty {
                ContentUnavailableView("Nothing elsewhere", systemImage: "checkmark.circle",
                                       description: Text("Everything Poo found is in your dev folders and caches."))
            }
        }
        .navigationTitle("Elsewhere on This Mac")
        .navigationSubtitle("Not Poo's to clean")
        .task { await store.loadPurgeable() }
    }

    /// Size shown in the sidebar.
    static func bytes(_ r: Report, items reportOnly: [Item], purgeable: UInt64) -> UInt64 {
        let items = reportOnly.reduce(UInt64(0)) { $0 + $1.total_bytes }
        let snaps = r.snapshots.flatMap { $0.count > 0 ? $0.bytes : nil } ?? 0
        return items + snaps + (purgeable >= 1_000_000_000 ? purgeable : 0)
    }
}

struct ElsewhereRow: View {
    var symbol: String
    var title: String
    var detail: String
    var bytes: UInt64?
    /// A System Settings (or app) URL that manages this space.
    var settings: String? = nil
    var path: String? = nil

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol).foregroundStyle(.secondary).frame(width: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).lineLimit(1)
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 8)
            if let bytes { Text(Fmt.bytes(bytes)).monospacedDigit().foregroundStyle(.secondary) }
            if let settings {
                Button("Manage…") { open(settings) }.controlSize(.small)
            } else if let path {
                Button("Reveal") { reveal(path) }.controlSize(.small)
            }
        }
        .padding(.vertical, 2)
        .help(path.map(Fmt.tilde) ?? detail)
        .contextMenu {
            if let settings { Button("Manage…") { open(settings) } }
            if let path {
                Button("Reveal in Finder") { reveal(path) }
                Button("Copy Path") { Pasteboard.copy(path) }
            }
            if let bytes { Button("Copy Size") { Pasteboard.copy(Fmt.bytes(bytes)) } }
        }
    }

    private func open(_ s: String) {
        if let u = URL(string: s) { NSWorkspace.shared.open(u) }
    }

    private func reveal(_ p: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)])
    }
}

enum Pasteboard {
    static func copy(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }
}

enum PrivacySettings {
    static func openFullDiskAccess() {
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!)
    }
}
