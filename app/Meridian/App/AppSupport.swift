import AppKit
import Foundation
import UserNotifications

/// Where Meridian's documentation, releases and issue tracker live.
nonisolated enum Links {
    static let repo = URL(string: "https://github.com/ankthba/meridian")!
    static let guide = URL(string: "https://github.com/ankthba/meridian/blob/main/docs/GUIDE.md")!
    static let releases = URL(string: "https://github.com/ankthba/meridian/releases")!
    static let privacy = URL(string: "https://github.com/ankthba/meridian/blob/main/PRIVACY.md")!

    static var appVersion: String {
        let v = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        let b = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?"
        return "\(v) (\(b))"
    }

    static var macOSVersion: String {
        let v = ProcessInfo.processInfo.operatingSystemVersion
        return "\(v.majorVersion).\(v.minorVersion).\(v.patchVersion)"
    }

    /// A new bug report with the app and macOS versions filled in (fields of
    /// `.github/ISSUE_TEMPLATE/bug_report.yml`). Nothing else is sent.
    static func newIssue() -> URL {
        var c = URLComponents(url: repo.appendingPathComponent("issues/new"), resolvingAgainstBaseURL: false)!
        c.queryItems = [
            URLQueryItem(name: "template", value: "bug_report.yml"),
            URLQueryItem(name: "version", value: appVersion),
            URLQueryItem(name: "macos", value: macOSVersion),
        ]
        return c.url!
    }

    @MainActor
    static func open(_ url: URL) {
        NSWorkspace.shared.open(url)
    }
}

/// Crash reports from earlier runs: the Rust core's panic reports
/// (`<data dir>/crashes`) and macOS's own reports for the app.
enum CrashReports {
    static var coreDirectory: URL { AppModel.dataDirectory.appendingPathComponent("crashes", isDirectory: true) }
    static var systemDirectory: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/DiagnosticReports", isDirectory: true)
    }

    /// Reports written after `date`, newest first.
    static func since(_ date: Date) -> [URL] {
        let fm = FileManager.default
        let keys: [URLResourceKey] = [.contentModificationDateKey]
        func list(_ dir: URL, _ match: (String) -> Bool) -> [(URL, Date)] {
            let items = (try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: keys)) ?? []
            return items.compactMap { u in
                guard match(u.lastPathComponent),
                      let d = try? u.resourceValues(forKeys: Set(keys)).contentModificationDate, d > date else { return nil }
                return (u, d)
            }
        }
        let core = list(coreDirectory) { $0.hasPrefix("rust-panic-") }
        let system = list(systemDirectory) { $0.hasPrefix("Meridian") && ($0.hasSuffix(".ips") || $0.hasSuffix(".crash")) }
        return (core + system).sorted { $0.1 > $1.1 }.map(\.0)
    }

    /// Shows the newest report in Finder, or the core's crash folder.
    static func reveal(_ reports: [URL]) {
        if reports.isEmpty {
            try? FileManager.default.createDirectory(at: coreDirectory, withIntermediateDirectories: true)
            NSWorkspace.shared.open(coreDirectory)
        } else {
            NSWorkspace.shared.activateFileViewerSelecting(reports)
        }
    }

    /// Reports since the previous launch, recording this launch.
    static func sinceLastLaunch() -> [URL] {
        let key = "lastLaunchAt"
        let last = UserDefaults.standard.double(forKey: key)
        UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: key)
        guard last > 0 else { return [] }
        return since(Date(timeIntervalSince1970: last))
    }
}

/// Alert notifications. Permission is asked when alerts are first used
/// (opening ALRT or an alert firing), not at launch.
enum Notifier {
    static func requestPermissionIfNeeded() {
        let center = UNUserNotificationCenter.current()
        center.getNotificationSettings { s in
            guard s.authorizationStatus == .notDetermined else { return }
            center.requestAuthorization(options: [.alert, .sound]) { _, _ in }
        }
    }

    static func post(title: String, body: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        let req = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        let center = UNUserNotificationCenter.current()
        center.getNotificationSettings { s in
            if s.authorizationStatus == .notDetermined {
                center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
                    if granted { center.add(req) }
                }
            } else {
                center.add(req)
            }
        }
    }
}
