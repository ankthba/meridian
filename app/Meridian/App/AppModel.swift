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
    private(set) var mode: DataModeFfi = .mock
    private(set) var startupError: String?
    var feeds: [FeedState] = []
    var instrumentsLoaded: UInt64 = 0
    var statusMessage: String = ""
    var lastAlert: String?
    var showKeyboardOverlay = false
    let workspace = Workspace()
    let links = LinkBus()

    static var dataDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("Meridian", isDirectory: true)
    }

    /// Stored preference; changing it requires a restart (mock and live
    /// data never mix in one process).
    static var preferredMode: DataModeFfi {
        get { UserDefaults.standard.string(forKey: "dataMode") == "live" ? .live : .mock }
        set { UserDefaults.standard.set(newValue == .live ? "live" : "mock", forKey: "dataMode") }
    }

    func start() {
        guard core == nil else { return }
        Theme.registerFonts()
        let env = ProcessInfo.processInfo.environment
        let modeOverride = env["MERIDIAN_MODE"].map { $0 == "live" ? DataModeFfi.live : .mock }
        let fixedClock = env["MERIDIAN_FIXED_CLOCK_NS"].flatMap(Int64.init)
        let mode = modeOverride ?? Self.preferredMode
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
            workspace.restore(core: c)
        } catch {
            startupError = "Core failed to start: \(error)"
        }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
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
            postNotification(title: "Alert — \(security)", body: message)
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

    private func postNotification(title: String, body: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        let req = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(req)
    }
}
