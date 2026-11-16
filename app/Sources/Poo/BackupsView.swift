import AppKit
import SwiftUI

/// What Poo saved before cleaning: ignored config files, stashes of uncommitted work and
/// rescue branches. Kept until you prune them (or the auto-prune setting does).
struct BackupsView: View {
    @Environment(Store.self) private var store
    @Environment(\.openSettings) private var openSettings
    @State private var selection: Set<String> = []
    @State private var confirmPrune: [String]?
    @State private var confirmOld = false
    @State private var confirmRestore: Backup?

    private var backups: [Backup] { (store.backups?.backups ?? []).sorted { $0.at > $1.at } }

    var body: some View {
        List(selection: $selection) {
            ForEach(backups) { b in
                BackupRow(backup: b)
                    .tag(b.id)
                    .contextMenu { menu(for: b) }
            }
        }
        .listStyle(.inset)
        .onDeleteCommand { if !selection.isEmpty { confirmPrune = Array(selection) } }
        .overlay { empty }
        .topBar { header }
        .safeAreaInset(edge: .bottom, spacing: 0) { bottomBar }
        .navigationTitle("Backups")
        .toolbar {
            ToolbarItemGroup {
                Button("Prune Older Than 30 Days…") { confirmOld = true }
                    .disabled(backups.isEmpty || store.busy)
                    .help("Delete backups made more than 30 days ago")
            }
        }
        .task {
            await store.loadBackups()
            await store.loadBackupConfig()
        }
        .confirmationDialog("Prune \(confirmPrune?.count ?? 0) backup\(confirmPrune?.count == 1 ? "" : "s")?", isPresented: Binding(get: { confirmPrune != nil }, set: { if !$0 { confirmPrune = nil } })) {
            Button("Prune", role: .destructive) {
                let ids = confirmPrune ?? []
                Task { await store.pruneBackups(ids: ids); selection.subtract(ids) }
            }
        } message: {
            Text("This permanently deletes the copied files, drops the stashes and deletes the poo/ branches. It can't be undone.")
        }
        .confirmationDialog("Prune backups older than 30 days?", isPresented: $confirmOld) {
            Button("Prune", role: .destructive) { Task { await store.pruneBackups(olderThanDays: 30) } }
        } message: {
            Text("Backups from more than 30 days ago are deleted permanently. It can't be undone.")
        }
        .confirmationDialog("Restore \(confirmRestore?.title ?? "")?", isPresented: Binding(get: { confirmRestore != nil }, set: { if !$0 { confirmRestore = nil } }), presenting: confirmRestore) { b in
            Button("Restore") { Task { await store.restoreBackup(b.id) } }
        } message: { b in
            Text("Copies \(b.files.map { "\($0) files" } ?? "the files") back to \(b.source.map(Fmt.tilde) ?? "where they came from"). Files already there are never overwritten.")
        }
    }

    private var header: some View {
        let list = store.backups
        let cfg = store.backupConfig
        let total = list?.total_bytes ?? backups.reduce(0) { $0 + ($1.bytes ?? 0) }
        return HStack(alignment: .center, spacing: 12) {
            Image(systemName: "archivebox").font(.title2).foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Group {
                    if let budget = list?.budget_bytes ?? cfg?.max_backup_bytes, budget > 0 {
                        Text("\(Fmt.bytes(total)) of \(Fmt.bytes(budget))")
                    } else {
                        Text("\(Fmt.bytes(total)) of backups")
                    }
                }
                .font(.headline)
                .monospacedDigit()
                .help("Backups use this much of their budget. The oldest file backups go first past it.")
                Text(keepSentence(days: list?.auto_prune_days ?? cfg?.auto_prune_days, git: cfg?.auto_prune_git == true)).font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
            if let dir = list?.dir {
                Button("Open Folder") { NSWorkspace.shared.open(URL(fileURLWithPath: dir)) }
                    .controlSize(.small)
            }
            Button("Settings…") {
                UserDefaults.standard.set("safety", forKey: "settingsTab")
                NSApp.activate()
                openSettings()
            }
            .controlSize(.small)
            .help("Backup budget and pruning")
        }
        .lineLimit(1)
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private func keepSentence(days: Int?, git: Bool) -> String {
        if let d = days, d > 0 {
            return "Kept \(d) days\(git ? "" : ", stashes and branches until you prune them")"
        }
        return "Kept until you prune them"
    }

    @ViewBuilder
    private var empty: some View {
        if !store.backupsAvailable {
            ContentUnavailableView("Backups need a newer poo", systemImage: "archivebox",
                                   description: Text("Update the poo command-line tool to see what Poo saved before cleaning."))
        } else if store.backups != nil && backups.isEmpty {
            ContentUnavailableView("No backups yet", systemImage: "archivebox",
                                   description: Text("Before removing a worktree, Poo saves its .env files, uncommitted work and detached commits here."))
        }
    }

    @ViewBuilder
    private var bottomBar: some View {
        if !selection.isEmpty || store.backupNote != nil {
            HStack {
                if let note = store.backupNote {
                    Label(note, systemImage: "checkmark.circle").foregroundStyle(.secondary)
                }
                Spacer()
                if !selection.isEmpty {
                    Button("Prune \(selection.count) selected…") { confirmPrune = Array(selection) }
                        .disabled(store.busy)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .floatingBar()
        }
    }

    @ViewBuilder
    private func menu(for b: Backup) -> some View {
        if b.kind == "files", let path = b.path {
            Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
            Button("Restore…") { confirmRestore = b }.disabled(b.exists == false)
        }
        if let ref = b.ref {
            Button("Copy \(b.kind == "stash" ? "Stash Message" : "Branch Name")") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(ref, forType: .string)
            }
        }
        Divider()
        Button("Prune…") { confirmPrune = selection.contains(b.id) ? Array(selection) : [b.id] }
    }
}

struct BackupRow: View {
    @Environment(Store.self) private var store
    var backup: Backup
    @State private var confirmRestore = false

    var body: some View {
        let b = backup
        HStack(spacing: 10) {
            Image(systemName: b.symbol)
                .foregroundStyle(.secondary)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(b.title).fontWeight(.medium)
                    Text(b.kindLabel).font(.caption2).padding(.horizontal, 5).padding(.vertical, 1).background(.quaternary, in: Capsule())
                    if b.exists == false {
                        Text("Missing").font(.caption2).foregroundStyle(.secondary)
                    }
                }
                Text(detail(b))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 2) {
                Text(Fmt.ago(b.date)).font(.callout).foregroundStyle(.secondary)
                    .help(b.date.formatted(date: .abbreviated, time: .shortened))
                if b.kind == "files" {
                    Text([b.files.map { "\($0) file\($0 == 1 ? "" : "s")" }, b.bytes.map(Fmt.bytes)].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption).monospacedDigit().foregroundStyle(.secondary)
                }
            }
            if b.kind == "files", let path = b.path {
                Button { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) } label: { Image(systemName: "magnifyingglass") }
                    .buttonStyle(.borderless)
                    .help("Reveal in Finder")
                    .accessibilityLabel("Reveal in Finder")
                    .disabled(b.exists == false)
                Button("Restore") { confirmRestore = true }
                    .controlSize(.small)
                    .disabled(b.exists == false || store.busy)
            }
        }
        .padding(.vertical, 3)
        .confirmationDialog("Restore \(b.title)?", isPresented: $confirmRestore) {
            Button("Restore") { Task { await store.restoreBackup(b.id) } }
        } message: {
            Text("Copies \(b.files.map { "\($0) files" } ?? "the files") back to \(b.source.map(Fmt.tilde) ?? "where they came from"). Files already there are never overwritten.")
        }
    }

    private func detail(_ b: Backup) -> String {
        var parts: [String] = []
        if let s = b.source { parts.append("from \(Fmt.tilde(s))") }
        if b.kind != "files" {
            if let r = b.repo { parts.append("in \(Fmt.tilde(r))") }
            if let ref = b.ref { parts.append(ref) }
        }
        return parts.joined(separator: " · ")
    }
}
