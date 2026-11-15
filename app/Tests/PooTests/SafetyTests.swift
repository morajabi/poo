import XCTest
@testable import Poo

final class SafetyTests: XCTestCase {
    private func item(_ id: String, group: String = "Rust", bucket: String = "build", tier: String = "cold",
                      action: String = "delete", bytes: UInt64 = 100, safeguards: [String]? = nil,
                      risks: [[String: String]] = []) throws -> Item {
        var obj: [String: Any] = [
            "id": id, "path": "/tmp/\(id)", "name": id, "rule": "r", "bucket": bucket, "group": group,
            "bytes": bytes, "total_bytes": bytes, "files": 1, "tier": tier, "cost": "rebuild",
            "reasons": ["old"], "rebuild": "cargo build", "action": ["kind": action], "risks": risks,
        ]
        if let safeguards { obj["safeguards"] = safeguards }
        return try JSONDecoder().decode(Item.self, from: JSONSerialization.data(withJSONObject: obj))
    }

    func testSafeguardsGroupAcrossCounts() throws {
        let items = [
            try item("a", action: "worktree-remove", safeguards: ["Backs up 3 ignored config files first", "Its branch stays"]),
            try item("b", action: "worktree-remove", safeguards: ["Backs up 1 ignored config files first", "Its branch stays"]),
            try item("c"),
        ]
        let lines = SafeguardSummary.lines(for: items)
        XCTAssertEqual(lines.count, 2)
        let wt = try XCTUnwrap(lines.first { $0.title == "2 worktrees" })
        XCTAssertEqual(wt.detail, "Backs up ignored config files first · Its branch stays")
        XCTAssertTrue(wt.keepsBackup)
        XCTAssertEqual(lines.first { $0.id != wt.id }?.detail, "Deleted permanently; rebuild with cargo build")
    }

    func testModesAcceptOldAndNewLevels() throws {
        XCTAssertEqual(CleanMode(level: "force"), .inUse)
        XCTAssertEqual(CleanMode(level: "in-use"), .inUse)
        XCTAssertEqual(CleanMode(level: "danger"), .dangerous)
        let i = try item("x", risks: [["tag": "recent", "level": "in-use", "message": "m"]])
        XCTAssertEqual(CleanMode.needed(for: [i]), .inUse)
        XCTAssertEqual(CleanMode.inUse.flag, "--include-in-use")
        XCTAssertEqual(CleanMode.dangerous.flag, "--dangerously-include-protected")
    }

    func testThresholds() {
        XCTAssertEqual(LowDiskGuard.parseThreshold("20G", total: 0), 20_000_000_000)
        XCTAssertEqual(LowDiskGuard.parseThreshold("500MB", total: 0), 500_000_000)
        XCTAssertEqual(LowDiskGuard.parseThreshold("10%", total: 1_000), 100)
        XCTAssertNil(LowDiskGuard.parseThreshold("lots", total: 0))
    }

    func testBackupConfigDecodesPartial() throws {
        let c = try JSONDecoder().decode(BackupConfig.self, from: Data(#"{"use_trash": true}"#.utf8))
        XCTAssertEqual(c.use_trash, true)
        XCTAssertNil(c.auto_prune_days)
        let list = try JSONDecoder().decode(BackupList.self, from: Data(#"{"backups":[{"id":"a","at":1,"kind":"stash","repo":"/r","ref":"poo: x"}]}"#.utf8))
        XCTAssertEqual(list.backups.first?.symbol, "tray.and.arrow.down")
    }
}
