import AppKit
import SwiftUI

/// What each live data source needs and unlocks. Meridian runs on real data
/// only, so sources without credentials leave their screens NOT AVAILABLE
/// until the user adds them here.
struct SetupItem: Identifiable {
    enum Need {
        case nothing
        case contact
        case keys([(field: String, label: String)])
    }

    let id: String
    let name: String
    let unlocks: String
    let need: Need
    let link: String?
    let note: String?

    /// Sources and links. Links are the providers' own pages (cited in each
    /// provider crate's README or the vendor docs).
    static let all: [SetupItem] = [
        SetupItem(
            id: "alpaca", name: "Alpaca — US stocks",
            unlocks: "Stock quotes and streaming, price history, market and company news, option chains (DES price, GP, GIP, HP, W, MOST, TOP, CN, OMON)",
            need: .keys([("key_id", "Key ID"), ("secret_key", "Secret key")]),
            link: "https://alpaca.markets",
            note: "Free account. The free plan's real-time prices come from the IEX exchange only (labeled IEX on screen)."
        ),
        SetupItem(
            id: "edgar", name: "SEC EDGAR — filings and financials",
            unlocks: "Company search and descriptions, as-reported financials, filings with AI summary and diff (SECF, DES, FA, CF)",
            need: .contact,
            link: "https://www.sec.gov/about/developer-resources",
            note: "No key. The SEC requires a contact name and email, sent with each request."
        ),
        SetupItem(
            id: "fred", name: "FRED — economic data",
            unlocks: "Economic series and release calendar (ECO)",
            need: .keys([("api_key", "API key")]),
            link: "https://fred.stlouisfed.org/docs/api/api_key.html",
            note: "Free key from the St. Louis Fed."
        ),
        SetupItem(
            id: "finnhub", name: "Finnhub — analysts and news",
            unlocks: "Market headlines, company news, analyst recommendations, earnings history (TOP, N, CN, ANR, ERN)",
            need: .keys([("api_key", "API key")]),
            link: "https://finnhub.io",
            note: "Free tier."
        ),
        SetupItem(
            id: "anthropic", name: "Anthropic — ASK analyst",
            unlocks: "ASK and AI filing summaries",
            need: .keys([("api_key", "API key")]),
            link: "https://platform.claude.com/docs/en/get-api-key",
            note: "Billed per use by Anthropic."
        ),
        SetupItem(
            id: "keyless", name: "Always on — no key needed",
            unlocks: "Crypto from Coinbase and Kraken (CRYP), FX reference rates from the ECB via Frankfurter (FXC), US Treasury yield curve, company press releases (N → Press Releases, CN)",
            need: .nothing, link: nil, note: nil
        ),
    ]

    var isConfigured: Bool {
        switch need {
        case .nothing: return true
        case .contact:
            return !(ProviderSettingField.read(id, "contact") ?? "").trimmingCharacters(in: .whitespaces).isEmpty
        case .keys(let fields):
            return fields.allSatisfy { Keychain.read(provider: id, field: $0.field) != nil }
        }
    }

    /// Sources whose absence leaves most screens empty; Settings opens on
    /// launch while either is missing.
    static var essentialMissing: Bool {
        all.filter { $0.id == "alpaca" || $0.id == "edgar" }.contains { !$0.isConfigured }
    }

    static var missingCount: Int { all.filter { !$0.isConfigured }.count }

    /// Providers are built at launch, so credential changes apply after a
    /// restart.
    static func relaunch() {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        p.arguments = ["-c", "sleep 1; /usr/bin/open \"$0\"", Bundle.main.bundlePath]
        try? p.run()
        NSApp.terminate(nil)
    }
}

struct SetupView: View {
    @Bindable var app = AppModel.shared
    @State private var refresh = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Meridian shows real market data only. Sources below without credentials leave their screens NOT AVAILABLE. Keys are stored only in your macOS Keychain.")
                    .font(.callout).foregroundStyle(.secondary)
                ForEach(SetupItem.all) { item in
                    row(item)
                    Divider()
                }
                HStack {
                    if app.restartNeeded {
                        Text("Restart to apply changes.").foregroundStyle(.orange)
                    }
                    Spacer()
                    Button("Restart Meridian") { SetupItem.relaunch() }
                        .disabled(!app.restartNeeded)
                }
            }
            .padding(.trailing, 8)
            .id(refresh)
        }
    }

    @ViewBuilder
    private func row(_ item: SetupItem) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(item.name).font(.headline)
                Spacer()
                Text(item.isConfigured ? "● ready" : "● not set")
                    .foregroundStyle(item.isConfigured ? .green : .orange)
            }
            Text(item.unlocks).font(.callout)
            if let note = item.note { Text(note).font(.caption).foregroundStyle(.secondary) }
            if let link = item.link, let url = URL(string: link) {
                Link(link, destination: url).font(.caption)
            }
            switch item.need {
            case .nothing:
                EmptyView()
            case .contact:
                ProviderSettingField(provider: item.id, key: "contact", label: "Your name and email, e.g. Jane Doe jane@example.com") {
                    changed()
                }
            case .keys(let fields):
                ForEach(fields, id: \.field) { f in
                    SecretRow(provider: item.id, field: f.field, label: f.label) { changed() }
                }
                if item.id == "alpaca" {
                    Picker("Feed", selection: Binding(
                        get: { ProviderSettingField.read("alpaca", "feed") ?? "iex" },
                        set: { ProviderSettingField.write("alpaca", "feed", $0); changed() }
                    )) {
                        Text("IEX (free Basic plan, single exchange)").tag("iex")
                        Text("SIP (Algo Trader Plus, all exchanges)").tag("sip")
                    }
                    .frame(maxWidth: 420)
                }
            }
        }
    }

    private func changed() {
        app.restartNeeded = true
        app.missingSetup = SetupItem.missingCount
        refresh += 1
    }
}
