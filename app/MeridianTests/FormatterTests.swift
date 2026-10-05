import MeridianCore
import XCTest
@testable import Meridian

final class FormatterTests: XCTestCase {
    func testSignedAndLarge() {
        XCTAssertEqual(TerminalFormatter.string(1.5, .change(decimals: 2)), "+1.50")
        XCTAssertEqual(TerminalFormatter.string(-1.5, .changePercent(decimals: 1)), "-1.5%")
        XCTAssertEqual(TerminalFormatter.string(3_390_000_000_000, .large(decimals: 2)), "3.39T")
        XCTAssertEqual(TerminalFormatter.string(1234.5, .number(decimals: 1)), "1,234.5")
        XCTAssertEqual(TerminalFormatter.string(nil, .number(decimals: 1)), "")
        XCTAssertEqual(TerminalFormatter.string(.nan, .number(decimals: 1)), "")
    }

    func testHotRowLayoutMatchesRust() {
        // Mirrors core/crates/stream/src/row.rs LAYOUT.
        let rust: [LayoutFieldFfi] = HotRowDecoder.expected.map { LayoutFieldFfi(name: $0.key, offset: UInt32($0.value)) }
        XCTAssertNil(HotRowDecoder.verify(rust))
    }

    func testNamedArgs() {
        XCTAssertEqual(PanelModel.namedArgs("GP", ["5Y"]).first?.value, "5Y")
        XCTAssertEqual(PanelModel.namedArgs("CN", ["rate", "cut"]).first?.value, "rate cut")
        XCTAssertTrue(PanelModel.namedArgs("DES", ["X"]).isEmpty)
    }
}
