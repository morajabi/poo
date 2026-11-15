import XCTest
@testable import Poo

final class LiveTests: XCTestCase {
    private func item(_ id: String, bucket: String, group: String, bytes: UInt64, tier: String,
                      project: String? = nil, rebuild: String? = nil, kind: String = "delete") -> String {
        """
        {"id":"\(id)","path":"/tmp/\(id)","name":"\(id)","rule":"r","bucket":"\(bucket)","group":"\(group)",
         "bytes":\(bytes),"total_bytes":\(bytes),"files":1,"tier":"\(tier)","cost":"rebuild","reasons":[],
         \(project.map { "\"project\":\"\($0)\"," } ?? "")\(rebuild.map { "\"rebuild\":\"\($0)\"," } ?? "")
         "action":{"kind":"\(kind)"}}
        """
    }

    func reportFor(_ rows: [(String, String, String, String)]) throws -> Report {
        try report(rows.map { item($0.0, bucket: $0.1, group: $0.2, bytes: 1_000, tier: $0.3) })
    }

    private func report(_ items: [String]) throws -> Report {
        let bucket = { (id: String) in
            """
            {"id":"\(id)","name":"\(id.capitalized)","order":1,"color":"#888888","symbol":"hammer","description":"",
             "hidden":false,"bytes":1,"count":1,"tiers":{},"groups":[]}
            """
        }
        let json = """
        {"schema":2,"version":"t","scanned_at":0,"duration_ms":0,"disk":{"total":100,"free":10,"used":90},
         "roots":[],"total_bytes":0,"buckets":[\(bucket("build")),\(bucket("caches"))],"plans":[],"projects":[],
         "items":[\(items.joined(separator: ","))],"warnings":[]}
        """
        return try JSONDecoder().decode(Report.self, from: Data(json.utf8))
    }

    func testAreasRankByWhatYouCanFree() throws {
        let r = try report([
            item("a", bucket: "build", group: "Swift", bytes: 5_000, tier: "idle", project: "/p/one", rebuild: "swift build / swift test"),
            item("b", bucket: "build", group: "Swift", bytes: 3_000, tier: "cold", project: "/p/two", rebuild: "swift build / swift test"),
            item("c", bucket: "build", group: "Swift", bytes: 9_000, tier: "warm", project: "/p/three"),
            item("d", bucket: "caches", group: "Rust", bytes: 20_000, tier: "junk"),
        ])
        let areas = AreaSummary.notInUse(r)
        XCTAssertEqual(areas.map(\.id), ["caches/Rust", "build/Swift"])
        let swift = areas[1]
        XCTAssertEqual(swift.reclaim, 8_000)
        XCTAssertEqual(swift.total, 17_000)
        XCTAssertEqual(swift.count, 2)
        XCTAssertEqual(swift.note, "Not used lately in one and two · comes back with swift build")
        XCTAssertEqual(areas[0].note, "Leftovers nothing needs · nothing to rebuild")
    }

    func testFreshnessText() {
        let now = Date()
        XCTAssertEqual(FreshnessLabel.age(now, now: now), "just now")
        XCTAssertEqual(FreshnessLabel.age(now.addingTimeInterval(-12), now: now), "12s ago")
        XCTAssertEqual(FreshnessLabel.age(now.addingTimeInterval(-180), now: now), "3m ago")
    }

    func testFreeSpaceMatchesStatfs() throws {
        let f = try XCTUnwrap(FreeSpace.read())
        XCTAssertGreaterThan(f.total, 0)
        XCTAssertLessThanOrEqual(f.free, f.total)
    }

    /// The CLI writes a temp file and renames it over the report; the watcher must keep firing.
    func testReportWatcherSeesAtomicRenames() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("poo-watch-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let url = dir.appendingPathComponent("report.json")
        try Data("{}".utf8).write(to: url)
        let w = ReportFileWatcher(url: url)
        let fired = expectation(description: "fired")
        fired.expectedFulfillmentCount = 2
        fired.assertForOverFulfill = false
        let count = Counter()
        w.onChange = { if count.bump() <= 2 { fired.fulfill() } }
        w.start()
        for n in 1...2 {
            Thread.sleep(forTimeInterval: 0.4)
            let tmp = dir.appendingPathComponent("report.json.tmp")
            try Data(String(repeating: "x", count: n * 10).utf8).write(to: tmp)
            _ = try FileManager.default.replaceItemAt(url, withItemAt: tmp)
        }
        wait(for: [fired], timeout: 5)
        w.stop()
    }

    func testLineSplitter() {
        let got = Counter()
        let s = LineSplitter { line in if line.hasPrefix("{") { _ = got.bump() } }
        s.append(Data("{\"event\":\"ready\"}\n{\"ev".utf8))
        s.append(Data("ent\":\"changed\"}\npartial".utf8))
        XCTAssertEqual(got.value, 2)
    }
}

final class Counter: @unchecked Sendable {
    private var n = 0
    private let lock = NSLock()
    func bump() -> Int { lock.lock(); defer { lock.unlock() }; n += 1; return n }
    var value: Int { lock.lock(); defer { lock.unlock() }; return n }
}

extension LiveTests {
    func testAreaTitlesAndCostSections() throws {
        let r = try reportFor([
            ("a", "build", "Swift", "idle"), ("b", "caches", "Rust", "junk"),
        ])
        let areas = AreaSummary.notInUse(r)
        XCTAssertEqual(areas.first { $0.group == "Swift" }?.title, "Swift build output")
        XCTAssertEqual(areas.first { $0.group == "Rust" }?.title, "Rust cache")
        XCTAssertEqual(AreaListRow.percent(0.384), "38%")
        XCTAssertEqual(AreaListRow.percent(0.022), "2.2%")
        XCTAssertEqual(AreaListRow.percent(0.0001), "<0.1%")
    }
}
