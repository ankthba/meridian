import AppKit
import MeridianCore
import SwiftUI

@main
struct MeridianApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        Window("Meridian", id: "main") {
            MainWindow()
                .frame(minWidth: 1100, minHeight: 680)
        }
        .defaultSize(width: 1680, height: 1000)
        .commands {
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") { Task { await UpdateChecker.shared.check(userInitiated: true) } }
            }
            CommandGroup(replacing: .newItem) {
                Button("Import Portfolio…") { NotificationCenter.default.post(name: .openImport, object: nil) }
                    .keyboardShortcut("i", modifiers: [.command, .shift])
            }
            CommandMenu("Terminal") {
                Button("Today") { run("TODAY") }
                Button("Calendar") { run("CALENDAR") }
                Button("Filings") { run("FILINGS") }
                Button("Portfolio") { run("PORT") }
                Divider()
                Button("Launchpad") { NotificationCenter.default.post(name: .openLaunchpad, object: nil) }
                    .keyboardShortcut("l", modifiers: [.command, .shift])
                Button("Keyboard Reference") { AppModel.shared.showKeyboardOverlay.toggle() }
                    .keyboardShortcut("/", modifiers: .command)
            }
            CommandGroup(replacing: .help) {
                Button("Meridian Guide") { Links.open(Links.guide) }
                Button("Commands and Functions") { run("HELP") }
                Divider()
                Button("Release Notes") { Links.open(Links.releases) }
                Button("Privacy") { Links.open(Links.privacy) }
                Divider()
                Button("Report an Issue…") { Links.open(Links.newIssue()) }
                Button("Show Crash Reports") { CrashReports.reveal(CrashReports.since(.distantPast)) }
            }
        }

        WindowGroup("Launchpad", id: "launchpad", for: String.self) { _ in
            LaunchpadView()
                .frame(minWidth: 900, minHeight: 600)
        }
        .defaultSize(width: 1600, height: 950)

        Settings {
            SettingsView()
        }
    }
}

/// Runs a function in the focused pane (menu commands).
@MainActor
private func run(_ function: String) {
    AppModel.shared.workspace.focusedPanel.run(ActionFfi(function: function, security: nil, args: []))
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        MainActor.assumeIsolated {
            NSApp.appearance = NSAppearance(named: .darkAqua)
            AppModel.shared.start()
            KeyRouter.install()
            if SnapshotMode.enabled { SnapshotMode.run() }
            if PerfHarness.enabled { PerfHarness.run() }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        MainActor.assumeIsolated {
            AppModel.shared.shutdown()
            LaunchpadModel.shared.save()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
