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
}
