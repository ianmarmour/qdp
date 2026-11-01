import Foundation

/// Something the core tells the app, from a line `qdp_session_next_event`
/// returns.
enum SessionEvent: Equatable {
    /// Connecting to the host.
    case connecting
    /// Connected; the session is starting.
    case connected
    /// The connection was lost, and is made again after a delay.
    case reconnecting(delayMillis: Int, error: String)
    /// The video's size changed.
    case video(width: Int, height: Int)
    /// The host's cursor changed.
    case cursor
    /// The host's pointer moved without the user; `moves` if the app's
    /// pointer follows, rather than only the cursor drawn.
    case warp(x: Double, y: Double, moves: Bool)
    /// The host's pointer mode changed; in relative mode the pointer is
    /// captured.
    case pointerMode(relative: Bool)
    /// A gamepad's rumble, each motor from 0 to 65535.
    case rumble(slot: Int, low: Int, high: Int)
    /// The last second's video.
    case stats(VideoStats)
    /// Pairing needs the code the host shows; `retry` after a code that did
    /// not work.
    case pairingCode(retry: Bool)
    /// Pairing succeeded with the host whose key pin this is.
    case paired(pin: String)
    /// The session ended normally.
    case ended
    /// The session failed.
    case failed(String)

    /// Returns the event a line describes, or `nil` for one the app does not
    /// know.
    static func parse(_ line: String) -> SessionEvent? {
        let parts = line.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: false)
        let word = parts.first.map(String.init) ?? ""
        let rest = parts.count > 1 ? String(parts[1]) : ""
        let values = rest.split(separator: " ").map(String.init)
        func number(_ index: Int) -> Double? {
            index < values.count ? Double(values[index]) : nil
        }
        switch word {
        case "connecting":
            return .connecting
        case "connected":
            return .connected
        case "reconnecting":
            let delay = Int(values.first ?? "") ?? 0
            let error = rest.split(separator: " ", maxSplits: 1).dropFirst().first.map(String.init) ?? ""
            return .reconnecting(delayMillis: delay, error: error)
        case "video":
            guard let width = number(0), let height = number(1) else { return nil }
            return .video(width: Int(width), height: Int(height))
        case "cursor":
            return .cursor
        case "warp":
            guard let x = number(0), let y = number(1) else { return nil }
            return .warp(x: x, y: y, moves: values.count > 2 && values[2] == "move")
        case "pointer":
            return .pointerMode(relative: values.first == "relative")
        case "rumble":
            guard let slot = number(0), let low = number(1), let high = number(2) else { return nil }
            return .rumble(slot: Int(slot), low: Int(low), high: Int(high))
        case "stats":
            return VideoStats.parse(values).map(SessionEvent.stats)
        case "pair":
            return .pairingCode(retry: values.first == "retry")
        case "paired":
            guard let pin = values.first else { return nil }
            return .paired(pin: pin)
        case "ended":
            return .ended
        case "failed":
            return .failed(rest)
        default:
            return nil
        }
    }
}

/// What the video did over the last second.
struct VideoStats: Equatable {
    /// The frames shown.
    var fps: Int
    /// The mean time from capture on the host to decoding, once known, in
    /// milliseconds.
    var latencyMillis: Double?
    /// The mean time decoding took, in milliseconds.
    var decodeMillis: Double
    /// The slowest frame's latency, in milliseconds, if known.
    var slowestMillis: Double?

    /// The values of a `stats` line before its slowest frame's stages.
    private static let leadingValues = 3

    /// Reads a `stats` line's values.
    static func parse(_ values: [String]) -> VideoStats? {
        guard values.count >= leadingValues, let fps = Int(values[0]), let decode = Double(values[2]) else {
            return nil
        }
        return VideoStats(
            fps: fps,
            latencyMillis: Double(values[1]),
            decodeMillis: decode,
            slowestMillis: values.count > leadingValues ? Double(values[leadingValues]) : nil
        )
    }

    /// A short summary for the statistics overlay.
    var summary: String {
        let latency = latencyMillis.map { String(format: "%.1f ms", $0) } ?? "– ms"
        return String(format: "%d fps · %@ · decode %.1f ms", fps, latency, decodeMillis)
    }
}
