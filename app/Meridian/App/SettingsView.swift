import AppKit
import MeridianCore
import SwiftUI

/// The Settings window: data sources, general behavior, storage, the key
/// map, and about / data credits.
struct SettingsView: View {
    enum Tab: Hashable { case sources, general, storage, keyboard, about }
    @State private var tab: Tab = .sources

    var body: some View {
        TabView(selection: $tab) {
            DataSourcesPane()
                .tabItem { Label("Data Sources", systemImage: "antenna.radiowaves.left.and.right") }
                .tag(Tab.sources)
            GeneralPane()
                .tabItem { Label("General", systemImage: "gearshape") }
                .tag(Tab.general)
            StoragePane()
                .tabItem { Label("Storage", systemImage: "internaldrive") }
                .tag(Tab.storage)
            KeyboardPane()
                .tabItem { Label("Keyboard", systemImage: "keyboard") }
                .tag(Tab.keyboard)
            AboutPane()
                .tabItem { Label("About", systemImage: "info.circle") }
                .tag(Tab.about)
        }
        .frame(width: 860, height: 600)
    }
}

/// UserDefaults keys shared by Settings and the shell.
enum Preference {
    static let openSetupAtLaunch = "openSetupAtLaunch"
    static let restoreWorkspace = "restoreWorkspace"
    static let checkForUpdates = "checkForUpdates"
}

struct GeneralPane: View {
    @AppStorage(Preference.openSetupAtLaunch) private var openSetupAtLaunch = true
    @AppStorage(Preference.restoreWorkspace) private var restoreWorkspace = true
    @AppStorage(Preference.checkForUpdates) private var checkForUpdates = true
    @State private var resetDone = false

    var body: some View {
        Form {
            Section("Startup") {
                Toggle("Restore panels from the last session", isOn: $restoreWorkspace)
                Toggle("Open Settings when stock data or SEC filings aren't set up", isOn: $openSetupAtLaunch)
            }
            Section {
                LabeledContent("Panel layout") {
                    HStack {
                        if resetDone { Text("Reset").foregroundStyle(.secondary) }
                        Button("Reset to Default") {
                            AppModel.shared.workspace.resetToDefaults()
                            resetDone = true
                        }
                    }
                }
                LabeledContent("Launchpad") {
                    Button("Open Launchpad") { NotificationCenter.default.post(name: .openLaunchpad, object: nil) }
                }
            } header: {
                Text("Panels")
            } footer: {
                Text("The default layout opens Today, a worksheet, a price graph and the filings inbox. Panes in link group A follow each other's security.")
            }
            Section {
                Toggle("Check for updates automatically", isOn: $checkForUpdates)
                    .onChange(of: checkForUpdates) { _, on in
                        if on { UpdateChecker.shared.start() } else { UpdateChecker.shared.stop() }
                    }
                LabeledContent("Version \(UpdateChecker.currentVersion)") {
                    Button("Check Now") { Task { await UpdateChecker.shared.check(userInitiated: true) } }
                        .disabled(UpdateChecker.shared.checking)
                }
            } header: {
                Text("Updates")
            } footer: {
                Text("Asks GitHub for the latest release at launch and once a day. Nothing about you or your data is sent.")
            }
            Section("Help") {
                LabeledContent("Function directory") {
                    Button("Open HELP") {
                        let ws = AppModel.shared.workspace
                        ws.focusedPanel.run(ActionFfi(function: "HELP", security: nil, args: []))
                    }
                }
                LabeledContent("Keyboard reference") {
                    Button("Show") { AppModel.shared.showKeyboardOverlay = true }
                }
            }
            Section {
                LabeledContent("Data mode", value: AppModel.shared.mode == .live ? "Live: real data only" : "Mock: synthetic test data")
            } footer: {
                Text("Meridian always runs on real data. Screens whose source isn't set up say NOT AVAILABLE with the reason instead of filling in.")
            }
        }
        .formStyle(.grouped)
    }
}

struct StoragePane: View {
    @State private var size: String = "Calculating…"
    @State private var sources: [DataSourceFfi] = []
    @State private var message: String?

    private var dir: URL { AppModel.dataDirectory }

    var body: some View {
        Form {
            Section {
                LabeledContent("Location") {
                    Text(dir.path(percentEncoded: false)).textSelection(.enabled).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
                LabeledContent("Size on disk", value: size)
                HStack {
                    Spacer()
                    Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([dir]) }
                }
            } header: {
                Text("Data folder")
            } footer: {
                Text("Cached market data (DuckDB and Parquet), watchlists, workspaces, alerts, portfolios and ASK history. API keys are stored only in your macOS Keychain, never here.")
            }
            Section {
                ForEach(sources, id: \.provider) { s in
                    LabeledContent(DataSource.all.first { $0.id == s.provider }?.name ?? s.provider) {
                        HStack {
                            Text(s.cachePolicy).foregroundStyle(.secondary)
                            Button("Delete") { purge([s.provider]) }
                        }
                    }
                }
                HStack {
                    if let message { Text(message).font(.callout).foregroundStyle(.secondary) }
                    Spacer()
                    Button("Delete All Cached Market Data", role: .destructive) { purge(sources.map(\.provider)) }
                }
            } header: {
                Text("Cached market data")
            } footer: {
                Text("Deleting a cache only removes local copies; data is fetched again when needed. Some providers' terms limit how long data may be kept, and the cache follows them.")
            }
        }
        .formStyle(.grouped)
        .onAppear {
            sources = (try? AppModel.shared.core?.dataSources()) ?? []
            refreshSize()
        }
    }

    private func purge(_ providers: [String]) {
        var notes: [String] = []
        for p in providers {
            if let m = try? AppModel.shared.core?.purgeProvider(provider: p) { notes.append(m) }
        }
        message = providers.count == 1 ? notes.first : "Deleted cached data for \(providers.count) sources"
        refreshSize()
    }

    private func refreshSize() {
        let dir = self.dir
        Task.detached(priority: .utility) {
            let text = ByteCountFormatter.string(fromByteCount: Self.allocatedSize(of: dir), countStyle: .file)
            await MainActor.run { size = text }
        }
    }

    nonisolated private static func allocatedSize(of dir: URL) -> Int64 {
        var total: Int64 = 0
        guard let e = FileManager.default.enumerator(at: dir, includingPropertiesForKeys: [.totalFileAllocatedSizeKey]) else { return 0 }
        while let url = e.nextObject() as? URL {
            total += Int64((try? url.resourceValues(forKeys: [.totalFileAllocatedSizeKey]).totalFileAllocatedSize) ?? 0)
        }
        return total
    }
}

struct KeyboardPane: View {
    var body: some View {
        Form {
            Section {
                ForEach(KeyboardOverlay.rows, id: \.0) { r in
                    LabeledContent(r.0) {
                        Text(r.1).font(.system(.body, design: .monospaced)).foregroundStyle(.secondary)
                    }
                }
            } header: {
                Text("Keys")
            } footer: {
                Text("Mac F-keys send media keys unless you hold fn or turn on “Use F1, F2, etc. keys as standard function keys” in System Settings → Keyboard. ⌥1–⌥0 always work for the sector keys. HELP once opens help for the screen in the focused panel. Commands: type a security, a yellow key and a function, then Return, e.g. AAPL US <EQUITY> DES.")
            }
        }
        .formStyle(.grouped)
    }
}

struct AboutPane: View {
    @State private var attributions: [String] = []
    @State private var showingAcknowledgements = false

    private var version: String {
        let v = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        let b = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?"
        return "Version \(v) (\(b))"
    }

    var body: some View {
        Form {
            Section {
                HStack(spacing: 16) {
                    Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 84, height: 84)
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Meridian").font(.largeTitle.weight(.semibold))
                        Text("A market terminal for macOS, driven from a command line.").foregroundStyle(.secondary)
                        Text(version).font(.callout).foregroundStyle(.secondary)
                    }
                }
                .padding(.vertical, 6)
                LabeledContent("Source code") { Link("github.com/ankthba/meridian", destination: URL(string: "https://github.com/ankthba/meridian")!) }
                LabeledContent("Author") { Link("Aniketh Bandlamudi · aniketh.net", destination: URL(string: "https://aniketh.net/projects/")!) }
            }
            Section {
                Text("Market data from Alpaca, SEC EDGAR, FRED, Finnhub, Coinbase, Kraken, the European Central Bank via Frankfurter, the U.S. Treasury, and GlobeNewswire, PR Newswire and Business Wire press-release feeds. Each provider's terms apply to data fetched with your keys.")
                ForEach(attributions, id: \.self) { Text($0).italic() }
            } header: {
                Text("Data credits")
            }
            Section {
                LabeledContent("Open-source software") {
                    Button("Acknowledgements") { showingAcknowledgements = true }
                }
            } footer: {
                Text("Copyright and license notices for the Rust crates and C and C++ libraries built into Meridian.")
            }
            Section {
                Text("For personal use. Meridian doesn't redistribute market data and isn't affiliated with any market-data or terminal vendor.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $showingAcknowledgements) { AcknowledgementsSheet() }
        .onAppear {
            let all = (try? AppModel.shared.core?.dataSources()) ?? []
            var seen = Set<String>()
            attributions = all.compactMap(\.attribution).filter { seen.insert($0).inserted }
        }
    }
}

/// The bundled third-party notices (Resources/ThirdPartyNotices.txt, written
/// by scripts/third-party-notices.sh), read-only and selectable.
struct AcknowledgementsSheet: View {
    @Environment(\.dismiss) private var dismiss

    private static let notices: String? = Bundle.main.url(forResource: "ThirdPartyNotices", withExtension: "txt")
        .flatMap { try? String(contentsOf: $0, encoding: .utf8) }

    var body: some View {
        VStack(spacing: 0) {
            if let notices = Self.notices {
                NoticesTextView(text: notices)
            } else {
                Text("NOT AVAILABLE — ThirdPartyNotices.txt is missing from the app bundle")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            Divider()
            HStack {
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
        .frame(width: 760, height: 560)
        .onExitCommand { dismiss() }
    }
}

/// A scrollable, selectable, non-editable text view. AppKit's text system
/// handles the ~230 KB notices file far faster than a SwiftUI Text.
private struct NoticesTextView: NSViewRepresentable {
    let text: String

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        guard let view = scroll.documentView as? NSTextView else { return scroll }
        view.isEditable = false
        view.isSelectable = true
        view.isRichText = false
        view.font = .monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        view.textContainerInset = NSSize(width: 12, height: 12)
        view.string = text
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {}
}
