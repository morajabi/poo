import Foundation
import SwiftUI

// Mirrors the JSON contract of `poo` (src/model.rs). Unknown fields are ignored.

enum Tier: String, Codable, CaseIterable, Identifiable {
    case junk, cold, warm, hot, review, protected
    var id: String { rawValue }

    var label: String { rawValue.capitalized }

    var color: Color {
        switch self {
        case .junk: return .yellow
        case .cold: return .teal
        case .warm: return .purple
        case .hot: return .red
        case .review: return .blue
        case .protected: return .green
        }
    }

    var explanation: String {
        switch self {
        case .junk: return "Nothing to rebuild"
        case .cold: return "Unused for weeks or orphaned"
        case .warm: return "Used recently; costs a rebuild or redownload"
        case .hot: return "In use right now"
        case .review: return "Needs a human look"
        case .protected: return "Never cleaned automatically"
        }
    }

    var rank: Int { Tier.allCases.firstIndex(of: self) ?? 0 }
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
    var children: [Child]?

    var isVirtual: Bool { path.contains("://") }
    var lastUsedDate: Date? { last_used.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
    var isWorktree: Bool { action.kind == "worktree-remove" }
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

    var scannedAt: Date { Date(timeIntervalSince1970: TimeInterval(scanned_at)) }
    var visibleBuckets: [BucketSummary] { buckets.filter { !$0.hidden && $0.count > 0 } }
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
}

struct CleanResult: Codable {
    var dry_run: Bool
    var freed_bytes: UInt64
    var cleaned: Int
    var skipped: Int
    var failed: Int
    var disk_free_after: UInt64
    var results: [CleanItemResult]
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

// MARK: - Formatting

enum Fmt {
    static func bytes(_ n: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: n), countStyle: .file)
    }

    static func ago(_ date: Date?) -> String {
        guard let date else { return "–" }
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f.localizedString(for: date, relativeTo: Date())
    }

    static func tilde(_ path: String) -> String {
        let home = NSHomeDirectory()
        return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
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
