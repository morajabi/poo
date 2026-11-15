import SwiftUI

/// Shows exactly what a plan or a selection will clean before doing it, with every risk
/// the chosen mode accepts spelled out.
struct CleanConfirmView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    var planID: String?
    var free: UInt64?
    var items: [Item]? = nil
    /// Limits a plan to one bucket (menu bar "Clean" on a bucket row).
    var bucket: String? = nil
    /// Limits a plan to these areas (bucket/group keys), from the menu bar picker.
    var areas: Set<String>? = nil
    /// When set, the caller shows the progress (menu bar panel) and the sheet closes on start.
    var onStart: ((CleanSession) -> Void)? = nil
    @State private var plan: PlanResult?
    @State private var loading = true
    @State private var mode: CleanMode = .safe
    @State private var dangerAck = false
    @State private var session: CleanSession?
    var onShowBackups: (() -> Void)? = nil

    private var list: [Item] {
        let all = items ?? plan?.items ?? []
        guard let areas else { return all }
        return all.filter { areas.contains(AreaSummary.key($0)) }
    }
    private var needed: CleanMode { CleanMode.needed(for: list) }
    private var allowed: [Item] {
        list.filter { i in (i.risks ?? []).allSatisfy { r in (r.mode ?? .safe).rank <= mode.rank } }
    }
    private var total: UInt64 { allowed.reduce(0) { $0 + $1.total_bytes } }

    private var title: String {
        if let areas {
            let names = Set((plan?.items ?? []).filter { areas.contains(AreaSummary.key($0)) }.map(\.group)).sorted()
            let what = names.count == 1 ? names[0] : names.count == 2 ? "\(names[0]) and \(names[1])" : "\(areas.count) areas"
            return "\(plan?.name ?? "Not in use") · \(what)"
        }
        return bucket.flatMap { b in store.report?.buckets.first { $0.id == b }?.name }.map { "\(plan?.name ?? "Not in use") · \($0)" } ?? plan?.name ?? "Clean selected items"
    }

    var body: some View {
        if let session {
            CleaningView(session: session, onShowBackups: onShowBackups) { dismiss() }
                .interactiveDismissDisabled(session.phase == .running)
        } else {
            review
        }
    }

    private var review: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title).font(.title3.weight(.semibold))
            if let d = plan?.description { Text(d).foregroundStyle(.secondary) }
            if loading && items == nil {
                ProgressView().frame(maxWidth: .infinity, minHeight: 200)
            } else if list.isEmpty {
                Text("Nothing to clean.").foregroundStyle(.secondary).frame(maxWidth: .infinity, minHeight: 120)
            } else {
                List(list) { i in
                    let blocked = !allowed.contains(i)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack {
                            Text(i.name).lineLimit(1).truncationMode(.middle)
                            Spacer()
                            TierBadge(tier: i.tier)
                            Text(Fmt.bytes(i.total_bytes)).monospacedDigit().frame(width: 72, alignment: .trailing)
                        }
                        Text(([i.comesBack] + i.reasons).joined(separator: " · ")).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        ForEach((i.risks ?? []).filter(\.isWarning), id: \.tag) { r in
                            Label(r.message, systemImage: r.isDangerous ? "exclamationmark.octagon.fill" : "exclamationmark.triangle.fill")
                                .font(.caption)
                                .foregroundStyle(r.isDangerous ? .red : .orange)
                        }
                    }
                    .opacity(blocked ? 0.45 : 1)
                    .help(blocked ? "Skipped in this mode" : "")
                }
                .frame(minHeight: 200)
                WhatHappensView(items: allowed)
                if needed != .safe {
                    VStack(alignment: .leading, spacing: 6) {
                        Picker("Include", selection: $mode) {
                            ForEach(CleanMode.allCases.filter { $0.rank <= needed.rank }) { m in
                                Text(m.label).tag(m)
                            }
                        }
                        .pickerStyle(.radioGroup)
                        Text(mode.help).font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        if mode == .dangerous {
                            Toggle("I understand these items may hold work, data or running builds", isOn: $dangerAck)
                                .toggleStyle(.checkbox).font(.caption)
                        }
                    }
                }
            }
            HStack {
                Text("\(allowed.count) of \(list.count) items · \(Fmt.bytes(total))").foregroundStyle(.secondary)
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Clean \(Fmt.bytes(total))", role: .destructive) {
                    // Clean exactly what was reviewed, in the chosen mode.
                    let s = store.startClean(allowed, mode: mode)
                    if let onStart { onStart(s); dismiss() } else { session = s }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(allowed.isEmpty || store.busy || (mode == .dangerous && !dangerAck))
            }
        }
        .padding(20)
        .frame(width: 620)
        .task {
            if items == nil, let planID {
                plan = await store.plan(planID, free: free, bucket: bucket)
            }
            loading = false
        }
    }
}

/// "What happens": what poo backs up, keeps or deletes, aggregated across the items.
struct WhatHappensView: View {
    var items: [Item]

    /// "Deleted permanently; comes back with: cargo build" → "Rebuild: cargo build"
    static func short(_ d: String) -> String {
        for p in ["Deleted permanently; comes back with: ", "Deleted permanently; rebuild with "] where d.hasPrefix(p) {
            return "Rebuild: " + d.dropFirst(p.count)
        }
        return d == "Deleted permanently" ? "Nothing to rebuild" : d
    }

    var body: some View {
        let lines = SafeguardSummary.lines(for: items)
        // Say "deleted permanently" once instead of on every line.
        let allDelete = !lines.isEmpty && lines.allSatisfy { $0.detail.hasPrefix("Deleted permanently") }
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text("What happens").font(.headline)
                if allDelete {
                    Text("Deleted permanently, not moved to the Trash").font(.caption).foregroundStyle(.secondary)
                }
            }
            if lines.isEmpty {
                Text("Nothing will be cleaned in this mode.").font(.callout).foregroundStyle(.secondary)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(lines) { l in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Image(systemName: l.symbol)
                                    .foregroundStyle(l.symbol == "checkmark.shield" ? Color.green : Color.secondary)
                                    .frame(width: 16)
                                VStack(alignment: .leading, spacing: 1) {
                                    HStack {
                                        Text(l.title).font(.callout.weight(.medium))
                                        Spacer()
                                        Text(Fmt.bytes(l.bytes)).font(.caption).monospacedDigit().foregroundStyle(.secondary)
                                    }
                                    Text(allDelete ? Self.short(l.detail) : l.detail).font(.caption).foregroundStyle(.secondary)
                                        .fixedSize(horizontal: false, vertical: true)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 190)
                .fixedSize(horizontal: false, vertical: true)
                if lines.contains(where: \.keepsBackup) {
                    Label("Backups stay in Poo › Backups until you prune them.", systemImage: "archivebox")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }
}

enum SafeguardSummary {
    struct Line: Identifiable, Equatable {
        var id: String
        var title: String
        var detail: String
        var symbol: String
        var bytes: UInt64
        var count: Int
        var keepsBackup: Bool
    }

    /// What poo says it does for an item, or what its action implies on older CLIs.
    static func sentences(for item: Item) -> [String] {
        if let s = item.safeguards, !s.isEmpty { return s }
        switch item.action.kind {
        case "worktree-remove": return ["Removed with git worktree remove"]
        case "trash": return ["Moves to the Trash"]
        case "command":
            // Keep the command's verb words, not the ids and paths it takes.
            let words = (item.action.command ?? []).prefix { $0.range(of: #"^[a-z][a-z-]*$"#, options: .regularExpression) != nil }
            return ["Removed with \(words.joined(separator: " "))"]
        default:
            return [item.rebuild.map { "Deleted permanently; rebuild with \($0)" } ?? "Deleted permanently"]
        }
    }

    /// Sentences that say something is saved or left in place.
    static func isProtective(_ text: String) -> Bool {
        let t = text.lowercased()
        return ["backs up", "backed up", "backup", "stash", "branch", "kept", "stays", "keeps"].contains { t.contains($0) }
    }

    static func symbol(for sentence: String, rebuild: String?) -> String {
        let t = sentence.lowercased()
        if isProtective(t) { return "checkmark.shield" }
        if t.contains("trash") { return "trash" }
        if rebuild != nil || t.contains("rebuild") { return "hammer" }
        return "xmark.bin"
    }

    static func title(for item: Item, count: Int) -> String {
        if item.isWorktree { return "\(count) worktree\(count == 1 ? "" : "s")" }
        return "\(item.group) · \(count) item\(count == 1 ? "" : "s")"
    }

    /// Drops per-item counts ("Backs up 3 files" -> "Backs up files") so items group together.
    static func normalize(_ s: String) -> String {
        var out = s.replacingOccurrences(of: #"\b\d+\s+"#, with: "", options: .regularExpression)
        while out.hasSuffix(".") { out.removeLast() }
        return out
    }

    static func lines(for items: [Item]) -> [Line] {
        var groups: [String: [Item]] = [:]
        var order: [String] = []
        for i in items {
            let kind = i.isWorktree ? "worktree" : i.group
            let key = kind + "|" + sentences(for: i).map(normalize).joined(separator: "|")
            if groups[key] == nil { order.append(key) }
            groups[key, default: []].append(i)
        }
        return order.map { key in
            let members = groups[key]!
            let first = members[0]
            let raw = sentences(for: first)
            let shown = members.count == 1 ? raw.map { s in s.hasSuffix(".") ? String(s.dropLast()) : s } : raw.map(normalize)
            let text = shown.joined(separator: " · ").lowercased()
            let symbol = isProtective(text) ? "checkmark.shield" : text.contains("trash") ? "trash" : (first.rebuild != nil || text.contains("rebuild") || text.contains("comes back")) ? "hammer" : "xmark.bin"
            return Line(
                id: key,
                title: title(for: first, count: members.count),
                detail: shown.joined(separator: " · "),
                symbol: symbol,
                bytes: members.reduce(0) { $0 + $1.total_bytes },
                count: members.count,
                keepsBackup: ["backs up", "backed up", "backup", "stash", "poo/"].contains { text.contains($0) }
            )
        }
        .sorted { $0.bytes > $1.bytes }
    }
}
