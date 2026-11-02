import QDPCore
import UIKit

/// The on-screen gamepad over the stream on iPhone and iPad, which the
/// stream's menu shows and hides. While it shows it takes every touch on
/// the screen, and the core lays its controls out, works out the gamepad's
/// state from the touches and sends it as a gamepad of its own. The controls
/// are drawn as phone games draw theirs, translucent rings the picture shows
/// through, lit where a thumb holds them; a press taps the haptics, as a real
/// button would click.
final class OnScreenControllerView: UIView {
    /// A control's number, as the core gives it: a Standard Gamepad button
    /// index, the directional pad's first, or a stick's.
    private enum Control: UInt32 {
        case a = 0, b, x, y, leftShoulder, rightShoulder, leftTrigger, rightTrigger
        case select, start, leftStickButton, rightStickButton
        case dpad = 12
        case leftStick = 17, rightStick = 18
    }

    /// The directional pad's direction bits, as the core gives them.
    private enum Direction: UInt32, CaseIterable {
        case up = 1, down = 2, left = 4, right = 8
    }

    private let model: StreamModel
    private let gamepad: OpaquePointer? = qdp_virtual_gamepad_new()
    private var controls: [QdpVirtualControl] = []
    /// The touches on the controls, by touch.
    private var fingers: [ObjectIdentifier: UInt32] = [:]
    private let haptics = UIImpactFeedbackGenerator(style: .light)

    /// A control's fill at rest: dark and translucent, so that the
    /// picture shows through, and bright scenes too.
    private static let fill = UIColor(white: 0, alpha: 0.28)
    /// A control's fill while held.
    private static let heldFill = UIColor(white: 1, alpha: 0.35)
    /// A control's ring.
    private static let outline = UIColor(white: 1, alpha: 0.6)
    /// Labels, arrows and glyphs.
    private static let ink = UIColor(white: 1, alpha: 0.9)
    /// A stick's knob at rest, and while held.
    private static let knob = UIColor(white: 1, alpha: 0.4)
    private static let heldKnob = UIColor(white: 1, alpha: 0.75)
    /// The ring's width, in points.
    private static let outlineWidth: CGFloat = 2
    /// A stick's knob's radius, as a share of its base's.
    private static let knobShare: CGFloat = 0.45
    /// A label's size, as a share of its control's height.
    private static let labelShare: CGFloat = 0.42
    /// A glyph's or arrow's size, as a share of its control's.
    private static let glyphShare: CGFloat = 0.45
    /// A shoulder button's corners, as a share of its height.
    private static let shoulderCornerShare: CGFloat = 0.35

    /// The face buttons' letters, in the colors controllers print them in.
    private static let faceColors: [Control: UIColor] = [
        .a: UIColor(red: 0.48, green: 0.83, blue: 0.37, alpha: 1),
        .b: UIColor(red: 1, green: 0.42, blue: 0.38, alpha: 1),
        .x: UIColor(red: 0.37, green: 0.66, blue: 1, alpha: 1),
        .y: UIColor(red: 1, green: 0.82, blue: 0.37, alpha: 1),
    ]

    /// The labels of the controls drawn with text.
    private static let labels: [Control: String] = [
        .a: "A", .b: "B", .x: "X", .y: "Y",
        .leftShoulder: "LB", .rightShoulder: "RB", .leftTrigger: "LT", .rightTrigger: "RT",
        .leftStickButton: "L3", .rightStickButton: "R3",
    ]

    init(model: StreamModel) {
        self.model = model
        super.init(frame: .zero)
        isMultipleTouchEnabled = true
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        isHidden = true
        isAccessibilityElement = true
        accessibilityLabel = "On-screen controller"
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
    /// button and drawing the controls again if they look different.
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

    /// Takes the controls as they now are from the core, and draws them.
    private func refresh() {
        let count = qdp_virtual_gamepad_controls(gamepad, nil, 0)
        var buffer = [QdpVirtualControl](repeating: QdpVirtualControl(), count: count)
        let written = buffer.withUnsafeMutableBufferPointer { pointer in
            qdp_virtual_gamepad_controls(gamepad, pointer.baseAddress, count)
        }
        controls = Array(buffer.prefix(min(written, count)))
        setNeedsDisplay()
    }

    // MARK: Drawing

    override func draw(_ rect: CGRect) {
        for control in controls {
            guard let kind = Control(rawValue: control.control) else { continue }
            let frame = CGRect(
                x: CGFloat(control.x), y: CGFloat(control.y),
                width: CGFloat(control.width), height: CGFloat(control.height)
            )
            switch kind {
            case .leftStick, .rightStick:
                drawStick(in: frame, control: control)
            case .dpad:
                drawDPad(in: frame, directions: control.directions)
            case .leftShoulder, .rightShoulder, .leftTrigger, .rightTrigger:
                drawShoulder(in: frame, kind: kind, held: control.held)
            default:
                drawRoundButton(in: frame, kind: kind, held: control.held)
            }
        }
    }

    /// Draws a stick: its base, and its knob where its tilt has it.
    private func drawStick(in frame: CGRect, control: QdpVirtualControl) {
        drawCircle(in: frame, held: false)
        let radius = frame.width / 2
        let knob = radius * Self.knobShare
        let travel = radius - knob
        let center = CGPoint(
            x: frame.midX + CGFloat(control.tilt_x) * travel,
            y: frame.midY + CGFloat(control.tilt_y) * travel
        )
        (control.held ? Self.heldKnob : Self.knob).setFill()
        UIBezierPath(ovalIn: CGRect(x: center.x - knob, y: center.y - knob, width: 2 * knob, height: 2 * knob)).fill()
    }

    /// Draws the directional pad: a cross whose arms light as their
    /// directions are held.
    private func drawDPad(in frame: CGRect, directions: UInt32) {
        let arm = frame.width / 3
        let corner = arm / 4
        Self.fill.setFill()
        UIBezierPath(rect: CGRect(x: frame.midX - arm / 2, y: frame.midY - arm / 2, width: arm, height: arm)).fill()
        for direction in Direction.allCases {
            let rect: CGRect
            let symbol: String
            switch direction {
            case .up:
                rect = CGRect(x: frame.midX - arm / 2, y: frame.minY, width: arm, height: arm)
                symbol = "arrowtriangle.up.fill"
            case .down:
                rect = CGRect(x: frame.midX - arm / 2, y: frame.maxY - arm, width: arm, height: arm)
                symbol = "arrowtriangle.down.fill"
            case .left:
                rect = CGRect(x: frame.minX, y: frame.midY - arm / 2, width: arm, height: arm)
                symbol = "arrowtriangle.left.fill"
            case .right:
                rect = CGRect(x: frame.maxX - arm, y: frame.midY - arm / 2, width: arm, height: arm)
                symbol = "arrowtriangle.right.fill"
            }
            let path = UIBezierPath(roundedRect: rect, cornerRadius: corner)
            (directions & direction.rawValue != 0 ? Self.heldFill : Self.fill).setFill()
            path.fill()
            stroke(path)
            drawSymbol(symbol, in: rect)
        }
    }

    /// Draws a shoulder button or trigger: a rounded rectangle and its label.
    private func drawShoulder(in frame: CGRect, kind: Control, held: Bool) {
        let path = UIBezierPath(roundedRect: frame, cornerRadius: frame.height * Self.shoulderCornerShare)
        (held ? Self.heldFill : Self.fill).setFill()
        path.fill()
        stroke(path)
        drawLabel(Self.labels[kind] ?? "", in: frame, color: Self.ink)
    }

    /// Draws a round button: the face buttons, the stick buttons, Select and
    /// Start.
    private func drawRoundButton(in frame: CGRect, kind: Control, held: Bool) {
        drawCircle(in: frame, held: held)
        switch kind {
        case .select:
            drawSymbol("rectangle.on.rectangle", in: frame)
        case .start:
            drawSymbol("line.3.horizontal", in: frame)
        default:
            drawLabel(Self.labels[kind] ?? "", in: frame, color: Self.faceColors[kind] ?? Self.ink)
        }
    }

    /// Draws a filled, outlined circle in `frame`.
    private func drawCircle(in frame: CGRect, held: Bool) {
        let inset = Self.outlineWidth / 2
        let path = UIBezierPath(ovalIn: frame.insetBy(dx: inset, dy: inset))
        (held ? Self.heldFill : Self.fill).setFill()
        path.fill()
        stroke(path)
    }

    /// Outlines `path`.
    private func stroke(_ path: UIBezierPath) {
        Self.outline.setStroke()
        path.lineWidth = Self.outlineWidth
        path.stroke()
    }

    /// Draws `text` in `color`, centred in `frame`, sized to it.
    private func drawLabel(_ text: String, in frame: CGRect, color: UIColor) {
        let attributes: [NSAttributedString.Key: Any] = [
            .font: UIFont.systemFont(ofSize: frame.height * Self.labelShare, weight: .semibold),
            .foregroundColor: color,
        ]
        let size = (text as NSString).size(withAttributes: attributes)
        (text as NSString).draw(
            at: CGPoint(x: frame.midX - size.width / 2, y: frame.midY - size.height / 2),
            withAttributes: attributes
        )
    }

    /// Draws the SF Symbol `name`, centred in `frame`, sized to it.
    private func drawSymbol(_ name: String, in frame: CGRect) {
        let side = min(frame.width, frame.height) * Self.glyphShare
        let configuration = UIImage.SymbolConfiguration(pointSize: side, weight: .semibold)
        guard let image = UIImage(systemName: name, withConfiguration: configuration)?
            .withTintColor(Self.ink, renderingMode: .alwaysOriginal) else { return }
        let size = image.size
        image.draw(in: CGRect(x: frame.midX - size.width / 2, y: frame.midY - size.height / 2, width: size.width, height: size.height))
    }
}
