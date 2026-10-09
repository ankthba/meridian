import AppKit
import Foundation

/// Looks for a newer release on GitHub: at launch when enabled (Settings →
/// General), daily while running, and from Meridian → Check for Updates….
/// One unauthenticated request to api.github.com; nothing about the user or
/// their data is sent.
@MainActor
@Observable
final class UpdateChecker {
    static let shared = UpdateChecker()
    static let latestRelease = URL(string: "https://api.github.com/repos/ankthba/meridian/releases/latest")!

    struct Release: Equatable {
        let version: String
        let url: URL
    }

    private(set) var available: Release?
    private(set) var checking = false
    private var timer: Task<Void, Never>?

    static var currentVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
    }

    static var automatic: Bool {
        UserDefaults.standard.object(forKey: Preference.checkForUpdates) as? Bool ?? true
    }

    /// Starts automatic checks (launch, then daily) if they're enabled.
    func start() {
        timer?.cancel()
        guard Self.automatic else { return }
        timer = Task { [weak self] in
            while !Task.isCancelled {
                await self?.check(userInitiated: false)
                try? await Task.sleep(nanoseconds: 86_400 * 1_000_000_000)
            }
        }
    }

    func stop() {
        timer?.cancel()
        timer = nil
    }

    func check(userInitiated: Bool) async {
        guard !checking else { return }
        checking = true
        defer { checking = false }
        do {
            var req = URLRequest(url: Self.latestRelease, timeoutInterval: 20)
            req.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            req.setValue("Meridian/\(Self.currentVersion)", forHTTPHeaderField: "User-Agent")
            let (data, response) = try await URLSession.shared.data(for: req)
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            guard status == 200 else { throw UpdateError.http(status) }
            let latest = try JSONDecoder().decode(Latest.self, from: data)
            let version = latest.tagName.hasPrefix("v") ? String(latest.tagName.dropFirst()) : latest.tagName
            available = Self.isNewer(version, than: Self.currentVersion) ? Release(version: version, url: latest.htmlUrl) : nil
            if userInitiated { present() }
        } catch {
            if userInitiated {
                let a = NSAlert()
                a.messageText = "Couldn't check for updates"
                a.informativeText = (error as? UpdateError)?.text ?? error.localizedDescription
                a.runModal()
            }
        }
    }

    private func present() {
        let a = NSAlert()
        if let r = available {
            a.messageText = "Meridian \(r.version) is available"
            a.informativeText = "You have \(Self.currentVersion). The release page has the download and what's new."
            a.addButton(withTitle: "Open Release Page")
            a.addButton(withTitle: "Later")
            if a.runModal() == .alertFirstButtonReturn { NSWorkspace.shared.open(r.url) }
        } else {
            a.messageText = "Meridian is up to date"
            a.informativeText = "\(Self.currentVersion) is the latest release."
            a.runModal()
        }
    }

    /// Numeric dotted-version comparison ("1.10.0" > "1.9.2").
    nonisolated static func isNewer(_ a: String, than b: String) -> Bool {
        let pa = a.split(separator: ".").map { Int($0) ?? 0 }
        let pb = b.split(separator: ".").map { Int($0) ?? 0 }
        for i in 0..<max(pa.count, pb.count) {
            let x = i < pa.count ? pa[i] : 0
            let y = i < pb.count ? pb[i] : 0
            if x != y { return x > y }
        }
        return false
    }

    private struct Latest: Decodable {
        let tagName: String
        let htmlUrl: URL

        enum CodingKeys: String, CodingKey {
            case tagName = "tag_name"
            case htmlUrl = "html_url"
        }
    }

    private enum UpdateError: Error {
        case http(Int)

        var text: String {
            switch self {
            case let .http(code): "GitHub answered with HTTP \(code). Try again later, or see \(Links.releases.absoluteString)."
            }
        }
    }
}
