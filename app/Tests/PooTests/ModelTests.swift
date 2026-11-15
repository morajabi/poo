import XCTest
@testable import Poo

final class ModelTests: XCTestCase {
    func testVersionCompare() {
        XCTAssertTrue(Updater.isNewer("0.2.0", than: "0.1.9"))
        XCTAssertFalse(Updater.isNewer("0.1.0", than: "0.1.0"))
        XCTAssertTrue(Updater.isNewer("v1.0.0", than: "0.9"))
    }

    func testDecodesCachedReport() throws {
        // Decodes the report the CLI wrote, when one exists on this machine.
        guard let data = try? Data(contentsOf: PooCLI.reportURL) else { return }
        let r = try JSONDecoder().decode(Report.self, from: data)
        XCTAssertFalse(r.buckets.isEmpty)
    }
}
