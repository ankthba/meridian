import AppKit
import MeridianCore
import SwiftUI
import UniformTypeIdentifiers

/// What opened the importer: from a portfolio (add to it) or from anywhere.
struct ImportRequest: Identifiable {
    let id = UUID()
    let portfolioId: Int64?
}

/// Import a broker's CSV export: choose the file, check what Meridian read,
/// map columns when the format isn't recognized, then import. Parsing and
/// storage happen in the core; nothing leaves this Mac.
@MainActor
@Observable
final class ImportModel {
    let portfolioId: Int64?
    var fileName = ""
    var csv: String?
    var preview: ImportPreviewFfi?
    var mapping: ImportMappingFfi?
    var addToExisting: Bool
    var newName = ""
    var busy = false
    var error: String?
    var result: ImportResultFfi?

    init(portfolioId: Int64?) {
        self.portfolioId = portfolioId
        addToExisting = portfolioId != nil
    }

    func chooseFile() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.commaSeparatedText, .plainText, UTType(filenameExtension: "csv") ?? .data]
        panel.allowsMultipleSelection = false
        panel.message = "Choose the CSV your broker exported"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        guard let data = try? Data(contentsOf: url) else {
            error = "Couldn't read \(url.lastPathComponent)."
            return
        }
        load(data, name: url.lastPathComponent)
        Task { await refresh() }
    }

    func load(_ data: Data, name: String) {
        // Brokers export UTF-8 or Windows-1252; the core expects text.
        csv = String(data: data, encoding: .utf8) ?? String(data: data, encoding: .windowsCP1252)
        fileName = name
        preview = nil
        mapping = nil
        result = nil
    }

    func refresh() async {
        guard let csv, let core = AppModel.shared.core else { return }
        busy = true
        defer { busy = false }
        do {
            let p = try await core.previewImport(csv: csv, mapping: mapping)
            preview = p
            if mapping == nil { mapping = p.suggestedMapping }
            if newName.isEmpty { newName = p.recognized ? "\(p.formatName) import" : "Imported portfolio" }
            error = nil
        } catch {
            self.error = error.userMessage
        }
    }

    func commit() async {
        guard let csv, let core = AppModel.shared.core, let preview else { return }
        busy = true
        defer { busy = false }
        do {
            let useMapping = preview.recognized && mapping == preview.suggestedMapping ? nil : mapping
            result = try await core.commitImport(
                csv: csv,
                mapping: useMapping,
                portfolioId: addToExisting ? portfolioId : nil,
                newPortfolioName: addToExisting ? nil : newName.trimmingCharacters(in: .whitespaces)
            )
            error = nil
        } catch {
            self.error = error.userMessage
        }
    }
}

struct ImportSheet: View {
    @State var model: ImportModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Form {
                if let r = model.result {
                    done(r)
                } else if let p = model.preview {
                    previewSections(p)
                } else {
                    intro
                }
                if let e = model.error {
                    Section { Text(e).foregroundStyle(.red).textSelection(.enabled) }
                }
            }
            .formStyle(.grouped)
            Divider()
            HStack {
                if model.busy { ProgressView().controlSize(.small) }
                Spacer()
                if model.result == nil {
                    Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                    if model.preview != nil {
                        Button("Choose Another File…") { model.chooseFile() }
                        Button("Import") { Task { await model.commit() } }
                            .keyboardShortcut(.defaultAction)
                            .disabled(model.busy || !canImport)
                    } else {
                        Button("Choose CSV…") { model.chooseFile() }.keyboardShortcut(.defaultAction)
                    }
                } else {
                    Button("Done") { openPortfolio(); dismiss() }.keyboardShortcut(.defaultAction)
                }
            }
            .padding(14)
        }
        .frame(width: 820, height: 640)
    }

    private var canImport: Bool {
        guard let p = model.preview, p.totalRows > 0 else { return false }
        return model.addToExisting || !model.newName.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private var intro: some View {
        Section {
            Text("Bring in your real holdings from a broker's CSV export. Meridian reads the file on this Mac, shows you what it understood, and only then imports. Re-importing an overlapping export adds just the new rows.")
                .fixedSize(horizontal: false, vertical: true)
            LabeledContent("Recognized", value: "Robinhood · Fidelity · Charles Schwab · Vanguard · positions files")
            LabeledContent("Anything else", value: "Map the columns yourself after choosing the file")
        } header: {
            Text("Import from your broker")
        } footer: {
            Text("Robinhood: Account → Reports and statements → Account activity report. Fidelity: Activity & Orders → Download. Schwab: History → Export. Vanguard: Activity → Download transactions.")
        }
    }

    @ViewBuilder
    private func previewSections(_ p: ImportPreviewFfi) -> some View {
        Section {
            LabeledContent("File", value: model.fileName)
            LabeledContent("Format", value: p.recognized ? p.formatName : "Not recognized; map the columns below")
            LabeledContent("Rows", value: "\(p.totalRows)" + (p.firstDate.map { " · \($0) to \(p.lastDate ?? $0)" } ?? ""))
            if !p.counts.isEmpty {
                LabeledContent("Contains", value: p.counts.map { "\($0.count) \($0.label.lowercased())" }.joined(separator: " · "))
            }
            if p.warningCount > 0 {
                DisclosureGroup("\(p.warningCount) row\(p.warningCount == 1 ? "" : "s") won't be imported, and why") {
                    ForEach(Array(p.warnings.prefix(60).enumerated()), id: \.offset) { _, w in
                        Text(w.line > 0 ? "Line \(w.line): \(w.message)" : w.message).font(.callout).foregroundStyle(.secondary)
                    }
                }
            }
        } header: {
            Text("What Meridian read")
        }
        if !p.recognized || model.mapping != p.suggestedMapping {
            mappingSection(p)
        }
        Section("First rows") {
            rowsTable(p.rows)
        }
        Section("Import into") {
            if model.portfolioId != nil {
                Picker("Portfolio", selection: $model.addToExisting) {
                    Text("This portfolio").tag(true)
                    Text("A new portfolio").tag(false)
                }
                .pickerStyle(.segmented)
            }
            if !model.addToExisting {
                TextField("Name", text: $model.newName)
            }
        }
    }

    private func mappingSection(_ p: ImportPreviewFfi) -> some View {
        Section {
            ForEach(MappingField.all, id: \.label) { f in
                Picker(f.label, selection: Binding(
                    get: { model.mapping.flatMap { f.get($0) }.map(Int.init) ?? -1 },
                    set: { v in
                        guard var m = model.mapping else { return }
                        f.set(&m, v < 0 ? nil : UInt32(v))
                        model.mapping = m
                    }
                )) {
                    Text("—").tag(-1)
                    ForEach(Array(p.headers.enumerated()), id: \.offset) { i, h in Text(h.isEmpty ? "Column \(i + 1)" : h).tag(i) }
                }
            }
            Toggle("Dates are day first (31/12/2026)", isOn: Binding(
                get: { model.mapping?.dayFirst ?? false },
                set: { model.mapping?.dayFirst = $0 }
            ))
            HStack {
                Spacer()
                Button("Update Preview") { Task { await model.refresh() } }
            }
        } header: {
            Text("Columns")
        } footer: {
            Text("Without a date column the file is read as a list of current holdings.")
        }
    }

    private func rowsTable(_ rows: [ImportRowFfi]) -> some View {
        Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 4) {
            GridRow {
                ForEach(["Date", "Type", "Symbol", "Quantity", "Price", "Amount"], id: \.self) { Text($0).foregroundStyle(.secondary) }
            }
            .font(.caption)
            ForEach(Array(rows.prefix(12).enumerated()), id: \.offset) { _, r in
                GridRow {
                    Text(r.tradeDate)
                    Text(r.kindLabel)
                    Text(r.symbol ?? "")
                    Text(r.quantity.map { TerminalFormatter.fixed($0, 4, grouping: true) } ?? "").gridColumnAlignment(.trailing)
                    Text(r.price.map { TerminalFormatter.fixed($0, 2) } ?? "").gridColumnAlignment(.trailing)
                    Text(r.amount.map { TerminalFormatter.fixed($0, 2) } ?? "").gridColumnAlignment(.trailing)
                }
                .font(.system(.callout, design: .monospaced))
            }
        }
    }

    private func done(_ r: ImportResultFfi) -> some View {
        Section {
            LabeledContent("Portfolio", value: r.portfolioName)
            LabeledContent("Imported", value: "\(r.imported) transaction\(r.imported == 1 ? "" : "s")")
            if r.duplicates > 0 { LabeledContent("Already there", value: "\(r.duplicates) skipped") }
            if r.warningCount > 0 { LabeledContent("Not imported", value: "\(r.warningCount) row\(r.warningCount == 1 ? "" : "s"), listed in the preview") }
        } header: {
            Text("Imported")
        } footer: {
            Text("Done opens the portfolio. Today and Calendar now use these holdings.")
        }
    }

    private func openPortfolio() {
        guard let r = model.result else { return }
        let ws = AppModel.shared.workspace
        for p in ws.panels { p.reload() }
        ws.focusedPanel.run(ActionFfi(function: "PORT", security: nil, args: [KeyValue(key: "portfolio", value: String(r.portfolioId))]))
    }
}

/// Mapping fields shown when a file isn't recognized.
private struct MappingField {
    let label: String
    let get: (ImportMappingFfi) -> UInt32?
    let set: (inout ImportMappingFfi, UInt32?) -> Void

    static let all: [MappingField] = [
        MappingField(label: "Trade date", get: \.tradeDate, set: { $0.tradeDate = $1 }),
        MappingField(label: "Symbol", get: \.symbol, set: { $0.symbol = $1 }),
        MappingField(label: "Action", get: \.action, set: { $0.action = $1 }),
        MappingField(label: "Quantity", get: \.quantity, set: { $0.quantity = $1 }),
        MappingField(label: "Price", get: \.price, set: { $0.price = $1 }),
        MappingField(label: "Amount", get: \.amount, set: { $0.amount = $1 }),
        MappingField(label: "Fees", get: \.fees, set: { $0.fees = $1 }),
        MappingField(label: "Cost basis", get: \.costBasis, set: { $0.costBasis = $1 }),
        MappingField(label: "Average cost", get: \.averageCost, set: { $0.averageCost = $1 }),
        MappingField(label: "Currency", get: \.currency, set: { $0.currency = $1 }),
        MappingField(label: "Description", get: \.description, set: { $0.description = $1 }),
    ]
}
