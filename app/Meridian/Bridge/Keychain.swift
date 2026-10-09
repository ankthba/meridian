import Foundation
import MeridianCore
import Security

/// Provider credentials in the macOS login keychain (generic passwords,
/// service `meridian.provider.<provider>`, account = field name).
///
/// The app is signed with a stable Apple Development identity, so the
/// keychain ACL survives rebuilds. The data-protection keychain would need
/// a provisioning profile for its access group; the login keychain doesn't.
nonisolated enum Keychain {
    static func service(_ provider: String) -> String { "meridian.provider.\(provider)" }

    /// `MERIDIAN_SECRETS=none` (tests and first-run checks): behave as if no
    /// key were stored, and never touch the keychain.
    static let disabled = ProcessInfo.processInfo.environment["MERIDIAN_SECRETS"] == "none"

    static func read(provider: String, field: String) -> String? {
        if disabled { return nil }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service(provider),
            kSecAttrAccount as String: field,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    @discardableResult
    static func write(provider: String, field: String, value: String) -> Bool {
        if disabled { return false }
        let base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service(provider),
            kSecAttrAccount as String: field,
        ]
        let data = Data(value.utf8)
        let status = SecItemUpdate(base as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var add = base
            add[kSecValueData as String] = data
            add[kSecAttrLabel as String] = "Meridian \(provider) \(field)"
            return SecItemAdd(add as CFDictionary, nil) == errSecSuccess
        }
        return status == errSecSuccess
    }

    @discardableResult
    static func delete(provider: String, field: String) -> Bool {
        if disabled { return true }
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service(provider),
            kSecAttrAccount as String: field,
        ]
        let s = SecItemDelete(q as CFDictionary)
        return s == errSecSuccess || s == errSecItemNotFound
    }
}

/// `SecretSource` for the Rust core. Called synchronously from Rust threads;
/// SecItem APIs are thread-safe.
nonisolated final class KeychainSecretSource: SecretSource, @unchecked Sendable {
    func secret(provider: String, field: String) -> String? {
        Keychain.read(provider: provider, field: field)
    }
}
