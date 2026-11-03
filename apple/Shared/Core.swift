import Foundation
import QDPCore

/// The Rust core, `QDPCore`: its one-time setup and the device's key.
enum Core {
    /// Sets up the core once, keeping the device's key in the app's private
    /// Application Support directory.
    static func start() {
        _ = started
    }

    private static let started: Void = {
        let directory = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("QDP", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        directory.path.withCString { qdp_init($0) }
    }()

    /// The device's key pin, which a host accepts with
    /// `qdp serve --client-key`, or `nil` if the key cannot be kept.
    static var clientPin: String? {
        start()
        return takeString(qdp_client_pin())
    }

    /// Returns whether `text` is a key pin: 64 hexadecimal digits.
    static func isPin(_ text: String) -> Bool {
        text.withCString { qdp_is_pin($0) }
    }

    /// Returns whether a QDP host answers at `address`, "host" or
    /// "host:port", without connecting to it. Blocks for up to a second and
    /// a half, so it is called off the main thread.
    static func hostReachable(_ address: String) -> Bool {
        address.withCString { qdp_host_reachable($0) }
    }

    /// Returns a string the core returned, freeing it, or `nil` for none.
    static func takeString(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
        guard let pointer else { return nil }
        defer { qdp_string_free(pointer) }
        return String(cString: pointer)
    }
}
