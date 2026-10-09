import AppKit
import MeridianCore
import Observation
import UserNotifications

/// Receives core events on a Rust thread and forwards them to the main actor.
nonisolated final class CoreEventSink: CoreEvents, @unchecked Sendable {
    func onEvent(event: CoreEventFfi) {
        Task { @MainActor in AppModel.shared.handle(event) }
    }
}

struct FeedState: Identifiable, Equatable {
    var id: String { provider }
    var provider: String
    var connected: Bool
    var message: String
}

/// App-wide state: the Rust core, data mode, feeds, and the workspace.
@MainActor
@Observable
final class AppModel {
    static let shared = AppModel()

    private(set) var core: Core?
    private(set) var mode: DataModeFfi = .live
    private(set) var startupError: String?
    var feeds: [FeedState] = []
    var instrumentsLoaded: UInt64 = 0
    var statusMessage: String = ""
    var lastAlert: String?
    var showKeyboardOverlay = false
    /// Crash reports written since the previous launch (shown once in the top bar).
    var crashReports: [URL] = []
    let workspace = Workspace()
    let links = LinkBus()

    static var dataDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("Meridian", isDirectory: true)
    }

    /// Live sources without credentials (status bar); computed once the core
    /// is up, since the EDGAR contact lives in the core's settings store.
    var missingSetup = 0

    func start() {
        guard core == nil else { return }
        Theme.registerFonts()
        let env = ProcessInfo.processInfo.environment
        // Real data only. The synthetic feed is reachable solely through
        // MERIDIAN_MODE=mock, for snapshot tests and the perf harness; it is
        // never offered in the UI and is labeled MOCK DATA when running.
        let mode: DataModeFfi = env["MERIDIAN_MODE"] == "mock" ? .mock : .live
        let fixedClock = env["MERIDIAN_FIXED_CLOCK_NS"].flatMap(Int64.init)
        let config = CoreConfigFfi(
            mode: mode,
            dataDir: Self.dataDirectory.path,
            inMemory: env["MERIDIAN_IN_MEMORY"] == "1",
            fixedClockNs: fixedClock,
            mockSeed: 42,
            mockExtraSymbols: UInt32(env["MERIDIAN_MOCK_EXTRA"].flatMap(Int.init) ?? 0),
            mockUpdateRate: Double(env["MERIDIAN_MOCK_RATE"] ?? "") ?? 2.0
        )
        do {
            try FileManager.default.createDirectory(at: Self.dataDirectory, withIntermediateDirectories: true)
            let c = try Core(config: config, secrets: KeychainSecretSource(), events: CoreEventSink())
            if let layout = try? c.hotRowLayout(), let mismatch = HotRowDecoder.verify(layout) {
                startupError = mismatch
                return
            }
            try c.start()
            core = c
            self.mode = (try? c.dataMode()) ?? mode
            if self.mode == .live { missingSetup = DataSource.missingCount }
            workspace.restore(core: c)
        } catch {
            startupError = "Core failed to start: \(error)"
        }
        // Notification permission is asked when alerts are first used
        // (Notifier), not at launch.
        if mode == .live, !SnapshotMode.enabled, !PerfHarness.enabled {
            crashReports = CrashReports.sinceLastLaunch()
            UpdateChecker.shared.start()
        }
    }

    /// Credentials or provider settings changed in Settings: swap the data
    /// sources in the running core and re-run every open screen.
    func sourcesChanged() {
        guard let core else { return }
        do {
            try core.reloadProviders()
        } catch {
            statusMessage = "Couldn't apply data source changes: \(error.userMessage)"
        }
        missingSetup = DataSource.missingCount
        for p in workspace.panels { p.reload() }
    }

    func shutdown() {
        workspace.save()
        try? core?.shutdown()
    }

    func handle(_ e: CoreEventFfi) {
        switch e {
        case let .feedStatus(provider, connected, message):
            if let i = feeds.firstIndex(where: { $0.provider == provider }) {
                feeds[i] = FeedState(provider: provider, connected: connected, message: message)
            } else {
                feeds.append(FeedState(provider: provider, connected: connected, message: message))
            }
        case let .alertFired(_, security, message, _):
            lastAlert = message
            Notifier.post(title: "Alert — \(security)", body: message)
        case let .universeLoaded(n):
            instrumentsLoaded = n
        case let .status(message):
            statusMessage = message
        case let .show(function, security, args):
            // ASK pushes a function into the panel after the focused one.
            let ws = workspace
            let target = ws.panels[(ws.focused + 1) % ws.panels.count]
            target.run(ActionFfi(function: function, security: security, args: args))
        }
    }
}
