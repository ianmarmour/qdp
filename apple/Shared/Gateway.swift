import Foundation
import Security

/// The organization's gateway the app reaches computers through
/// (Section 13 of the specification), as the user set it in Settings: its
/// address, its key pin if its administrator gave one, and the user's name.
/// The session token its last sign-in gave stays in the Keychain, as the
/// credential it is.
enum Gateway {
    /// The gateway's address, or `nil` if none is set.
    static var address: String? {
        let text = UserDefaults.standard.string(forKey: SettingsKey.gatewayAddress)?
            .trimmingCharacters(in: .whitespaces) ?? ""
        return text.isEmpty ? nil : text
    }

    /// Its key pin, or empty to verify it by the system's certificate
    /// authorities.
    static var pin: String {
        UserDefaults.standard.string(forKey: SettingsKey.gatewayPin)?.trimmingCharacters(in: .whitespaces) ?? ""
    }

    /// The user's name in the organization's directory, or empty.
    static var user: String {
        UserDefaults.standard.string(forKey: SettingsKey.gatewayUser) ?? ""
    }

    /// The Keychain item's service.
    private static let service = "io.github.ianmarmour.qdp.gateway"

    /// The session token of the last sign-in, in hexadecimal, or empty if
    /// there is none or it expired.
    static var sessionToken: String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: address ?? "",
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
              let data = item as? Data,
              let stored = String(data: data, encoding: .utf8)
        else { return "" }
        // The item keeps when the token expires, a space, and the token.
        let parts = stored.split(separator: " ", maxSplits: 1)
        guard parts.count == 2, let expires = TimeInterval(parts[0]), Date().timeIntervalSince1970 < expires else {
            return ""
        }
        return String(parts[1])
    }

    /// Keeps the session `token` a "gatewaysession" event gave, valid for
    /// `lifetimeSeconds`, for the gateway set now.
    static func keepSession(token: String, lifetimeSeconds: Int) {
        guard let address else { return }
        let expires = Date().timeIntervalSince1970 + TimeInterval(lifetimeSeconds)
        let data = Data("\(Int(expires)) \(token)".utf8)
        let match: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: address,
        ]
        SecItemDelete(match as CFDictionary)
        var item = match
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(item as CFDictionary, nil)
    }
}

/// A computer reached through the gateway: its name for people, and what
/// the gateway knows it as, its ID there or its name inside the network.
struct GatewayHost: Codable, Hashable, Identifiable {
    var name: String
    var target: String

    var id: String { target }

    /// The stream that reaches it.
    var stream: StreamTarget { StreamTarget(name: name, address: target, authentication: .gateway) }
}
