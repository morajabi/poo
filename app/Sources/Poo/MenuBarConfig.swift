import Foundation
import SwiftUI

/// `[menubar]` in config.toml: what the status item and its panel show.
/// Read through `poo config get menubar`, cached for an instant first frame.
struct MenubarSettings: Codable, Equatable {
    var title = "icon"
    var warn = false
    var warn_below = "10%"
    var notify = false
    var sections = ["areas"]
    var area_order = "size"
    var max_areas = 6

    enum Part: String, CaseIterable, Identifiable {
        case icon, free, not_in_use, percent_free
        var id: String { rawValue }
        var label: String {
            switch self {
            case .icon: return "Icon"
            case .free: return "Free space"
            case .not_in_use: return "Not in use"
            case .percent_free: return "Percent free"
            }
        }
    }

    enum Section: String, CaseIterable, Identifiable {
        case health, disk, areas, buckets, freed, schedules
        var id: String { rawValue }
        var label: String {
            switch self {
            case .health: return "Growth today"
            case .disk: return "Disk bar"
            case .areas: return "Clean button and areas"
            case .buckets: return "Buckets with Clean buttons"
            case .freed: return "Freed this week"
            case .schedules: return "Next auto-clean"
            }
        }
    }

    var parts: [Part] {
        title.split(separator: "+").compactMap { Part(rawValue: $0.trimmingCharacters(in: .whitespaces)) }
    }

    func shows(_ p: Part) -> Bool { parts.contains(p) }
    func shows(_ s: Section) -> Bool { sections.contains(s.rawValue) }

    /// The parts in a stable order, joined back for config.toml.
    static func title(_ parts: Set<Part>) -> String {
        let ordered = Part.allCases.filter(parts.contains)
        return ordered.isEmpty ? "icon" : ordered.map(\.rawValue).joined(separator: "+")
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let d = MenubarSettings()
        title = (try? c.decodeIfPresent(String.self, forKey: .title)) ?? d.title
        warn = (try? c.decodeIfPresent(Bool.self, forKey: .warn)) ?? d.warn
        warn_below = (try? c.decodeIfPresent(String.self, forKey: .warn_below)) ?? d.warn_below
        notify = (try? c.decodeIfPresent(Bool.self, forKey: .notify)) ?? d.notify
        sections = (try? c.decodeIfPresent([String].self, forKey: .sections)) ?? d.sections
        area_order = (try? c.decodeIfPresent(String.self, forKey: .area_order)) ?? d.area_order
        max_areas = (try? c.decodeIfPresent(Int.self, forKey: .max_areas)) ?? d.max_areas
    }

    private static let cacheKey = "menubarConfig"

    static func cached() -> MenubarSettings {
        guard let d = UserDefaults.standard.data(forKey: cacheKey),
              let s = try? JSONDecoder().decode(MenubarSettings.self, from: d) else { return MenubarSettings() }
        return s
    }

    func cache() {
        if let d = try? JSONEncoder().encode(self) { UserDefaults.standard.set(d, forKey: Self.cacheKey) }
    }

    /// Free bytes below which the menu bar number is tinted.
    func threshold(total: UInt64) -> UInt64? {
        LowDiskGuard.parseThreshold(warn_below, total: total)
    }

    /// The status item's text, e.g. "124 GB · 25%".
    func text(for r: Report) -> String {
        var out: [String] = []
        for p in parts {
            switch p {
            case .icon: continue
            case .free: out.append(Fmt.bytes(r.disk.free))
            case .percent_free:
                let pct = r.disk.total > 0 ? Double(r.disk.free) / Double(r.disk.total) * 100 : 0
                out.append("\(Int(pct.rounded()))%")
            case .not_in_use: out.append(r.notInUseBytes > 0 ? "\(Fmt.bytes(r.notInUseBytes)) to free" : "")
            }
        }
        return out.filter { !$0.isEmpty }.joined(separator: " · ")
    }

    /// Orders areas the way the panel lists them.
    func order(_ areas: [AreaSummary]) -> [AreaSummary] {
        switch area_order {
        case "bucket": return areas.sorted { ($0.bucket.order, -Int64($0.reclaim)) < ($1.bucket.order, -Int64($1.reclaim)) }
        case "name": return areas.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
        default: return areas
        }
    }
}

/// Where the CLI keeps config.toml, resolved like `config_dir()` in the CLI.
enum ConfigFile {
    static var url: URL {
        let env = ProcessInfo.processInfo.environment
        if let d = env["POO_CONFIG_DIR"] { return URL(fileURLWithPath: d).appendingPathComponent("config.toml") }
        if let h = env["POO_HOME"] { return URL(fileURLWithPath: h).appendingPathComponent("config/config.toml") }
        if let x = env["XDG_CONFIG_HOME"] { return URL(fileURLWithPath: x).appendingPathComponent("poo/config.toml") }
        return URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".config/poo/config.toml")
    }
}
