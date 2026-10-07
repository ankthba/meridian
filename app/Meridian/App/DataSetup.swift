import AppKit
import SwiftUI

/// A live data source as Settings presents it: what it needs, what it
/// unlocks, and where to get a key. Meridian runs on real data only, so a
/// source without credentials leaves its screens NOT AVAILABLE.
struct DataSource: Identifiable, Hashable {
    struct KeyField: Hashable {
        let field: String
        let label: String
    }

    enum Need: Hashable {
        case nothing
        case contact
        case keys([KeyField])
    }

    /// Provider id used by the core and the Keychain service name.
    let id: String
    let name: String
    let tagline: String
    let symbol: String
    let tint: Color
    let unlocks: String
    let functions: [String]
    let need: Need
    let linkTitle: String?
    let link: String?
    let note: String?

    static let keyed: [DataSource] = [
        DataSource(
            id: "alpaca", name: "Alpaca", tagline: "US stocks", symbol: "chart.line.uptrend.xyaxis", tint: .yellow,
            unlocks: "Stock quotes and streaming, price history, market and company news, dividends and splits, ETF proxies for world indices, and option chains.",
            functions: ["DES", "GP", "GIP", "HP", "W", "MOST", "WEI", "TOP", "N", "CN", "DVD", "OMON"],
            need: .keys([KeyField(field: "key_id", label: "Key ID"), KeyField(field: "secret_key", label: "Secret key")]),
            linkTitle: "Create a free account", link: "https://alpaca.markets",
            note: "On the free plan, real-time prices come from the IEX exchange only and are labeled IEX on screen. Paper-trading keys work for market data."
        ),
        DataSource(
            id: "edgar", name: "SEC EDGAR", tagline: "Filings and financials", symbol: "doc.text.magnifyingglass", tint: .blue,
            unlocks: "Company search and descriptions, as-reported financial statements, dividends per share, and filings with an AI summary and a diff between versions.",
            functions: ["SECF", "DES", "FA", "CF", "DVD"],
            need: .contact,
            linkTitle: "SEC developer resources", link: "https://www.sec.gov/about/developer-resources",
            note: "No key. The SEC's fair-access policy requires a contact name and email, sent with each request."
        ),
        DataSource(
            id: "fred", name: "FRED", tagline: "Economic data", symbol: "building.columns", tint: .green,
            unlocks: "Economic series and the release calendar from the Federal Reserve Bank of St. Louis.",
            functions: ["ECO"],
            need: .keys([KeyField(field: "api_key", label: "API key")]),
            linkTitle: "Request a free key", link: "https://fred.stlouisfed.org/docs/api/api_key.html",
            note: nil
        ),
        DataSource(
            id: "finnhub", name: "Finnhub", tagline: "Analysts and news", symbol: "newspaper", tint: .teal,
            unlocks: "Market headlines, company news, analyst recommendations and earnings history.",
            functions: ["TOP", "N", "CN", "ANR", "ERN"],
            need: .keys([KeyField(field: "api_key", label: "API key")]),
            linkTitle: "Get a free key", link: "https://finnhub.io",
            note: nil
        ),
        DataSource(
            id: "anthropic", name: "Anthropic", tagline: "ASK analyst", symbol: "sparkles", tint: .orange,
            unlocks: "ASK, an analyst built on Claude that answers from the terminal's own data, and AI summaries of filings.",
            functions: ["ASK", "CF"],
            need: .keys([KeyField(field: "api_key", label: "API key")]),
            linkTitle: "Get an API key", link: "https://platform.claude.com/docs/en/get-api-key",
            note: "Billed per use by Anthropic. Testing the key spends no tokens."
        ),
    ]

    static let keyless: [DataSource] = [
        DataSource(id: "coinbase", name: "Coinbase", tagline: "Crypto", symbol: "bitcoinsign.circle", tint: .indigo,
                   unlocks: "Real-time crypto quotes and streaming, and price history.", functions: ["CRYP", "W", "GP"],
                   need: .nothing, linkTitle: nil, link: nil, note: nil),
        DataSource(id: "kraken", name: "Kraken", tagline: "Crypto", symbol: "bitcoinsign.circle", tint: .purple,
                   unlocks: "Real-time crypto quotes for pairs Coinbase doesn't list.", functions: ["CRYP", "W"],
                   need: .nothing, linkTitle: nil, link: nil, note: nil),
        DataSource(id: "frankfurter", name: "ECB reference rates", tagline: "FX, via Frankfurter", symbol: "eurosign.circle", tint: .mint,
                   unlocks: "Daily euro foreign-exchange reference rates and history.", functions: ["FXC", "W"],
                   need: .nothing, linkTitle: nil, link: nil, note: nil),
        DataSource(id: "treasury", name: "U.S. Treasury", tagline: "Yield curve", symbol: "percent", tint: .cyan,
                   unlocks: "The daily par yield curve, also used as the risk-free rate in option pricing.", functions: ["ECO", "OMON", "OVME"],
                   need: .nothing, linkTitle: nil, link: nil, note: nil),
        DataSource(id: "rss", name: "Press releases", tagline: "GlobeNewswire, PR Newswire, Business Wire", symbol: "megaphone", tint: .pink,
                   unlocks: "Company press releases.", functions: ["N", "CN"],
                   need: .nothing, linkTitle: nil, link: nil, note: nil),
    ]

    static var all: [DataSource] { keyed + keyless }

    var isConfigured: Bool {
        switch need {
        case .nothing: return true
        case .contact: return !(ProviderSettings.read(id, "contact") ?? "").trimmingCharacters(in: .whitespaces).isEmpty
        case .keys(let fields): return fields.allSatisfy { Keychain.read(provider: id, field: $0.field) != nil }
        }
    }

    /// Sources without which most screens are empty; Settings opens at
    /// launch while either is missing (unless turned off in General).
    static var essentialMissing: Bool {
        keyed.filter { $0.id == "alpaca" || $0.id == "edgar" }.contains { !$0.isConfigured }
    }

    static var missingCount: Int { keyed.filter { !$0.isConfigured }.count }
}

enum SourceStatus: Equatable {
    case notSet
    case untested
    case testing
    case connected(String)
    case failed(String)

    var color: Color {
        switch self {
        case .connected: .green
        case .failed: .red
        case .notSet: .orange
        case .untested, .testing: .secondary
        }
    }

    var label: String {
        switch self {
        case .notSet: "Not set up"
        case .untested: "Not tested"
        case .testing: "Testing…"
        case .connected: "Connected"
        case .failed: "Can't connect"
        }
    }
}

/// Connection status per source, and the save → swap in → test flow.
@MainActor
@Observable
final class SourcesModel {
    static let shared = SourcesModel()

    private var results: [String: SourceStatus] = [:]

    func status(_ s: DataSource) -> SourceStatus {
        if !s.isConfigured { return .notSet }
        return results[s.id] ?? .untested
    }

    /// Makes one real request against the source with the current settings.
    func test(_ s: DataSource) async {
        guard s.isConfigured else { results[s.id] = .notSet; return }
        guard let core = AppModel.shared.core else { return }
        results[s.id] = .testing
        do {
            results[s.id] = .connected(try await core.testProvider(provider: s.id))
        } catch {
            results[s.id] = .failed(error.userMessage)
        }
    }

    /// Tests every configured source not yet tested this session.
    func testUntested() async {
        await withTaskGroup(of: Void.self) { group in
            for s in DataSource.all where status(s) == .untested {
                group.addTask { await self.test(s) }
            }
        }
    }

    /// Credentials or settings for `s` changed: swap the sources in the
    /// running core (no restart), refresh open screens, then test `s`.
    func apply(_ s: DataSource) async {
        results[s.id] = nil
        AppModel.shared.sourcesChanged()
        await test(s)
    }
}

/// Non-secret provider settings, stored as JSON in the core's SQLite store.
enum ProviderSettings {
    static func read(_ provider: String, _ key: String) -> String? {
        guard let j = try? AppModel.shared.core?.providerSettings(provider: provider),
              let d = j.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return nil }
        return obj[key] as? String
    }

    static func write(_ provider: String, _ key: String, _ value: String) {
        var obj: [String: Any] = [:]
        if let j = try? AppModel.shared.core?.providerSettings(provider: provider), let d = j.data(using: .utf8),
           let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] { obj = o }
        obj[key] = value
        if let d = try? JSONSerialization.data(withJSONObject: obj), let j = String(data: d, encoding: .utf8) {
            try? AppModel.shared.core?.setProviderSettings(provider: provider, json: j)
        }
    }
}
