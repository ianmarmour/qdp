import Observation
import QDPCore
import SwiftUI
import UIKit

/// The on-screen gamepad over the stream on iPhone and iPad, which the
/// stream's menu shows and hides. While it shows it takes every touch on
/// the screen, and the core lays its controls out, works out the gamepad's
/// state from the touches and sends it as a gamepad of its own. The controls
/// are Liquid Glass, as the app's other controls over the stream are, drawn
/// by SwiftUI over this view, which takes the touches itself; a press taps
/// the haptics, as a real button would click.
final class OnScreenControllerView: UIView {
    private let model: StreamModel
    private let gamepad: OpaquePointer? = qdp_virtual_gamepad_new()
    /// The touches on the controls, by touch.
    private var fingers: [ObjectIdentifier: UInt32] = [:]
    private let haptics = UIImpactFeedbackGenerator(style: .light)
    /// The controls as the core last gave them, which the glass shows.
    private let glass: GamepadGlassModel
    /// The SwiftUI view drawing the controls.
    private let glassHost: UIHostingController<GamepadGlass>

    init(model: StreamModel) {
        self.model = model
        let glass = GamepadGlassModel()
        self.glass = glass
        glassHost = UIHostingController(rootView: GamepadGlass(model: glass))
        super.init(frame: .zero)
        isMultipleTouchEnabled = true
        isOpaque = false
        backgroundColor = .clear
        isHidden = true
        isAccessibilityElement = true
        accessibilityLabel = "On-screen controller"
        // The controls are placed in this view's own coordinates, safe area
        // and all, and this view, not the glass, takes the touches.
        glassHost.safeAreaRegions = []
        let host: UIView = glassHost.view
        host.backgroundColor = .clear
        host.isUserInteractionEnabled = false
        host.frame = bounds
        host.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(host)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("not used")
    }

    deinit {
        qdp_virtual_gamepad_free(gamepad)
    }

    /// Shows the gamepad, connected to the session, or hides it,
    /// disconnected with every control released.
    func show(_ shown: Bool) {
        isHidden = !shown
        fingers.removeAll()
        qdp_virtual_gamepad_connect(gamepad, model.session, shown)
        if shown {
            setNeedsLayout()
        }
        refresh()
    }

    /// Connects the gamepad to a session that started while it shows.
    func sessionStarted() {
        guard !isHidden else { return }
        qdp_virtual_gamepad_connect(gamepad, model.session, true)
    }

    /// Releases every control held, as when the stream's menu opens.
    func release() {
        fingers.removeAll()
        qdp_virtual_gamepad_release(gamepad, model.session)
        refresh()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let insets = safeAreaInsets
        qdp_virtual_gamepad_lay_out(
            gamepad, model.session,
            Float(bounds.width), Float(bounds.height),
            Float(insets.left), Float(insets.top), Float(insets.right), Float(insets.bottom)
        )
        refresh()
    }

    // MARK: Touch

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        take(touches, phase: QDP_TOUCH_DOWN)
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        take(touches, phase: QDP_TOUCH_MOVE)
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        take(touches, phase: QDP_TOUCH_UP)
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        take(touches, phase: QDP_TOUCH_CANCEL)
    }

    /// Passes `touches` to the core, tapping the haptics if they pressed a
    /// button and showing the controls again if they look different.
    private func take(_ touches: Set<UITouch>, phase: QdpTouchPhase) {
        var outcome: UInt32 = 0
        for touch in touches {
            let id = ObjectIdentifier(touch)
            let finger = fingers[id] ?? (0...UInt32(fingers.count)).first { !fingers.values.contains($0) } ?? 0
            fingers[id] = phase == QDP_TOUCH_UP || phase == QDP_TOUCH_CANCEL ? nil : finger
            let point = touch.location(in: self)
            outcome |= qdp_virtual_gamepad_touch(gamepad, model.session, finger, phase, Float(point.x), Float(point.y))
        }
        if outcome & UInt32(QDP_VIRTUAL_GAMEPAD_PRESSED) != 0 {
            haptics.impactOccurred()
        }
        if outcome & UInt32(QDP_VIRTUAL_GAMEPAD_REDRAW) != 0 {
            refresh()
        }
    }

    /// Takes the controls as they now are from the core, for the glass to
    /// show.
    private func refresh() {
        let count = qdp_virtual_gamepad_controls(gamepad, nil, 0)
        var buffer = [QdpVirtualControl](repeating: QdpVirtualControl(), count: count)
        let written = buffer.withUnsafeMutableBufferPointer { pointer in
            qdp_virtual_gamepad_controls(gamepad, pointer.baseAddress, count)
        }
        glass.controls = Array(buffer.prefix(min(written, count)))
    }
}

/// A control's number, as the core gives it: a Standard Gamepad button
/// index, the directional pad's first, or a stick's.
private enum Control: UInt32 {
    case a = 0, b, x, y, leftShoulder, rightShoulder, leftTrigger, rightTrigger
    case select, start, leftStickButton, rightStickButton
    case dpad = 12
    case leftStick = 17, rightStick = 18

    /// The label of a control shown with text.
    var label: String {
        switch self {
        case .a: "A"
        case .b: "B"
        case .x: "X"
        case .y: "Y"
        case .leftShoulder: "LB"
        case .rightShoulder: "RB"
        case .leftTrigger: "LT"
        case .rightTrigger: "RT"
        case .leftStickButton: "L3"
        case .rightStickButton: "R3"
        case .select, .start, .dpad, .leftStick, .rightStick: ""
        }
    }

    /// The colour of a face button's letter, as controllers print them, or
    /// white.
    var tint: Color {
        switch self {
        case .a: Color(red: 0.56, green: 0.88, blue: 0.48)
        case .b: Color(red: 1, green: 0.54, blue: 0.5)
        case .x: Color(red: 0.49, green: 0.72, blue: 1)
        case .y: Color(red: 1, green: 0.84, blue: 0.42)
        default: .white
        }
    }
}

/// The directional pad's direction bits, as the core gives them, with the
/// symbol of each.
private enum Direction: UInt32, CaseIterable {
    case up = 1, down = 2, left = 4, right = 8

    /// The chevron pointing its way.
    var symbol: String {
        switch self {
        case .up: "chevron.up"
        case .down: "chevron.down"
        case .left: "chevron.left"
        case .right: "chevron.right"
        }
    }

    /// Where its chevron sits in the pad.
    var alignment: Alignment {
        switch self {
        case .up: .top
        case .down: .bottom
        case .left: .leading
        case .right: .trailing
        }
    }
}

/// The controls the glass shows, as the core last gave them.
@Observable
final class GamepadGlassModel {
    var controls: [QdpVirtualControl] = []
}

private extension QdpVirtualControl {
    /// Its frame in the view.
    var rect: CGRect {
        CGRect(x: CGFloat(x), y: CGFloat(y), width: CGFloat(width), height: CGFloat(height))
    }
}

/// A trigger and its shoulder button, side by side as the core places them,
/// which share one pill of glass.
private struct Pill: Identifiable {
    /// Its halves, left to right.
    let halves: [QdpVirtualControl]

    var id: UInt32 { halves.first?.control ?? 0 }

    /// The frame around its halves.
    var rect: CGRect {
        halves.map(\.rect).reduce(CGRect.null) { $0.union($1) }
    }

    /// Each side's trigger and shoulder button.
    static let pairs: [[Control]] = [[.leftTrigger, .leftShoulder], [.rightShoulder, .rightTrigger]]

    /// Returns the pills the shoulder buttons and triggers among `controls`
    /// make.
    static func of(_ controls: [QdpVirtualControl]) -> [Pill] {
        pairs.compactMap { pair in
            let halves = controls
                .filter { control in pair.contains { $0.rawValue == control.control } }
                .sorted { $0.x < $1.x }
            return halves.isEmpty ? nil : Pill(halves: halves)
        }
    }

    /// Returns whether `control` is half of a pill.
    static func contains(_ control: QdpVirtualControl) -> Bool {
        pairs.contains { pair in pair.contains { $0.rawValue == control.control } }
    }
}

/// The on-screen gamepad's controls in Liquid Glass, each where the core
/// placed it, as held or tilted as the touches have it. It takes no touches.
struct GamepadGlass: View {
    let model: GamepadGlassModel

    var body: some View {
        let controls = model.controls
        ZStack(alignment: .topLeading) {
            ForEach(Pill.of(controls)) { pill in
                PillGlass(pill: pill)
            }
            ForEach(controls.filter { !Pill.contains($0) }, id: \.control) { control in
                ControlGlass(control: control)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .ignoresSafeArea()
        .accessibilityHidden(true)
    }
}

/// How a held control's glass is tinted.
private let heldTint = Color.white.opacity(0.32)

/// Returns the glass of a control, lit while held.
private func controlGlass(held: Bool) -> Glass {
    held ? Glass.regular.tint(heldTint) : Glass.regular
}

/// A control that is not half of a pill, in its frame.
private struct ControlGlass: View {
    let control: QdpVirtualControl

    var body: some View {
        let rect = control.rect
        Group {
            if let kind = Control(rawValue: control.control) {
                switch kind {
                case .leftStick, .rightStick:
                    StickGlass(control: control, side: rect.width)
                case .dpad:
                    PadGlass(directions: control.directions, side: rect.width)
                default:
                    ButtonGlass(kind: kind, held: control.held, side: rect.height)
                }
            }
        }
        .frame(width: rect.width, height: rect.height)
        .position(x: rect.midX, y: rect.midY)
    }
}

/// A stick: clear glass for its base, and a knob of glass where its tilt
/// has it.
private struct StickGlass: View {
    let control: QdpVirtualControl
    let side: CGFloat

    /// The knob's width, as a share of the base's.
    private static let knobShare: CGFloat = 0.46

    var body: some View {
        let knob = side * Self.knobShare
        let travel = (side - knob) / 2
        ZStack {
            Color.clear
                .glassEffect(Glass.clear, in: .circle)
            Color.clear
                .frame(width: knob, height: knob)
                .glassEffect(controlGlass(held: control.held), in: .circle)
                .offset(x: CGFloat(control.tilt_x) * travel, y: CGFloat(control.tilt_y) * travel)
        }
    }
}

/// A round button: A, B, X and Y in their colours, Select and Start with
/// their symbols, and any other with its label. It sinks a little while
/// held.
private struct ButtonGlass: View {
    let kind: Control
    let held: Bool
    let side: CGFloat

    /// A label's size, as a share of the button's.
    private static let labelShare: CGFloat = 0.4
    /// A symbol's size, as a share of the button's.
    private static let symbolShare: CGFloat = 0.38
    /// How far a held button sinks.
    private static let heldScale: CGFloat = 0.92
    /// How long a button takes to sink or rise, in seconds.
    private static let pressDuration = 0.12

    var body: some View {
        label
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .glassEffect(controlGlass(held: held), in: .circle)
            .scaleEffect(held ? Self.heldScale : 1)
            .animation(.snappy(duration: Self.pressDuration), value: held)
    }

    @ViewBuilder private var label: some View {
        switch kind {
        case .select:
            Image(systemName: "rectangle.on.rectangle")
                .font(.system(size: side * Self.symbolShare, weight: .semibold))
                .foregroundStyle(.white)
        case .start:
            Image(systemName: "line.3.horizontal")
                .font(.system(size: side * Self.symbolShare, weight: .semibold))
                .foregroundStyle(.white)
        default:
            Text(kind.label)
                .font(.system(size: side * Self.labelShare, weight: .semibold, design: .rounded))
                .foregroundStyle(kind.tint)
        }
    }
}

/// The directional pad, which the default layout leaves out: a round pad of
/// glass with a chevron each way, bright while its direction is held.
private struct PadGlass: View {
    let directions: UInt32
    let side: CGFloat

    /// A chevron's size, as a share of the pad's.
    private static let chevronShare: CGFloat = 0.2
    /// The space between a chevron and the pad's edge, as a share of the
    /// pad's size.
    private static let edgeShare: CGFloat = 0.1
    /// A chevron's opacity while its direction is not held.
    private static let restingOpacity = 0.6

    var body: some View {
        ZStack {
            ForEach(Direction.allCases, id: \.rawValue) { direction in
                let held = directions & direction.rawValue != 0
                Image(systemName: direction.symbol)
                    .font(.system(size: side * Self.chevronShare, weight: .bold))
                    .foregroundStyle(.white.opacity(held ? 1 : Self.restingOpacity))
                    .padding(side * Self.edgeShare)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: direction.alignment)
            }
        }
        .glassEffect(controlGlass(held: directions != 0), in: .circle)
    }
}

/// A trigger and its shoulder button as one pill of glass, a line between
/// them, each half lit while held.
private struct PillGlass: View {
    let pill: Pill

    /// A label's size, as a share of the pill's height.
    private static let labelShare: CGFloat = 0.34
    /// The line between the halves: its opacity, width, and the space it
    /// leaves above and below, as a share of the pill's height.
    private static let lineOpacity = 0.35
    private static let lineWidth: CGFloat = 1
    private static let lineInsetShare: CGFloat = 0.24

    var body: some View {
        let rect = pill.rect
        HStack(spacing: 0) {
            ForEach(Array(pill.halves.enumerated()), id: \.element.control) { index, half in
                if index > 0 {
                    Rectangle()
                        .fill(.white.opacity(Self.lineOpacity))
                        .frame(width: Self.lineWidth)
                        .padding(.vertical, rect.height * Self.lineInsetShare)
                }
                Text(Control(rawValue: half.control)?.label ?? "")
                    .font(.system(size: rect.height * Self.labelShare, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(half.held ? heldTint : Color.clear)
            }
        }
        .clipShape(.capsule)
        .glassEffect(Glass.regular, in: .capsule)
        .frame(width: rect.width, height: rect.height)
        .position(x: rect.midX, y: rect.midY)
    }
}
