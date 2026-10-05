import MeridianCore
import Testing
@testable import Meridian

@Suite struct FormatterTests {
    @Test func signedAndLarge() {
        #expect(TerminalFormatter.string(1.5, .change(decimals: 2)) == "+1.50")
        #expect(TerminalFormatter.string(-1.5, .changePercent(decimals: 1)) == "-1.5%")
        #expect(TerminalFormatter.string(3_390_000_000_000, .large(decimals: 2)) == "3.39T")
        #expect(TerminalFormatter.string(1234.5, .number(decimals: 1)) == "1,234.5")
        #expect(TerminalFormatter.string(-0.004, .number(decimals: 2)) == "0.00")
        #expect(TerminalFormatter.string(nil, .number(decimals: 1)) == "")
        #expect(TerminalFormatter.string(.nan, .number(decimals: 1)) == "")
    }

    @Test func fastTimeIsLocalHHMMSS() {
        let s = TerminalFormatter.fastTime(1_791_216_000_000_000_000)
        #expect(s.count == 8 && s.filter { $0 == ":" }.count == 2)
    }

    @Test func hotRowLayoutMatchesRust() {
        // Mirrors core/crates/stream/src/row.rs LAYOUT.
        let rust: [LayoutFieldFfi] = HotRowDecoder.expected.map { LayoutFieldFfi(name: $0.key, offset: UInt32($0.value)) }
        #expect(HotRowDecoder.verify(rust) == nil)
    }

    @Test func namedArgs() {
        #expect(PanelModel.namedArgs("GP", ["5Y"]).first?.value == "5Y")
        #expect(PanelModel.namedArgs("CN", ["rate", "cut"]).first?.value == "rate cut")
        #expect(PanelModel.namedArgs("DES", ["X"]).isEmpty)
    }

    @Test func coreErrorsReadAsSentences() {
        #expect(CoreError.NotAvailable(reason: "Alpaca API key not set — add it in Settings").userMessage == "Alpaca API key not set — add it in Settings")
        #expect(CoreError.NotFound(what: "ZZZZ US Equity").userMessage == "ZZZZ US Equity not found")
    }
}
