import Foundation
import QDPCore

/// How a stream authenticates with its host.
enum Authentication: String, Codable, Hashable {
    /// A host the device paired with, which accepts the device's key.
    case paired
    /// Pairing with the code the host shows.
    case pair
    /// The host's pin and a token it accepts.
    case token
    /// Through the organization's gateway, which vouches for the user.
    case gateway

    /// The core's value.
    var core: QdpAuthentication {
        switch self {
        case .paired: QDP_AUTH_PAIRED
        case .pair: QDP_AUTH_PAIR
        case .token: QDP_AUTH_TOKEN
        case .gateway: QDP_AUTH_GATEWAY
        }
    }
}

/// The host a stream connects to and how it authenticates.
struct StreamTarget: Codable, Hashable, Identifiable {
    /// The host's name for people.
    var name: String
    /// Its address or name, with or without a port.
    var address: String
    /// How the stream authenticates.
    var authentication: Authentication
    /// The host's key pin, empty while pairing.
    var pin: String = ""
    /// The token, for token authentication.
    var token: String = ""

    var id: String { "\(address)|\(pin)|\(authentication.rawValue)" }
}

/// A host the device paired with: its name, the address it was last reached
/// at, and its key pin.
struct PairedHost: Codable, Hashable, Identifiable {
    var name: String
    var address: String
    var pin: String

    var id: String { pin }
}

/// The display a stream asks the host for.
enum Resolution: String, CaseIterable, Identifiable, Codable {
    case window, p720, p1080, p1440, p2160

    var id: String { rawValue }

    /// Its name in settings.
    var label: String {
        switch self {
        case .window: "Match this display"
        case .p720: "720p"
        case .p1080: "1080p"
        case .p1440: "1440p"
        case .p2160: "4K"
        }
    }

    /// Its height in pixels, or `nil` to match the display.
    var height: Int? {
        switch self {
        case .window: nil
        case .p720: 720
        case .p1080: 1080
        case .p1440: 1440
        case .p2160: 2160
        }
    }
}

/// Where video is decoded.
enum DecoderChoice: String, CaseIterable, Identifiable, Codable {
    case automatic, hardware, software

    var id: String { rawValue }

    /// Its name in settings.
    var label: String {
        switch self {
        case .automatic: "Automatic"
        case .hardware: "Hardware only"
        case .software: "Software"
        }
    }

    /// The core's value.
    var core: QdpDecoder {
        switch self {
        case .automatic: QDP_DECODER_AUTO
        case .hardware: QDP_DECODER_HARDWARE
        case .software: QDP_DECODER_SOFTWARE
        }
    }
}

/// What touching the screen does, on iPhone and iPad.
enum TouchMode: String, CaseIterable, Identifiable, Codable {
    /// Fingers move the host's pointer as on a laptop's trackpad.
    case trackpad
    /// Fingers touch the host's screen where they land.
    case direct

    var id: String { rawValue }

    /// Its name in settings.
    var label: String {
        switch self {
        case .trackpad: "Trackpad"
        case .direct: "Direct touch"
        }
    }

    /// What it does, for settings.
    var explanation: String {
        switch self {
        case .trackpad: "Drag to move the pointer, tap to click."
        case .direct: "Tap where you want to click."
        }
    }
}

/// What an iPhone and iPad app's touches come from.
enum TouchSource {
    /// Fingers on the device's screen, and any pointer attached to it.
    case touchscreen
    /// The pointer of the Mac the app runs on, which has no touchscreen.
    case macPointer

    /// Where this device's touches come from.
    static let current: TouchSource = ProcessInfo.processInfo.isiOSAppOnMac ? .macPointer : .touchscreen
}

/// The keys settings are kept under.
enum SettingsKey {
    static let resolution = "resolution"
    static let decoder = "decoder"
    static let touchMode = "touchMode"
    static let showStats = "showStats"
    static let microphone = "microphone"
    static let gatewayAddress = "gatewayAddress"
    static let gatewayPin = "gatewayPin"
    static let gatewayUser = "gatewayUser"
}

/// Returns the size and interface scale to ask the host for, for a view of
/// `pixels` at `scale` pixels a point, with `resolution` chosen: the host
/// draws its interface as much smaller as the picture is stretched, rounded
/// to a quarter, so that its text shows at the size this device's would.
func requestedDisplay(pixels: CGSize, scale: CGFloat, resolution: Resolution) -> (width: UInt32, height: UInt32, scalePercent: UInt32) {
    let width = max(pixels.width, 1)
    let height = max(pixels.height, 1)
    guard let target = resolution.height.map({ CGFloat($0) }), target < height else {
        return (UInt32(width.rounded()), UInt32(height.rounded()), UInt32((scale * 100).rounded()))
    }
    let shrink = target / height
    let scaled = (scale * shrink * 4).rounded() / 4
    return (
        UInt32((width * shrink).rounded()),
        UInt32(target),
        UInt32(max(scaled, 1) * 100)
    )
}
