import MeridianCore
import SwiftUI

/// Data-source setup (credentials in the Keychain, provider settings) and
/// the Data Sources table (capabilities, terms, attribution, purge).
struct SettingsView: View {
    @State private var sources: [DataSourceFfi] = []
    @State private var status = ""

    var body: some View {
        TabView {
            SetupView().tabItem { Text("Setup") }
            dataSources.tabItem { Text("Data Sources") }
        }
        .padding(14)
        .frame(width: 900, height: 680)
        .onAppear { sources = (try? AppModel.shared.core?.dataSources()) ?? [] }
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
    var onChange: () -> Void = {}
    @State private var value = ""
    @State private var saved = false

    var body: some View {
        HStack {
            // Return saves too, and so does closing Settings with text
            // still in the field, so a pasted key is never silently lost.
            SecureField(label, text: $value)
                .onSubmit(save)
            Button("Save", action: save)
                .disabled(value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Button("Remove") { Keychain.delete(provider: provider, field: field); saved = false; onChange() }
            Text(Keychain.read(provider: provider, field: field) != nil || saved ? "set" : "not set")
                .foregroundStyle(.secondary)
                .frame(width: 50)
        }
        .onDisappear(perform: save)
    }

    private func save() {
        let v = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !v.isEmpty else { return }
        saved = Keychain.write(provider: provider, field: field, value: v)
        value = ""
        onChange()
    }
}

/// Non-secret provider settings stored as JSON in SQLite.
struct ProviderSettingField: View {
    let provider: String
    let key: String
    let label: String
    var onChange: () -> Void = {}
    @State private var value = ""
    @State private var stored = ""

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
        HStack {
            TextField(label, text: $value)
                .onSubmit(save)
            Button("Save", action: save)
                .disabled(value.trimmingCharacters(in: .whitespaces) == stored)
        }
        .onAppear { value = Self.read(provider, key) ?? ""; stored = value }
        .onDisappear { if value.trimmingCharacters(in: .whitespaces) != stored { save() } }
    }

    private func save() {
        let v = value.trimmingCharacters(in: .whitespaces)
        Self.write(provider, key, v)
        stored = v
        onChange()
    }
}
