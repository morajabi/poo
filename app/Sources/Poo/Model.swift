import Foundation
import SwiftUI

// Mirrors the JSON contract of `poo` (src/model.rs). Unknown fields are ignored.

enum Tier: String, Codable, CaseIterable, Identifiable {
    case junk, cold, idle, warm, hot, review, protected
    var id: String { rawValue }

    var label: String {
        switch self {
        case .junk: return "Junk"
        case .cold: return "Unused"
        case .idle: return "Idle"
        case .warm: return "In use"
        case .hot: return "Active now"
        case .review: return "Check first"
        case .protected: return "Kept"
        }
    }

    /// Calm colors: cleanable tiers share a soft green/teal family, in-use tiers are neutral.
    var color: Color {
        switch self {
        case .junk: return Color(red: 0.20, green: 0.62, blue: 0.50)
        case .cold: return .teal
        case .idle: return Color(red: 0.30, green: 0.62, blue: 0.62)
        case .warm, .hot, .protected: return .secondary
        case .review: return Color(red: 0.33, green: 0.52, blue: 0.85)
        }
    }

    var symbol: String? {
        switch self {
        case .protected: return "lock.fill"
        case .hot: return "bolt.fill"
        default: return nil
        }
    }

    var explanation: String {
        switch self {
        case .junk: return "Leftovers nothing needs. Nothing to rebuild."
        case .cold: return "Unused for days, orphaned, or its work already landed. Safe to clean."
        case .idle: return "Nothing is using it right now. Costs a rebuild if you come back."
        case .warm: return "You are working with this. Poo leaves it alone unless you include items in use."
        case .hot: return "A build or app is using this right now. Poo leaves it alone."
        case .review: return "Poo isn't sure about this one. Take a look before cleaning."
        case .protected: return "Kept on purpose. Poo never cleans it on its own."
        }
    }

    var status: ItemStatus {
        switch self {
        case .junk, .cold, .idle: return .notInUse
        case .warm, .hot: return .inUse
        case .review, .protected: return .needsLook
        }
    }

    var rank: Int { Tier.allCases.firstIndex(of: self) ?? 0 }
}

/// The three plain-language states the app shows instead of seven tiers.
enum ItemStatus: String, CaseIterable, Identifiable, Hashable {
    case notInUse, inUse, needsLook
    var id: String { rawValue }

    var label: String {
        switch self {
        case .notInUse: return "Not in use"
        case .inUse: return "In use"
        case .needsLook: return "Needs a look"
        }
    }

    var symbol: String {
        switch self {
        case .notInUse: return "moon.zzz"
        case .inUse: return "hammer"
        case .needsLook: return "eye"
        }
    }

    var tiers: [Tier] { Tier.allCases.filter { $0.status == self } }
}

struct DiskInfo: Codable, Hashable {
    var total: UInt64
    var free: UInt64
    var used: UInt64
    var usedFraction: Double { total > 0 ? Double(used) / Double(total) : 0 }
}

struct GroupSummary: Codable, Hashable, Identifiable {
    var name: String
    var bytes: UInt64
    var count: Int
    var id: String { name }
}

struct BucketSummary: Codable, Hashable, Identifiable {
    var id: String
    var name: String
    var order: Int
    var color: String
    var symbol: String
    var description: String
    var hidden: Bool
    var bytes: UInt64
    var count: Int
    var tiers: [String: UInt64]
    var groups: [GroupSummary]

    var swiftColor: Color { Color(hex: color) }
    func bytes(for tier: Tier) -> UInt64 { tiers[tier.rawValue] ?? 0 }
    func bytes(for status: ItemStatus) -> UInt64 { status.tiers.reduce(0) { $0 + bytes(for: $1) } }
    var notInUseBytes: UInt64 { bytes(for: .notInUse) }
}

struct PlanSummary: Codable, Hashable, Identifiable {
    var id: String
    var name: String
    var description: String
    var bytes: UInt64
    var count: Int
    var target_bytes: UInt64?
}

struct ItemAction: Codable, Hashable {
    var kind: String
    var command: [String]?
}

struct Risk: Codable, Hashable {
    var tag: String
    var level: String
    var message: String

    var mode: CleanMode? { CleanMode(level: level) }
    var isWarning: Bool { (mode ?? .safe) != .safe }
    var isDangerous: Bool { mode == .dangerous }
}

/// How much risk a clean may take; mirrors the CLI's risk levels.
enum CleanMode: String, CaseIterable, Identifiable {
    case safe, inUse = "in-use", dangerous
    var id: String { rawValue }
    var rank: Int { CleanMode.allCases.firstIndex(of: self) ?? 0 }

    /// Accepts the old level names too ("standard", "force", "danger").
    init?(level: String) {
        switch level {
        case "safe", "standard": self = .safe
        case "in-use", "force": self = .inUse
        case "dangerous", "danger": self = .dangerous
        default: return nil
        }
    }

    var label: String {
        switch self {
        case .safe: return "Only safe items"
        case .inUse: return "Include items in use"
        case .dangerous: return "Dangerously include protected"
        }
    }

    var help: String {
        switch self {
        case .safe: return "Cleans only what nothing is using. Anything with a warning is skipped."
        case .inUse: return "Also cleans recently used items and checkouts an agent has open; uncommitted work is stashed first."
        case .dangerous: return "Also running builds, data and protected items; detached commits are saved to a poo/ branch first."
        }
    }

    var flag: String? {
        switch self {
        case .safe: return nil
        case .inUse: return "--include-in-use"
        case .dangerous: return "--dangerously-include-protected"
        }
    }

    static func needed(for items: [Item]) -> CleanMode {
        items.flatMap { $0.risks ?? [] }.compactMap(\.mode).max { $0.rank < $1.rank } ?? .safe
    }
}

struct Child: Codable, Hashable {
    var name: String
    var bytes: UInt64
}

struct Item: Codable, Hashable, Identifiable {
    var id: String
    var path: String
    var name: String
    var rule: String
    var bucket: String
    var group: String
    var tags: [String]?
    var bytes: UInt64
    var total_bytes: UInt64
    var files: UInt64
    var last_used: Int64?
    var tier: Tier
    var cost: String
    var reasons: [String]
    var rebuild: String?
    var project: String?
    var repo: String?
    var agent: String?
    var action: ItemAction
    var risks: [Risk]?
    var children: [Child]?
    /// What poo does before or while cleaning this item (backups, stashes, Trash).
    var safeguards: [String]?
    /// Bytes a clean actually returns (APFS private size); clones and hard links share the rest.
    var frees: UInt64?
    /// Shown for awareness only (iOS backups, snapshots, Messages): poo never cleans it.
    var report_only: Bool?
    /// Where the owner manages it, e.g. an x-apple.systempreferences: URL.
    var manage_url: String?

    var status: ItemStatus { tier.status }
    /// Report-only items are never cleanable, whatever action a CLI attaches.
    var isCleanable: Bool { action.kind != "none" && report_only != true }
    var isVirtual: Bool { path.contains("://") }
    var lastUsedDate: Date? { last_used.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    var isWorktree: Bool { action.kind == "worktree-remove" }
    var isReportOnly: Bool { report_only == true }
    /// What cleaning gives back: the APFS private size when the CLI knows it.
    var freeable: UInt64 { frees ?? bytes }
    /// "frees 3.1 GB" is worth showing when sharing makes it differ by 10% and 50 MB.
    var freesDiffers: Bool {
        guard let f = frees else { return false }
        let d = f > total_bytes ? f - total_bytes : total_bytes - f
        return d >= 50_000_000 && Double(d) >= Double(total_bytes) * 0.1
    }

    /// How a clean comes back, the top question about cleaners: answered from cost, tier and rebuild.
    var comesBack: String {
        if isReportOnly { return "Managed by its app" }
        if isWorktree { return "Branch and commits stay" }
        if group.localizedCaseInsensitiveContains("device support") { return "Until you reconnect a device" }
        switch cost {
        case "none": return "Nothing to rebuild"
        case "data": return "Holds data, can't come back"
        case "redownload":
            if bucket == "deps" { return "Until the next install" }
            // Caches you use regrow on their own; cold ones wait until you need them.
            return tier == .warm || tier == .hot || tier == .idle ? "Regrows within days" : "Until you need it again"
        default:
            return tier == .junk ? "Nothing to rebuild" : "Until you rebuild"
        }
    }

    /// The most useful one-liner for a row. The age has its own column, so a reason that only
    /// says "last used 3h ago" gives way to what cleaning costs ("Rebuild: cargo build").
    var why: String {
        let r = reasons.first { !$0.hasPrefix("last used") && !$0.hasPrefix("used in the last") }
        if let r { return r }
        if let rebuild { return "\(comesBack) · \(rebuild)" }
        return comesBack
    }
}

struct Project: Codable, Hashable, Identifiable {
    var path: String
    var name: String
    var kind: String
    var repo: String?
    var agent: String?
    var branch: String?
    var stacks: [String]
    var last_activity: Int64?
    var artifact_bytes: UInt64
    var item_count: Int
    var policy: String
    var status: String
    var dirty: Bool?
    var merged: Bool?
    var recency_rank: Int?
    var active: [String]?
    var id: String { path }
}

struct Report: Codable {
    var schema: Int
    var version: String
    var scanned_at: Int64
    var duration_ms: UInt64
    var disk: DiskInfo
    var roots: [String]
    var total_bytes: UInt64
    var buckets: [BucketSummary]
    var plans: [PlanSummary]
    var projects: [Project]
    var items: [Item]
    var warnings: [String]
    var detected: [String]?
    var rule_hits: [String: [UInt64]]?
    /// How much more the disk holds than before (C18), when the CLI tracks it.
    var growth: Growth?
    /// Local Time Machine snapshots (C3), when the CLI reports them.
    var snapshots: Snapshots?

    var scannedAt: Date { Date(timeIntervalSince1970: TimeInterval(scanned_at)) }
    var visibleBuckets: [BucketSummary] { buckets.filter { !$0.hidden && $0.count > 0 } }
    /// Bytes nothing is using right now (junk, cold and idle) across visible buckets.
    var notInUseBytes: UInt64 { visibleBuckets.reduce(0) { $0 + $1.notInUseBytes } }
    func bytes(for status: ItemStatus) -> UInt64 { visibleBuckets.reduce(0) { $0 + $1.bytes(for: status) } }
    func plan(_ id: String) -> PlanSummary? { plans.first { $0.id == id } }
}

struct Growth: Codable, Hashable {
    /// Change in used bytes over the last day and week (negative after cleanups).
    var day_bytes: Int64?
    var week_bytes: Int64?
}

struct Snapshots: Codable, Hashable {
    var count: Int
    var bytes: UInt64?
    var names: [String]?
}

struct ScanProgress: Codable {
    var phase: String
    var detail: String
    var files: UInt64
    var bytes: UInt64
    var items: Int
    var elapsed_ms: UInt64
}

struct CleanItemResult: Codable, Identifiable {
    var id: String
    var path: String
    var name: String
    var status: String
    var bytes: UInt64
    var reason: String?
    var warnings: [String]?
    var notes: [String]?
}

struct CleanResult: Codable {
    var dry_run: Bool
    var freed_bytes: UInt64
    var cleaned: Int
    var skipped: Int
    var failed: Int
    var disk_free_after: UInt64
    var results: [CleanItemResult]
    var snapshot_note: String?
    var cancelled: Bool?
    var disk_free_before: UInt64?
}

struct HistoryEntry: Codable {
    var at: Int64
    var bytes: UInt64
}

struct HistoryList: Codable {
    var history: [HistoryEntry]
}

/// One `clean-progress` line from `poo clean --progress`.
struct CleanProgressEvent: Codable {
    var done: Int
    var total: Int
    var freed_bytes: UInt64
    var total_bytes: UInt64?
    var current: String?
    var status: String?
}

struct PlanResult: Codable {
    var plan: String
    var name: String
    var description: String
    var bytes: UInt64
    var count: Int
    var target_bytes: UInt64?
    var items: [Item]
}

struct CatalogBucket: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var order: Int
    var color: String
    var symbol: String
    var description: String
    var hidden: Bool
    var source: String
}

struct CatalogRule: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var bucket: String
    var group: String
    var kind: String
    var enabled: Bool
    var cost: String
    var tier: Tier?
    var warm_days: Int
    var description: String
    var recommend: String?
    var source: String
    var tags: [String]?
    var stacks: [String]?
}

struct CatalogMod: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var version: String
    var description: String
    var builtin: Bool
    var enabled: Bool
    var path: String?
    var rules: Int
}

struct Catalog: Codable {
    var mods: [CatalogMod]
    var buckets: [CatalogBucket]
    var rules: [CatalogRule]
    var errors: [String]
    /// Whether the CLI can read folders macOS guards (Mail, Safari, app containers).
    var full_disk_access: Bool?
}

/// An agent `poo ask` can open with a brief (`poo ask --list`).
struct Launcher: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var kind: String?
    var available: Bool
}

struct LauncherList: Codable {
    var launchers: [Launcher]
    var `default`: String?
    var skill_installed: Bool?
}

/// `poo prompt <target>`: a brief an agent can start from.
struct AgentPrompt: Codable {
    var title: String?
    var prompt: String
    var cwd: String?
}

struct ScheduleSpec: Codable, Hashable {
    var id: String
    var plan: String
    var every: String
    var at: String?
    var when_free_below: String?
    var target_free: String?
    var max_bytes: String?
    var enabled: Bool
}

struct ScheduleRun: Codable, Hashable {
    var at: Int64
    var status: String
    var freed_bytes: UInt64
    var cleaned: Int
    var skipped: Int
    var message: String?
}

struct ScheduleEntry: Codable, Identifiable, Hashable {
    var schedule: ScheduleSpec
    var last_run: ScheduleRun?
    var next_run: Int64?
    var id: String { schedule.id }
}

struct ScheduleList: Codable {
    var schedules: [ScheduleEntry]
    var agent_loaded: Bool?
}

struct SchedulePreset: Codable, Identifiable, Hashable {
    var id: String
    var description: String
}

struct PresetList: Codable {
    var presets: [SchedulePreset]
}

struct IntegrationTarget: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var detected: Bool
    var skill_installed: Bool
    var skill_current: Bool
    var mcp_registered: Bool
}

struct IntegrationStatus: Codable {
    var targets: [IntegrationTarget]
}

struct RootsInfo: Codable {
    var roots: [String]
    var detected: Bool
}

struct Backup: Codable, Identifiable, Hashable {
    var id: String
    var at: Int64
    var kind: String
    var label: String?
    var source: String?
    var item: String?
    var path: String?
    var repo: String?
    var ref: String?
    var files: UInt64?
    var bytes: UInt64?
    var exists: Bool?

    var date: Date { Date(timeIntervalSince1970: TimeInterval(at)) }
    var title: String { label ?? source.map { URL(fileURLWithPath: $0).lastPathComponent } ?? id }

    var symbol: String {
        switch kind {
        case "files": return "doc.on.doc"
        case "stash": return "tray.and.arrow.down"
        case "branch": return "arrow.triangle.branch"
        default: return "archivebox"
        }
    }

    var kindLabel: String {
        switch kind {
        case "files": return "Config files"
        case "stash": return "Stash"
        case "branch": return "Branch"
        default: return kind.capitalized
        }
    }
}

struct BackupList: Codable {
    var backups: [Backup]
    var total_bytes: UInt64?
    var dir: String?
    var budget_bytes: UInt64?
    var auto_prune_days: Int?
}

/// `[backup]` in config.toml. Defaults mirror the CLI's.
struct BackupConfig: Codable, Equatable {
    var config_files: Bool? = true
    var max_file_bytes: UInt64? = 1_000_000
    var max_total_bytes: UInt64? = 50_000_000
    var max_backup_bytes: UInt64? = 2_000_000_000
    var stash_dirty: Bool? = true
    var rescue_branches: Bool? = true
    var use_trash: Bool? = false
    var auto_prune_days: Int? = 90
    var auto_prune_git: Bool? = false
}

struct HookTarget: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var detected: Bool?
    var installed: Bool?
    var file: String?
}

struct HookStatus: Codable {
    var targets: [HookTarget]
    var on_agent_done: String?
    var remove_landed_worktrees: Bool?
}

// MARK: - Formatting

enum Fmt {
    private static let byteFormatter: ByteCountFormatter = {
        let f = ByteCountFormatter()
        f.countStyle = .file
        f.allowsNonnumericFormatting = false
        return f
    }()

    static func bytes(_ n: UInt64) -> String {
        n == 0 ? "0 KB" : byteFormatter.string(fromByteCount: Int64(clamping: n))
    }

    /// Made once: list rows call `ago` for every row on every render.
    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    static func ago(_ date: Date?) -> String {
        guard let date else { return "–" }
        return relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    private static let home = NSHomeDirectory()

    static func tilde(_ path: String) -> String {
        path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
    }

    /// "1 item", "3 items".
    static func count(_ n: Int, _ noun: String, plural: String? = nil) -> String {
        "\(n) \(n == 1 ? noun : plural ?? noun + "s")"
    }
}

extension Color {
    init(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        var v: UInt64 = 0
        Scanner(string: s).scanHexInt64(&v)
        self.init(
            .sRGB,
            red: Double((v >> 16) & 0xFF) / 255,
            green: Double((v >> 8) & 0xFF) / 255,
            blue: Double(v & 0xFF) / 255
        )
    }
}
