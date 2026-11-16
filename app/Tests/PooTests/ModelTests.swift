import Sparkle
import XCTest
@testable import Poo

final class ModelTests: XCTestCase {
    /// Releases are numbered 0.1.<n> and Sparkle compares CFBundleVersion with its own
    /// comparator, so the patch must order numerically, not as text.
    func testSparkleOrdersReleaseVersions() {
        let c = SUStandardVersionComparator.default
        XCTAssertEqual(c.compareVersion("0.1.9", toVersion: "0.1.10"), .orderedAscending)
        XCTAssertEqual(c.compareVersion("0.1.10", toVersion: "0.2.0"), .orderedAscending)
        XCTAssertEqual(c.compareVersion("0.1.3", toVersion: "0.1.3"), .orderedSame)
    }

    /// Outside a bundled release build (tests, `swift run`) Sparkle must stay off.
    @MainActor func testUpdaterOffWhenUnbundled() {
        XCTAssertFalse(Updater().enabled)
    }

    func testDecodesCachedReport() throws {
        // Decodes the report the CLI wrote, when one exists on this machine.
        guard let data = try? Data(contentsOf: PooCLI.reportURL) else { return }
        let r = try JSONDecoder().decode(Report.self, from: data)
        XCTAssertFalse(r.buckets.isEmpty)
    }

    /// `erasing_bytes` is newer than the app: older CLIs leave it out, and a dry run never erases.
    @MainActor func testCleanResultErasingBytes() throws {
        let base = #"{"dry_run":false,"freed_bytes":1000,"cleaned":2,"skipped":0,"failed":0,"disk_free_after":5000,"results":[]"#
        let old = try JSONDecoder().decode(CleanResult.self, from: Data((base + "}").utf8))
        XCTAssertNil(old.erasing_bytes)
        let new = try JSONDecoder().decode(CleanResult.self, from: Data((base + #","erasing_bytes":400}"#).utf8))
        XCTAssertEqual(new.erasing_bytes, 400)

        let s = CleanSession(items: [], freeBefore: 4000)
        s.finish(new)
        XCTAssertEqual(s.erasingBytes, 400)
        XCTAssertEqual(s.freeAfter, 5000)

        var dry = new
        dry.dry_run = true
        let d = CleanSession(items: [], freeBefore: 4000)
        d.finish(dry)
        XCTAssertEqual(d.erasingBytes, 0)
        XCTAssertNil(d.freeAfter)
    }
}
