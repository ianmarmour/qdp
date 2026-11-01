import Foundation
import Observation
import QuartzCore
import QDPCore
#if os(iOS)
import UIKit
#endif

/// A stream from a host: the core's session, its state as the views show
/// it, and the user's input passed on to it.
///
/// The session draws its video itself, with Metal, into a layer added over
/// the one the view gives; a queue of the model's waits for its events and
/// hands them to the main actor.
@MainActor
@Observable
final class StreamModel {
    /// Where the stream stands.
    enum Phase: Equatable {
        case connecting
        case starting
        case pairing(retry: Bool)
        case streaming
        case reconnecting(String)
        case ended
        case failed(String)
    }

    /// The host and how the stream authenticates; after pairing, the host
    /// as paired.
    private(set) var target: StreamTarget
    private(set) var phase: Phase = .connecting
    private(set) var stats: VideoStats?
    /// The video's size in pixels, once a frame arrived.
    private(set) var videoSize: CGSize?
    /// Whether the host's pointer is relative, so that the mouse is
    /// captured.
    private(set) var relativePointer = false

    /// Called when the host's cursor changes.
    @ObservationIgnored var onCursorChanged: (() -> Void)?
    /// Called when the host moves its pointer without the user, to `x, y`
    /// in video pixels; `moves` if the app's pointer follows.
    @ObservationIgnored var onWarp: ((Double, Double, Bool) -> Void)?
    /// Called with a gamepad's rumble: its slot and motors from 0 to 65535.
    @ObservationIgnored var onRumble: ((Int, Int, Int) -> Void)?
    /// Called when the pointer mode changes.
    @ObservationIgnored var onPointerMode: ((Bool) -> Void)?

    /// The core's session, while one runs.
    @ObservationIgnored private(set) var session: OpaquePointer?
    @ObservationIgnored private let store: HostStore
    @ObservationIgnored private let listening = Flag()
    @ObservationIgnored private let listener = DispatchGroup()

    /// How long the event queue waits for each event before checking it
    /// should stop, in milliseconds.
    private static let eventWait: UInt32 = 100

    init(target: StreamTarget, store: HostStore) {
        self.target = target
        self.store = store
    }

    /// The device's name, which a host shows when pairing.
    static var deviceName: String {
        #if os(macOS)
        Host.current().localizedName ?? "Mac"
        #else
        UIDevice.current.name
        #endif
    }

    /// Starts the session, drawing into a layer over `layer`, which is
    /// `pixels` at `scale` pixels a point, on a display refreshing at
    /// `refreshMillihertz`. Returns whether it started; if not, the phase
    /// says why.
    @discardableResult
    func start(layer: CALayer, pixels: CGSize, scale: CGFloat, refreshMillihertz: UInt32) -> Bool {
        guard session == nil else { return true }
        Core.start()
        let defaults = UserDefaults.standard
        let resolution = Resolution(rawValue: defaults.string(forKey: SettingsKey.resolution) ?? "") ?? .window
        let decoder = DecoderChoice(rawValue: defaults.string(forKey: SettingsKey.decoder) ?? "") ?? .automatic
        let display = requestedDisplay(pixels: pixels, scale: scale, resolution: resolution)
        let host = strdup(target.address)
        let pin = strdup(target.pin)
        let token = strdup(target.token)
        let name = strdup(Self.deviceName)
        defer {
            free(host)
            free(pin)
            free(token)
            free(name)
        }
        var config = QdpConfig(
            host: UnsafePointer(host),
            authentication: target.authentication.core,
            pin: UnsafePointer(pin),
            token: UnsafePointer(token),
            device_name: UnsafePointer(name),
            width: display.width,
            height: display.height,
            refresh_millihertz: refreshMillihertz,
            scale_percent: display.scalePercent,
            decoder: decoder.core,
            microphone: defaults.bool(forKey: SettingsKey.microphone)
        )
        var error: UnsafeMutablePointer<CChar>?
        let layerPointer = Unmanaged.passUnretained(layer).toOpaque()
        guard let started = qdp_session_start(
            &config, layerPointer, Double(pixels.width), Double(pixels.height), Double(scale), &error
        ) else {
            phase = .failed(Core.takeString(error) ?? "The stream could not start.")
            return false
        }
        session = started
        listen(to: started)
        return true
    }

    /// Ends the session, returning once its layer is no longer drawn into.
    func stop() {
        guard let session else { return }
        listening.value = false
        listener.wait()
        qdp_session_stop(session)
        self.session = nil
    }

    /// Tells the session its layer is now `pixels` at `scale` pixels a
    /// point.
    func resize(pixels: CGSize, scale: CGFloat) {
        guard let session else { return }
        qdp_session_resize(session, Double(pixels.width), Double(pixels.height), Double(scale))
    }

    /// Tells the host the display changed to `pixels` at `scale` pixels a
    /// point, as when the window resizes or the device rotates; the core
    /// keeps such requests to a few a second.
    func displayChanged(pixels: CGSize, scale: CGFloat, refreshMillihertz: UInt32) {
        guard let session else { return }
        let resolution = Resolution(rawValue: UserDefaults.standard.string(forKey: SettingsKey.resolution) ?? "") ?? .window
        let display = requestedDisplay(pixels: pixels, scale: scale, resolution: resolution)
        qdp_session_display(session, display.width, display.height, refreshMillihertz, display.scalePercent)
    }

    /// Answers a request for the pairing code with `code`, or declines with
    /// `nil`, which ends the stream.
    func answerPairing(_ code: String?) {
        guard let session else { return }
        phase = .starting
        if let code {
            code.withCString { qdp_session_pairing_code(session, $0) }
        } else {
            qdp_session_pairing_code(session, nil)
        }
    }

    /// Waits for the session's events on a queue of their own and handles
    /// them on the main actor.
    ///
    /// The queue runs at the default quality of service, as the session's
    /// threads that send the events do, so that waiting for them inverts no
    /// priorities.
    private func listen(to session: OpaquePointer) {
        listening.value = true
        let flag = listening
        let wait = Self.eventWait
        let address = Int(bitPattern: session)
        DispatchQueue.global(qos: .default).async(group: listener) { [weak self] in
            guard let session = OpaquePointer(bitPattern: address) else { return }
            while flag.value {
                guard let line = Core.takeString(qdp_session_next_event(session, wait)) else {
                    continue
                }
                guard let event = SessionEvent.parse(line) else { continue }
                let finished = event == .ended || {
                    if case .failed = event { return true }
                    return false
                }()
                Task { @MainActor [weak self] in self?.handle(event) }
                if finished { return }
            }
        }
    }

    private func handle(_ event: SessionEvent) {
        switch event {
        case .connecting:
            if case .pairing = phase { return }
            phase = .connecting
        case .connected:
            phase = .starting
        case let .reconnecting(_, error):
            phase = .reconnecting(error)
        case let .video(width, height):
            videoSize = CGSize(width: width, height: height)
            phase = .streaming
        case .cursor:
            onCursorChanged?()
        case let .warp(x, y, moves):
            onWarp?(x, y, moves)
        case let .pointerMode(relative):
            relativePointer = relative
            onPointerMode?(relative)
        case let .rumble(slot, low, high):
            onRumble?(slot, low, high)
        case let .stats(stats):
            self.stats = stats
        case let .pairingCode(retry):
            phase = .pairing(retry: retry)
        case let .paired(pin):
            // From now on the host is reached with the device's key, also
            // when the stream connects again.
            store.keep(PairedHost(name: target.name, address: target.address, pin: pin))
            target.authentication = .paired
            target.pin = pin
            phase = .starting
        case .ended:
            phase = .ended
        case let .failed(error):
            phase = .failed(error)
        }
    }
}

/// A flag shared between threads.
final class Flag: @unchecked Sendable {
    private let lock = NSLock()
    private var stored = false

    var value: Bool {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }
}
