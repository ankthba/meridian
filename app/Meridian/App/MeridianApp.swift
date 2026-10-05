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
            CommandGroup(replacing: .newItem) {}
            CommandMenu("Terminal") {
                Button("Launchpad") { NotificationCenter.default.post(name: .openLaunchpad, object: nil) }
                    .keyboardShortcut("l", modifiers: [.command, .shift])
                Button("Keyboard Reference") { AppModel.shared.showKeyboardOverlay.toggle() }
                    .keyboardShortcut("/", modifiers: .command)
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
