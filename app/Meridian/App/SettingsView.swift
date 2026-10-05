import MeridianCore
import SwiftUI

/// Data mode, provider credentials (Keychain), provider settings, and the
/// Data Sources table (capabilities, terms, attribution, purge).
struct SettingsView: View {
    @State private var mode: DataModeFfi = AppModel.preferredMode
    @State private var sources: [DataSourceFfi] = []
    @State private var status = ""

    /// (provider, field, label, isSecret)
    static let credentialFields: [(String, String, String, Bool)] = [
        ("anthropic", "api_key", "Anthropic API key (ASK)", true),
        ("alpaca", "key_id", "Alpaca key ID", true),
        ("alpaca", "secret_key", "Alpaca secret key", true),
        ("finnhub", "api_key", "Finnhub API key", true),
        ("fred", "api_key", "FRED API key", true),
    ]

    var body: some View {
        TabView {
            general.tabItem { Text("General") }
            credentials.tabItem { Text("API Keys") }
            dataSources.tabItem { Text("Data Sources") }
        }
        .padding(14)
        .frame(width: 900, height: 620)
        .onAppear { sources = (try? AppModel.shared.core?.dataSources()) ?? [] }
    }

    private var general: some View {
        Form {
            Picker("Data mode", selection: $mode) {
                Text("MOCK — synthetic data, no keys needed").tag(DataModeFfi.mock)
                Text("LIVE — real providers you configure").tag(DataModeFfi.live)
            }
            .onChange(of: mode) { _, m in
                AppModel.preferredMode = m
                status = "Restart Meridian to switch to \(m == .mock ? "MOCK" : "LIVE"). Mock and live data never mix in one session."
            }
            ProviderSettingField(provider: "edgar", key: "contact", label: "SEC EDGAR contact (name and email, sent as User-Agent — required by SEC)")
            Picker("Alpaca feed", selection: Binding(
                get: { ProviderSettingField.read("alpaca", "feed") ?? "iex" },
                set: { ProviderSettingField.write("alpaca", "feed", $0) }
            )) {
                Text("IEX (free Basic plan, single venue)").tag("iex")
                Text("SIP (Algo Trader Plus, consolidated)").tag("sip")
            }
            Text(status).font(.callout).foregroundStyle(.orange)
            Text("Changes to providers take effect after restart.").font(.callout).foregroundStyle(.secondary)
        }
    }

    private var credentials: some View {
        Form {
            Text("Keys are stored only in your macOS Keychain (service “meridian.provider.<name>”).")
                .font(.callout).foregroundStyle(.secondary)
            ForEach(Self.credentialFields, id: \.2) { f in
                SecretRow(provider: f.0, field: f.1, label: f.2)
            }
        }
    }

    private var dataSources: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                ForEach(sources, id: \.provider) { s in
                    VStack(alignment: .leading, spacing: 4) {
                        HStack {
                            Text(s.provider.uppercased()).font(.headline)
                            if let c = s.connected { Text(c ? "● connected" : "● disconnected").foregroundStyle(c ? .green : .red) }
                            Spacer()
                            Text(s.cachePolicy).foregroundStyle(.secondary)
                            Text(s.aiPolicy).foregroundStyle(.secondary)
                            Button("Delete cached data") {
                                status = (try? AppModel.shared.core?.purgeProvider(provider: s.provider)) ?? "failed"
                            }
                        }
                        Text(s.termsNote).font(.callout)
                        if let a = s.attribution { Text(a).font(.callout).italic() }
                        if !s.docsUrl.isEmpty { Text(s.docsUrl).font(.caption).foregroundStyle(.secondary).textSelection(.enabled) }
                        ForEach(Array(s.capabilities.enumerated()), id: \.offset) { _, c in
                            HStack {
                                Text(c.capability).frame(width: 190, alignment: .leading)
                                Text(c.assetClasses).frame(width: 220, alignment: .leading).foregroundStyle(.secondary)
                                Text(c.delay).frame(width: 70, alignment: .leading)
                                Text(c.source).frame(width: 110, alignment: .leading).foregroundStyle(.secondary)
                                Text(c.history ?? "").foregroundStyle(.secondary)
                            }
                            .font(.system(.caption, design: .monospaced))
                        }
                    }
                    Divider()
                }
                if !status.isEmpty { Text(status).foregroundStyle(.orange) }
            }
        }
    }
}

struct SecretRow: View {
    let provider: String
    let field: String
    let label: String
    @State private var value = ""
    @State private var saved = false

    var body: some View {
        HStack {
            SecureField(label, text: $value)
            Button("Save") {
                saved = Keychain.write(provider: provider, field: field, value: value.trimmingCharacters(in: .whitespacesAndNewlines))
                value = ""
            }
            .disabled(value.isEmpty)
            Button("Remove") { Keychain.delete(provider: provider, field: field); saved = false }
            Text(Keychain.read(provider: provider, field: field) != nil || saved ? "set" : "not set")
                .foregroundStyle(.secondary)
                .frame(width: 50)
        }
    }
}

/// Non-secret provider settings stored as JSON in SQLite.
struct ProviderSettingField: View {
    let provider: String
    let key: String
    let label: String
    @State private var value = ""

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

    var body: some View {
        TextField(label, text: $value)
            .onAppear { value = Self.read(provider, key) ?? "" }
            .onSubmit { Self.write(provider, key, value) }
    }
}
