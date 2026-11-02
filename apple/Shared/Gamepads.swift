import CoreHaptics
import Foundation
import GameController
import QDPCore

/// The controllers connected while a stream runs, sent to the host as
/// gamepads with Standard Gamepad buttons, and rumbling as it asks.
@MainActor
final class Gamepads {
    /// Called when Select, Start and both shoulder buttons are held
    /// together, which opens the stream's menu.
    var onMenuChord: (() -> Void)?

    private let model: StreamModel
    private var slots: [ObjectIdentifier: UInt32] = [:]
    private var engines: [UInt32: CHHapticEngine] = [:]
    private var players: [UInt32: CHHapticPatternPlayer] = [:]
    private var observers: [NSObjectProtocol] = []

    /// The Standard Gamepad's button indices.
    private enum Button: Int {
        case a = 0, b, x, y, leftShoulder, rightShoulder, leftTrigger, rightTrigger
        case select, start, leftStick, rightStick, up, down, left, right, home
    }

    /// The buttons held together that open the stream's menu.
    private static let menuChord: UInt32 = [Button.select, .start, .leftShoulder, .rightShoulder]
        .reduce(0) { $0 | (1 << UInt32($1.rawValue)) }

    /// The largest motor value of a rumble.
    private static let maxRumble = 65535.0

    /// How long one rumble plays unless the host renews it, in seconds.
    private static let rumbleDuration: TimeInterval = 0.5

    init(model: StreamModel) {
        self.model = model
    }

    /// Starts following the controllers.
    func start() {
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: .GCControllerDidConnect, object: nil, queue: .main) { [weak self] note in
            guard let controller = note.object as? GCController else { return }
            MainActor.assumeIsolated { self?.connect(controller) }
        })
        observers.append(center.addObserver(forName: .GCControllerDidDisconnect, object: nil, queue: .main) { [weak self] note in
            guard let controller = note.object as? GCController else { return }
            MainActor.assumeIsolated { self?.disconnect(controller) }
        })
        GCController.controllers().forEach(connect)
    }

    /// Stops following the controllers and disconnects them from the host.
    func stop() {
        observers.forEach { NotificationCenter.default.removeObserver($0) }
        observers.removeAll()
        GCController.controllers().forEach(disconnect)
    }

    /// Plays a gamepad's rumble, each motor from 0 to 65535.
    func rumble(slot: Int, low: Int, high: Int) {
        let slot = UInt32(slot)
        try? players[slot]?.stop(atTime: CHHapticTimeImmediate)
        players[slot] = nil
        let intensity = Float(Double(max(low, high)) / Self.maxRumble)
        guard intensity > 0, let engine = engines[slot] else { return }
        let event = CHHapticEvent(
            eventType: .hapticContinuous,
            parameters: [CHHapticEventParameter(parameterID: .hapticIntensity, value: intensity)],
            relativeTime: 0,
            duration: Self.rumbleDuration
        )
        guard let pattern = try? CHHapticPattern(events: [event], parameters: []),
              let player = try? engine.makePlayer(with: pattern) else { return }
        try? player.start(atTime: CHHapticTimeImmediate)
        players[slot] = player
    }

    private func connect(_ controller: GCController) {
        guard let pad = controller.extendedGamepad, slots[ObjectIdentifier(controller)] == nil,
              let session = model.session else { return }
        let slot = (0...UInt32(slots.count)).first { !slots.values.contains($0) } ?? 0
        slots[ObjectIdentifier(controller)] = slot
        if let engine = controller.haptics?.createEngine(withLocality: .default) {
            try? engine.start()
            engines[slot] = engine
        }
        let name = controller.vendorName ?? "Controller"
        name.withCString { qdp_session_gamepad_connected(session, slot, $0, engines[slot] != nil) }
        model.gamepadConnected = true
        pad.valueChangedHandler = { [weak self] pad, _ in
            MainActor.assumeIsolated { self?.send(pad, slot: slot) }
        }
        send(pad, slot: slot)
    }

    private func disconnect(_ controller: GCController) {
        guard let slot = slots.removeValue(forKey: ObjectIdentifier(controller)) else { return }
        controller.extendedGamepad?.valueChangedHandler = nil
        try? players[slot]?.stop(atTime: CHHapticTimeImmediate)
        players[slot] = nil
        engines.removeValue(forKey: slot)?.stop()
        if let session = model.session {
            qdp_session_gamepad_disconnected(session, slot)
        }
        model.gamepadConnected = !slots.isEmpty
    }

    private func send(_ pad: GCExtendedGamepad, slot: UInt32) {
        guard let session = model.session else { return }
        var buttons: UInt32 = 0
        func hold(_ button: Button, _ pressed: Bool) {
            if pressed { buttons |= 1 << UInt32(button.rawValue) }
        }
        hold(.a, pad.buttonA.isPressed)
        hold(.b, pad.buttonB.isPressed)
        hold(.x, pad.buttonX.isPressed)
        hold(.y, pad.buttonY.isPressed)
        hold(.leftShoulder, pad.leftShoulder.isPressed)
        hold(.rightShoulder, pad.rightShoulder.isPressed)
        hold(.leftTrigger, pad.leftTrigger.isPressed)
        hold(.rightTrigger, pad.rightTrigger.isPressed)
        hold(.select, pad.buttonOptions?.isPressed ?? false)
        hold(.start, pad.buttonMenu.isPressed)
        hold(.leftStick, pad.leftThumbstickButton?.isPressed ?? false)
        hold(.rightStick, pad.rightThumbstickButton?.isPressed ?? false)
        hold(.up, pad.dpad.up.isPressed)
        hold(.down, pad.dpad.down.isPressed)
        hold(.left, pad.dpad.left.isPressed)
        hold(.right, pad.dpad.right.isPressed)
        hold(.home, pad.buttonHome?.isPressed ?? false)
        if buttons & Self.menuChord == Self.menuChord {
            onMenuChord?()
            return
        }
        // The Game Controller framework reports up as positive; QDP's
        // reports, as Android's, have it negative.
        qdp_session_gamepad(
            session, slot, buttons,
            pad.leftThumbstick.xAxis.value, -pad.leftThumbstick.yAxis.value,
            pad.rightThumbstick.xAxis.value, -pad.rightThumbstick.yAxis.value,
            pad.leftTrigger.value, pad.rightTrigger.value
        )
    }
}
