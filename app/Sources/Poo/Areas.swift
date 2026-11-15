import SwiftUI

/// What's not in use, grouped by area (bucket + group), shared by the overview and the panel.
/// The views that show it are interchangeable: `overviewAreaStyle` and `menuAreaStyle`
/// (user defaults) pick one, so another layout can be swapped in without touching the data.
struct AreaSummary: Identifiable, Hashable {
    var bucket: BucketSummary
    var group: String
    /// Bytes not in use (junk, unused, idle).
    var reclaim: UInt64
    /// Everything in this area, in use or not.
    var total: UInt64
    /// Items not in use.
    var count: Int
    /// A plain-language line: why it's not in use, where, and what happens if you clean it.
    var note: String
    /// A shorter line for the menu bar panel: "Build output in orbit-ios".
    var shortNote: String
    /// The items not in use, for reviewing a whole area or section.
    var items: [Item] = []

    /// "Swift build output", "Node packages", "DerivedData".
    var title: String {
        switch bucket.id {
        case "build": return "\(group) build output"
        case "deps": return "\(group) packages"
        case "caches": return group.lowercased().hasSuffix("cache") ? group : "\(group) cache"
        default: return group
        }
    }

    /// "2 folders in orbit-ios and web-dash · last used 2w ago"
    var detail: String {
        let what = "\(count) \(count == 1 ? "folder" : "folders")"
        let place = Self.place(items).map { " " + $0 } ?? ""
        let last = items.compactMap(\.lastUsedDate).max().map { " · last used \(Fmt.ago($0))" } ?? ""
        return what + place + last
    }

    /// What cleaning this area costs you, for grouping.
    var cost: AreaCost { AreaCost.of(items) }

    var id: String { Self.key(bucket: bucket.id, group: group) }
    static func key(bucket: String, group: String) -> String { bucket + "/" + group }
    static func key(_ i: Item) -> String { key(bucket: i.bucket, group: i.group) }

    /// Areas with something not in use, biggest first.
    static func notInUse(_ r: Report) -> [AreaSummary] {
        let visible = Dictionary(uniqueKeysWithValues: r.visibleBuckets.map { ($0.id, $0) })
        var totals: [String: UInt64] = [:]
        var unused: [String: [Item]] = [:]
        var buckets: [String: BucketSummary] = [:]
        for i in r.items {
            guard let b = visible[i.bucket] else { continue }
            let k = key(i)
            buckets[k] = b
            totals[k, default: 0] += i.bytes
            if i.status == .notInUse { unused[k, default: []].append(i) }
        }
        return unused.compactMap { k, items -> AreaSummary? in
            guard let b = buckets[k], let first = items.first else { return nil }
            let reclaim = items.reduce(0) { $0 + $1.bytes }
            guard reclaim > 0 else { return nil }
            return AreaSummary(bucket: b, group: first.group, reclaim: reclaim, total: totals[k] ?? reclaim,
                               count: items.count, note: note(for: items),
                               shortNote: shortNote(items), items: items)
        }
        .sorted { $0.reclaim != $1.reclaim ? $0.reclaim > $1.reclaim : $0.id < $1.id }
    }

    /// "Not used lately in 4 projects · comes back with swift build"
    static func note(for items: [Item]) -> String {
        let tiers = Set(items.map(\.tier))
        let why: String
        if tiers == [.junk] { why = "Leftovers nothing needs" }
        else if !tiers.contains(.idle) { why = "Unused for a while" }
        else { why = "Not used lately" }

        let place = place(items)

        let back: String
        if items.allSatisfy(\.isWorktree) {
            back = "branches and commits stay"
        } else if tiers == [.junk] {
            back = "nothing to rebuild"
        } else if let cmd = commonRebuild(items) {
            back = "comes back with \(cmd)"
        } else {
            back = "comes back when needed"
        }
        return [why + (place.map { " " + $0 } ?? ""), back].joined(separator: " · ")
    }

    /// "web-dash · used 2w ago"
    static func shortNote(_ items: [Item]) -> String {
        let projects = Set(items.compactMap { $0.project.map { URL(fileURLWithPath: $0).lastPathComponent } })
        let where_: String? = projects.count == 1 ? projects.first : projects.count > 1 ? "\(projects.count) projects" : nil
        let when = items.compactMap(\.lastUsedDate).max().map { "used \(Fmt.ago($0))" }
        let parts = [where_, when].compactMap { $0 }
        return parts.isEmpty ? "\(items.count) \(items.count == 1 ? "item" : "items")" : parts.joined(separator: " · ")
    }

    /// "in orbit-ios", "in a and b", "in 4 projects"
    static func place(_ items: [Item]) -> String? {
        let projects = Array(Set(items.compactMap { $0.project.map { URL(fileURLWithPath: $0).lastPathComponent } })).sorted()
        switch projects.count {
        case 0: return nil
        case 1: return "in \(projects[0])"
        case 2: return "in \(projects[0]) and \(projects[1])"
        default: return "in \(projects.count) projects"
        }
    }

    /// The rebuild command most items share, when it reads like a short command.
    private static func commonRebuild(_ items: [Item]) -> String? {
        var counts: [String: Int] = [:]
        for i in items { if let r = i.rebuild { counts[r, default: 0] += 1 } }
        guard let top = counts.max(by: { $0.value < $1.value })?.key else { return nil }
        var cmd = top
        if let cut = cmd.range(of: " (") { cmd = String(cmd[..<cut.lowerBound]) }
        if let cut = cmd.range(of: " / ") { cmd = String(cmd[..<cut.lowerBound]) }
        guard !cmd.contains(";"), !cmd.contains("<"), cmd.split(separator: " ").count <= 4 else { return nil }
        return cmd
    }
}

/// Sections of the overview, by what a clean costs you. Calm, plain titles.
enum AreaCost: Int, CaseIterable, Identifiable {
    case nothing, rebuild, redownload, worktree, data
    var id: Int { rawValue }

    var title: String {
        switch self {
        case .nothing: return "Nothing to redo"
        case .rebuild: return "Rebuilds next time you build"
        case .redownload: return "Downloads again when needed"
        case .worktree: return "Worktrees you're done with"
        case .data: return "Check before cleaning"
        }
    }

    var subtitle: String {
        switch self {
        case .nothing: return "Logs, temp files and leftovers nothing needs."
        case .rebuild: return "Build output of projects you haven't built lately. Your code isn't touched."
        case .redownload: return "Installed packages and caches. They come back on the next install."
        case .worktree: return "Extra checkouts whose work landed or went quiet. Branches and commits stay."
        case .data: return "These may hold data you made. Review each one."
        }
    }

    static func of(_ items: [Item]) -> AreaCost {
        if !items.isEmpty, items.allSatisfy(\.isWorktree) { return .worktree }
        let costs = Set(items.map(\.cost))
        if costs.contains("data") { return .data }
        if costs.contains("redownload") { return .redownload }
        if costs.contains("rebuild") { return .rebuild }
        return .nothing
    }
}

// MARK: - Grouped list (overview default), like the Storage list in System Settings

/// Areas in sections by what a clean costs, each row: icon, title, where and when, size, info.
struct AreaGroupedList: View {
    var areas: [AreaSummary]
    var onOpen: (AreaSummary) -> Void
    var onReview: ([Item]) -> Void

    var body: some View {
        let all = max(areas.reduce(0) { $0 + $1.reclaim }, 1)
        let sections = AreaCost.allCases.compactMap { c -> (AreaCost, [AreaSummary])? in
            let rows = areas.filter { $0.cost == c }
            return rows.isEmpty ? nil : (c, rows)
        }
        VStack(alignment: .leading, spacing: 14) {
            ForEach(sections, id: \.0) { cost, rows in
                let bytes = rows.reduce(0) { $0 + $1.reclaim }
                VStack(alignment: .leading, spacing: 6) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(cost.title).font(.subheadline.weight(.semibold))
                            Text(cost.subtitle).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Review \(Fmt.bytes(bytes))…") {
                            onReview(rows.flatMap(\.items).filter(\.isCleanable))
                        }
                        .controlSize(.small)
                        .help("See exactly what goes before anything is cleaned.")
                    }
                    VStack(spacing: 0) {
                        ForEach(Array(rows.enumerated()), id: \.element.id) { idx, a in
                            if idx > 0 { Divider().padding(.leading, 46) }
                            AreaListRow(area: a, share: Double(a.reclaim) / Double(all)) { onOpen(a) }
                        }
                    }
                    .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
                }
            }
        }
    }
}

struct AreaListRow: View {
    var area: AreaSummary
    /// Share of everything not in use, shown like "38%".
    var share: Double
    var onOpen: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            AreaIcon(symbol: area.bucket.symbol, color: area.bucket.swiftColor)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 5) {
                    Text(area.title).lineLimit(1)
                    Text(Self.percent(share)).foregroundStyle(.secondary).monospacedDigit()
                }
                Text(area.detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 8)
            Text(Fmt.bytes(area.reclaim))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .contentTransition(.numericText())
            Button(action: onOpen) { Image(systemName: "info.circle") }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("Show these items in Browse")
        }
        .font(.callout)
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Color.primary.opacity(hovering ? 0.04 : 0))
        .contentShape(Rectangle())
        .onTapGesture(perform: onOpen)
        .onHover { hovering = $0 }
        .help(area.note)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(area.title), \(Fmt.bytes(area.reclaim)), \(area.detail)")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction(.default, onOpen)
    }

    static func percent(_ f: Double) -> String {
        f < 0.001 ? "<0.1%" : f < 0.1 ? String(format: "%.1f%%", f * 100) : "\(Int((f * 100).rounded()))%"
    }
}

/// A small rounded tile with the bucket's symbol, like the icons in System Settings > Storage.
struct AreaIcon: View {
    var symbol: String
    var color: Color
    var size: CGFloat = 26

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
            .fill(color.gradient)
            .frame(width: size, height: size)
            .overlay(Image(systemName: symbol).font(.system(size: size * 0.5, weight: .medium)).foregroundStyle(.white))
    }
}

// MARK: - A: ranked by what you can free (overview)

/// One row per area, sorted by what it frees, with a bar scaled to the biggest.
struct AreaRankList: View {
    var areas: [AreaSummary]
    var limit = 7
    var onOpen: (AreaSummary) -> Void
    var onMore: () -> Void

    var body: some View {
        let shown = Array(areas.prefix(limit))
        let rest = areas.dropFirst(limit)
        let biggest = max(areas.first?.reclaim ?? 1, 1)
        VStack(spacing: 0) {
            ForEach(Array(shown.enumerated()), id: \.element.id) { idx, a in
                if idx > 0 { Divider().padding(.leading, 38) }
                AreaRankRow(area: a, fraction: Double(a.reclaim) / Double(biggest)) { onOpen(a) }
            }
            if !rest.isEmpty {
                Divider().padding(.leading, 38)
                Button(action: onMore) {
                    HStack {
                        Text("\(rest.count) more area\(rest.count == 1 ? "" : "s")")
                        Spacer()
                        Text(Fmt.bytes(rest.reduce(0) { $0 + $1.reclaim })).monospacedDigit()
                        Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                    }
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .padding(.leading, 28)
                    .padding(.horizontal, 10)
                    .frame(minHeight: 30)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 2)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 8))
    }
}

struct AreaRankRow: View {
    var area: AreaSummary
    var fraction: Double
    var onOpen: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 10) {
                Image(systemName: area.bucket.symbol)
                    .foregroundStyle(area.bucket.swiftColor)
                    .frame(width: 18)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(area.group).fontWeight(.medium).lineLimit(1)
                        Text(area.bucket.name).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Text(area.note)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                .frame(minWidth: 220, maxWidth: 380, alignment: .leading)
                .layoutPriority(1)
                RankBar(fraction: fraction)
                    .frame(minWidth: 60)
                VStack(alignment: .trailing, spacing: 2) {
                    Text(Fmt.bytes(area.reclaim))
                        .fontWeight(.medium)
                        .monospacedDigit()
                        .contentTransition(.numericText())
                    Text("\(area.count) item\(area.count == 1 ? "" : "s")")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .contentTransition(.numericText())
                }
                .frame(width: 76, alignment: .trailing)
                Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
            }
            .font(.callout)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(Color.primary.opacity(hovering ? 0.04 : 0))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("\(Fmt.bytes(area.reclaim)) not in use of \(Fmt.bytes(area.total)) in \(area.group). Click to browse.")
    }
}

/// A calm horizontal bar on a faint track.
struct RankBar: View {
    var fraction: Double
    var height: CGFloat = 6

    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Color.primary.opacity(0.06))
                Capsule().fill(Palette.reclaim)
                    .frame(width: max(height, g.size.width * min(max(fraction, 0), 1)))
            }
        }
        .frame(height: height)
        .animation(.smooth, value: fraction)
    }
}

// MARK: - E: pick areas, see a running total, clean once (menu bar panel)

struct AreaPicker: View {
    @Environment(Store.self) private var store
    var areas: [AreaSummary]
    var limit = 6
    var disabled: Bool
    var onReview: (Set<String>?) -> Void
    var onOpen: (AreaSummary) -> Void

    /// Rows past the limit share one checkbox.
    private static let restKey = "__rest__"

    var body: some View {
        @Bindable var store = store
        let shown = Array(areas.prefix(limit))
        let rest = Array(areas.dropFirst(limit))
        let restBytes = rest.reduce(0) { $0 + $1.reclaim }
        let selected = areas.filter { !isExcluded($0, rest: rest) }
        let all = max(areas.reduce(0) { $0 + $1.reclaim }, 1)
        let total = selected.reduce(0) { $0 + $1.reclaim }
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("\(Fmt.bytes(areas.reduce(0) { $0 + $1.reclaim })) not in use")
                    .font(.caption.weight(.semibold)).foregroundStyle(.secondary).monospacedDigit()
                Spacer()
                Button(store.menuExcludedAreas.isEmpty ? "Select none" : "Select all") {
                    store.menuExcludedAreas = store.menuExcludedAreas.isEmpty
                        ? Set(shown.map(\.id) + (rest.isEmpty ? [] : [Self.restKey])) : []
                }
                .buttonStyle(.link)
                .font(.caption)
            }
            .padding(.horizontal, 14)
            .padding(.top, 8)
            .padding(.bottom, 3)
            VStack(spacing: 0) {
                ForEach(shown) { a in
                    row(id: a.id, symbol: a.bucket.symbol, color: a.bucket.swiftColor, title: a.title,
                        detail: a.shortNote, bytes: a.reclaim, share: Double(a.reclaim) / Double(all))
                    .help(a.note)
                    .contextMenu { Button("Show in Poo") { onOpen(a) } }
                }
                if !rest.isEmpty {
                    row(id: Self.restKey, symbol: "ellipsis.circle", color: .secondary,
                        title: "\(rest.count) more area\(rest.count == 1 ? "" : "s")",
                        detail: rest.prefix(3).map(\.group).joined(separator: ", "), bytes: restBytes,
                        share: Double(restBytes) / Double(all))
                }
            }
            .padding(.horizontal, 5)
            Button { onReview(selected.count == areas.count ? nil : Set(selected.map(\.id))) } label: {
                Text(total > 0 ? "Review & clean \(Fmt.bytes(total))" : "Pick what to clean")
                    .monospacedDigit()
                    .contentTransition(.numericText())
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(disabled || total == 0)
            .animation(.snappy, value: total)
            .padding(.horizontal, 14)
            .padding(.top, 6)
            .padding(.bottom, 10)
            .help("Review first. Only items nothing is using are included.")
        }
    }

    private func isExcluded(_ a: AreaSummary, rest: [AreaSummary]) -> Bool {
        if rest.contains(where: { $0.id == a.id }) { return store.menuExcludedAreas.contains(Self.restKey) }
        return store.menuExcludedAreas.contains(a.id)
    }

    private func row(id: String, symbol: String, color: Color, title: String, detail: String, bytes: UInt64, share: Double) -> some View {
        let on = !store.menuExcludedAreas.contains(id)
        return Button {
            if on { store.menuExcludedAreas.insert(id) } else { store.menuExcludedAreas.remove(id) }
        } label: {
            HStack(spacing: 8) {
                // Colored check circles, one color per area, like Telegram's storage screen.
                Image(systemName: on ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(on ? color : Color.secondary)
                    .font(.title3)
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 4) {
                        Text(title).lineLimit(1)
                        Text(AreaListRow.percent(share)).foregroundStyle(.secondary).monospacedDigit()
                    }
                    Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 6)
                Text(Fmt.bytes(bytes))
                    .monospacedDigit()
                    .foregroundStyle(on ? .primary : .secondary)
                    .contentTransition(.numericText())
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(MenuRowStyle())
        .accessibilityAddTraits(on ? .isSelected : [])
        .accessibilityLabel("\(title), \(Fmt.bytes(bytes))")
    }
}

// MARK: - Freshness

/// "Updated 12s ago", with a small spinner while a scan or a watched change is picked up.
struct FreshnessLabel: View {
    @Environment(Store.self) private var store
    var prefix = "Updated"

    var body: some View {
        HStack(spacing: 5) {
            if store.refreshing {
                ProgressView().controlSize(.mini)
                    .transition(.opacity)
            }
            if let r = store.report {
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    Text(store.refreshing ? (prefix == "updated" ? "updating…" : "Updating…") : "\(prefix) \(Self.age(r.scannedAt, now: ctx.date))")
                        .monospacedDigit()
                }
            }
        }
        .animation(.default, value: store.refreshing)
        .help(store.watching ? "Poo is watching your folders and updates as they change." : "")
    }

    static func age(_ d: Date, now: Date) -> String {
        let s = max(0, Int(now.timeIntervalSince(d)))
        switch s {
        case ..<5: return "just now"
        case ..<60: return "\(s)s ago"
        case ..<3600: return "\(s / 60)m ago"
        case ..<86_400: return "\(s / 3600)h ago"
        default: return "\(s / 86_400)d ago"
        }
    }
}
