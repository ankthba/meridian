import MeridianCore
import SwiftUI

/// Settings → Data Sources: every live source with its status, credentials
/// (Keychain), a real connection test, and its terms of use.
struct DataSourcesPane: View {
    @State private var selection: DataSource.ID? = DataSource.keyed.first(where: { !$0.isConfigured })?.id ?? DataSource.keyed[0].id
    @State private var sources = SourcesModel.shared

    var body: some View {
        HStack(spacing: 0) {
            List(selection: $selection) {
                Section("Needs a key or contact") {
                    ForEach(DataSource.keyed) { row($0) }
                }
                Section("No key needed") {
                    ForEach(DataSource.keyless) { row($0) }
                }
            }
            .listStyle(.sidebar)
            .frame(width: 240)
            Divider()
            if let s = DataSource.all.first(where: { $0.id == selection }) {
                SourceDetail(source: s).id(s.id)
            } else {
                Text("Select a data source").foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task { await sources.testUntested() }
    }

    private func row(_ s: DataSource) -> some View {
        HStack(spacing: 10) {
            SourceIcon(source: s, size: 24)
            VStack(alignment: .leading, spacing: 1) {
                Text(s.name)
                Text(s.tagline).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            StatusDot(status: sources.status(s))
        }
        .padding(.vertical, 2)
        .tag(s.id)
    }
}

struct SourceIcon: View {
    let source: DataSource
    let size: CGFloat

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
            .fill(source.tint.gradient)
            .frame(width: size, height: size)
            .overlay(Image(systemName: source.symbol).font(.system(size: size * 0.52, weight: .semibold)).foregroundStyle(.white))
    }
}

struct StatusDot: View {
    let status: SourceStatus

    var body: some View {
        if status == .testing {
            ProgressView().controlSize(.mini)
        } else {
            Circle().fill(status.color).frame(width: 8, height: 8).help(status.label)
        }
    }
}

private struct SourceDetail: View {
    let source: DataSource
    @State private var sources = SourcesModel.shared
    @State private var values: [String: String] = [:]
    @State private var contact = ""
    @State private var reveal = false
    @State private var info: DataSourceFfi?
    @State private var purgeMessage: String?
    @State private var saving = false

    var body: some View {
        Form {
            Section {
                HStack(alignment: .center, spacing: 14) {
                    SourceIcon(source: source, size: 44)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(source.name).font(.title2.weight(.semibold))
                        Text(source.tagline).foregroundStyle(.secondary)
                    }
                    Spacer()
                    StatusBadge(status: sources.status(source))
                }
                Text(source.unlocks)
                FunctionChips(functions: source.functions)
                if let note = source.note {
                    Text(note).font(.callout).foregroundStyle(.secondary)
                }
            }

            switch source.need {
            case .nothing:
                EmptyView()
            case .contact:
                Section {
                    TextField("Name and email", text: $contact, prompt: Text(verbatim: "Jane Doe jane@example.com"))
                        .onSubmit { Task { await save() } }
                    saveRow(canSave: !contact.trimmingCharacters(in: .whitespaces).isEmpty)
                } header: {
                    Text("Contact")
                } footer: {
                    linkFooter
                }
            case .keys(let fields):
                Section {
                    ForEach(fields, id: \.field) { f in
                        LabeledContent(f.label) {
                            HStack(spacing: 6) {
                                Group {
                                    if reveal {
                                        TextField(f.label, text: binding(f.field), prompt: Text(placeholder(f.field)))
                                    } else {
                                        SecureField(f.label, text: binding(f.field), prompt: Text(placeholder(f.field)))
                                    }
                                }
                                .labelsHidden()
                                .textFieldStyle(.roundedBorder)
                                .onSubmit { Task { await save() } }
                            }
                        }
                    }
                    if source.id == "alpaca" {
                        Picker("Feed", selection: Binding(
                            get: { ProviderSettings.read("alpaca", "feed") ?? "iex" },
                            set: { ProviderSettings.write("alpaca", "feed", $0); Task { await sources.apply(source) } }
                        )) {
                            Text("IEX: free Basic plan, one exchange").tag("iex")
                            Text("SIP: Algo Trader Plus, all exchanges").tag("sip")
                        }
                    }
                    saveRow(canSave: values.values.contains { !$0.trimmingCharacters(in: .whitespaces).isEmpty })
                } header: {
                    HStack {
                        Text("Credentials")
                        Spacer()
                        Toggle("Show", isOn: $reveal).toggleStyle(.checkbox).font(.caption)
                    }
                } footer: {
                    linkFooter
                }
            }

            if source.need == .nothing || source.isConfigured {
                Section("Connection") {
                    HStack(alignment: .firstTextBaseline) {
                        connectionText
                        Spacer()
                        Button("Test Again") { Task { await sources.test(source) } }
                            .disabled(sources.status(source) == .testing)
                    }
                }
            }

            if let info {
                Section("Data and terms") {
                    ForEach(Array(info.capabilities.enumerated()), id: \.offset) { _, c in
                        LabeledContent(c.capability) {
                            Text([c.delay, c.source, c.history].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
                                .foregroundStyle(.secondary)
                        }
                    }
                    if !info.termsNote.isEmpty { Text(info.termsNote).font(.callout) }
                    if let a = info.attribution { Text(a).font(.callout).italic() }
                    LabeledContent("Local cache", value: info.cachePolicy)
                    LabeledContent("Use by ASK", value: info.aiPolicy)
                    if let url = URL(string: info.docsUrl), !info.docsUrl.isEmpty {
                        Link("Provider documentation", destination: url)
                    }
                    HStack {
                        Button("Delete Cached Data") {
                            purgeMessage = (try? AppModel.shared.core?.purgeProvider(provider: source.id)) ?? "Nothing to delete"
                        }
                        if let purgeMessage { Text(purgeMessage).font(.caption).foregroundStyle(.secondary) }
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onAppear {
            contact = ProviderSettings.read(source.id, "contact") ?? ""
            info = (try? AppModel.shared.core?.dataSources())?.first { $0.provider == source.id }
        }
    }

    private var linkFooter: some View {
        Group {
            if let link = source.link, let url = URL(string: link) {
                Link("\(source.linkTitle ?? "Website") ↗", destination: url).font(.callout)
            }
        }
    }

    @ViewBuilder
    private var connectionText: some View {
        switch sources.status(source) {
        case .connected(let m): Label(m, systemImage: "checkmark.circle.fill").foregroundStyle(.green)
        case .failed(let m): Label(m, systemImage: "xmark.octagon.fill").foregroundStyle(.red).textSelection(.enabled)
        case .testing: Label("Testing…", systemImage: "hourglass").foregroundStyle(.secondary)
        case .untested: Label("Not tested yet", systemImage: "circle.dashed").foregroundStyle(.secondary)
        case .notSet: Label("Not set up", systemImage: "exclamationmark.circle").foregroundStyle(.orange)
        }
    }

    private func saveRow(canSave: Bool) -> some View {
        HStack {
            if source.isConfigured {
                Button("Remove", role: .destructive) { Task { await remove() } }
            }
            Spacer()
            if saving { ProgressView().controlSize(.small) }
            Button("Save and Test") { Task { await save() } }
                .keyboardShortcut(.defaultAction)
                .disabled(!canSave || saving)
        }
    }

    private func binding(_ field: String) -> Binding<String> {
        Binding(get: { values[field] ?? "" }, set: { values[field] = $0 })
    }

    /// Shows the last four characters of a stored key so the user can tell
    /// which key is saved; the full key never leaves the Keychain view.
    private func placeholder(_ field: String) -> String {
        guard let v = Keychain.read(provider: source.id, field: field), v.count > 4 else { return "Paste key" }
        return "Saved ••••\(v.suffix(4)); paste to replace"
    }

    private func save() async {
        saving = true
        defer { saving = false }
        switch source.need {
        case .nothing:
            break
        case .contact:
            let v = contact.trimmingCharacters(in: .whitespaces)
            guard !v.isEmpty else { return }
            ProviderSettings.write(source.id, "contact", v)
        case .keys:
            var wrote = false
            for (field, value) in values {
                let v = value.trimmingCharacters(in: .whitespacesAndNewlines)
                if !v.isEmpty { wrote = Keychain.write(provider: source.id, field: field, value: v) || wrote }
            }
            values = [:]
            guard wrote else { return }
        }
        await sources.apply(source)
    }

    private func remove() async {
        switch source.need {
        case .nothing: return
        case .contact:
            ProviderSettings.write(source.id, "contact", "")
            contact = ""
        case .keys(let fields):
            for f in fields { Keychain.delete(provider: source.id, field: f.field) }
        }
        await sources.apply(source)
    }
}

private struct StatusBadge: View {
    let status: SourceStatus

    var body: some View {
        HStack(spacing: 5) {
            StatusDot(status: status)
            Text(status.label).font(.callout.weight(.medium))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .background(status.color.opacity(0.15), in: Capsule())
    }
}

/// The function mnemonics a source powers, as small monospaced chips.
private struct FunctionChips: View {
    let functions: [String]

    var body: some View {
        HStack(spacing: 4) {
            ForEach(functions, id: \.self) { f in
                Text(f)
                    .font(.system(.caption, design: .monospaced).weight(.semibold))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Color.secondary.opacity(0.15), in: RoundedRectangle(cornerRadius: 4))
            }
        }
    }
}
