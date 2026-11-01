import GameController
import QDPCore
import UIKit

/// The view a stream draws into on iPhone and iPad, which passes on touches
/// as a trackpad or the host's own touchscreen, keyboards, mice and
/// gamepads, and asks the display for the stream's frame rate.
final class StreamViewController: UIViewController {
    /// What touching the screen does.
    var touchMode: TouchMode {
        didSet { if touchMode != oldValue { touchModeChanged() } }
    }

    private let model: StreamModel
    private let openMenu: () -> Void
    private var gamepads: Gamepads?
    private var displayLink: CADisplayLink?
    private let keyboard = KeyboardField()
    private var keyboardRequest = 0
    private var observers: [NSObjectProtocol] = []

    /// The pointer, in video pixels, which trackpad touches move.
    private var pointer = CGPoint.zero
    /// The fingers touching the host's touchscreen, by touch.
    private var fingers: [ObjectIdentifier: UInt32] = [:]
    /// Whether a held press drags with the left button down.
    private var dragging = false
    private var scrollRemainder = CGSize.zero
    private var trackpadGestures: [UIGestureRecognizer] = []

    /// How much farther than the finger the pointer moves.
    private static let pointerSpeed: CGFloat = 1.5
    /// 1/120ths of a detent per point two fingers scroll.
    private static let unitsPerPoint: CGFloat = 3
    /// How long a finger is held still before it drags.
    private static let dragDelay: TimeInterval = 0.35
    /// The frame rates the display is asked for while streaming.
    private static let frameRates = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
    /// Millihertz per hertz.
    private static let millihertz = 1000
    /// The keyboard page usages of Return and Backspace.
    static let returnUsage: UInt16 = 0x28
    static let backspaceUsage: UInt16 = 0x2A

    init(model: StreamModel, touchMode: TouchMode, openMenu: @escaping () -> Void) {
        self.model = model
        self.touchMode = touchMode
        self.openMenu = openMenu
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("not used")
    }

    override func loadView() {
        let view = UIView()
        view.backgroundColor = .black
        view.isMultipleTouchEnabled = true
        self.view = view
        keyboard.controller = self
        view.addSubview(keyboard)
        installGestures()
        view.addGestureRecognizer(UIHoverGestureRecognizer(target: self, action: #selector(hovered)))
    }

    override var canBecomeFirstResponder: Bool { true }
    override var prefersPointerLocked: Bool { model.relativePointer }
    override var prefersHomeIndicatorAutoHidden: Bool { true }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
        guard model.session == nil else { return }
        let started = model.start(
            layer: view.layer,
            pixels: pixelSize,
            scale: scale,
            refreshMillihertz: refreshMillihertz
        )
        guard started else { return }
        model.onPointerMode = { [weak self] _ in self?.setNeedsUpdateOfPrefersPointerLocked() }
        model.onWarp = { [weak self] x, y, _ in self?.warped(to: CGPoint(x: x, y: y)) }
        let gamepads = Gamepads(model: model)
        gamepads.onMenuChord = { [weak self] in self?.openMenu() }
        model.onRumble = { [weak gamepads] slot, low, high in gamepads?.rumble(slot: slot, low: low, high: high) }
        gamepads.start()
        self.gamepads = gamepads
        touchModeChanged()
        followMice()
        let link = CADisplayLink(target: self, selector: #selector(step))
        link.preferredFrameRateRange = Self.frameRates
        link.add(to: .main, forMode: .common)
        displayLink = link
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: UIApplication.willResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.focusChanged(false) }
        })
        observers.append(center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.focusChanged(true) }
        })
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        model.resize(pixels: pixelSize, scale: scale)
        model.displayChanged(pixels: pixelSize, scale: scale, refreshMillihertz: refreshMillihertz)
    }

    /// Stops everything the controller started, as the stream closes.
    func tearDown() {
        observers.forEach { NotificationCenter.default.removeObserver($0) }
        observers.removeAll()
        displayLink?.invalidate()
        displayLink = nil
        gamepads?.stop()
        gamepads = nil
        GCMouse.mice().forEach { $0.mouseInput?.mouseMovedHandler = nil }
        model.stop()
    }

    @objc private func step(_ link: CADisplayLink) {}

    private var scale: CGFloat {
        view.window?.windowScene?.screen.scale ?? traitCollection.displayScale
    }

    private var pixelSize: CGSize {
        CGSize(width: view.bounds.width * scale, height: view.bounds.height * scale)
    }

    private var refreshMillihertz: UInt32 {
        UInt32((view.window?.windowScene?.screen.maximumFramesPerSecond ?? 60) * Self.millihertz)
    }

    private func focusChanged(_ focused: Bool) {
        guard let session = model.session else { return }
        qdp_session_focus(session, focused)
    }

    /// Shows the on-screen keyboard for request number `request`, once.
    func showKeyboard(request: Int) {
        guard request != keyboardRequest else { return }
        keyboardRequest = request
        keyboard.becomeFirstResponder()
    }

    // MARK: Touch

    private func installGestures() {
        let move = UIPanGestureRecognizer(target: self, action: #selector(panned))
        move.maximumNumberOfTouches = 1
        let scroll = UIPanGestureRecognizer(target: self, action: #selector(scrolled))
        scroll.minimumNumberOfTouches = 2
        scroll.maximumNumberOfTouches = 2
        let click = UITapGestureRecognizer(target: self, action: #selector(tapped))
        let rightClick = UITapGestureRecognizer(target: self, action: #selector(twoFingerTapped))
        rightClick.numberOfTouchesRequired = 2
        let keyboardTap = UITapGestureRecognizer(target: self, action: #selector(threeFingerTapped))
        keyboardTap.numberOfTouchesRequired = 3
        let drag = UILongPressGestureRecognizer(target: self, action: #selector(held))
        drag.minimumPressDuration = Self.dragDelay
        drag.delegate = self
        move.delegate = self
        trackpadGestures = [move, scroll, click, rightClick, keyboardTap, drag]
        for gesture in trackpadGestures {
            // Mice and trackpads attached to an iPad point and click as
            // themselves.
            gesture.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.direct.rawValue)]
            view.addGestureRecognizer(gesture)
        }
    }

    private func touchModeChanged() {
        let trackpad = touchMode == .trackpad
        trackpadGestures.forEach { $0.isEnabled = trackpad }
        if let session = model.session {
            qdp_session_draw_cursor(session, trackpad)
        }
    }

    /// The picture's place in the layer and the video's size, once a frame
    /// arrived.
    private var picture: (Placement, CGSize)? {
        guard let video = model.videoSize, let placement = Placement.fit(video: video, in: pixelSize) else {
            return nil
        }
        return (placement, video)
    }

    /// Returns a point of the view in the video's pixels.
    private func videoPoint(_ point: CGPoint) -> CGPoint? {
        guard let (placement, video) = picture else { return nil }
        return placement.videoPoint(CGPoint(x: point.x * scale, y: point.y * scale), video: video)
    }

    /// Moves the pointer by a finger's movement of `delta` points.
    private func movePointer(by delta: CGPoint) {
        guard let session = model.session, let (placement, video) = picture else { return }
        let videoPerPoint = scale * video.width / placement.width * Self.pointerSpeed
        if model.relativePointer {
            qdp_session_mouse_move(session, Float(delta.x * videoPerPoint), Float(delta.y * videoPerPoint))
            return
        }
        pointer.x = min(max(pointer.x + delta.x * videoPerPoint, 0), video.width - 1)
        pointer.y = min(max(pointer.y + delta.y * videoPerPoint, 0), video.height - 1)
        qdp_session_pointer(session, Float(pointer.x), Float(pointer.y))
    }

    private func click(_ button: QdpButton) {
        guard let session = model.session else { return }
        qdp_session_button(session, button, true)
        qdp_session_button(session, button, false)
    }

    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        let delta = gesture.translation(in: view)
        gesture.setTranslation(.zero, in: view)
        movePointer(by: delta)
    }

    @objc private func scrolled(_ gesture: UIPanGestureRecognizer) {
        guard let session = model.session else { return }
        let delta = gesture.translation(in: view)
        gesture.setTranslation(.zero, in: view)
        // Content follows the fingers: moving them down scrolls up.
        let horizontal = scrollRemainder.width - delta.x * Self.unitsPerPoint
        let vertical = scrollRemainder.height + delta.y * Self.unitsPerPoint
        let whole = (horizontal.rounded(.towardZero), vertical.rounded(.towardZero))
        scrollRemainder = CGSize(width: horizontal - whole.0, height: vertical - whole.1)
        if whole.0 != 0 || whole.1 != 0 {
            qdp_session_scroll(session, Int32(whole.1), Int32(whole.0))
        }
    }

    @objc private func tapped(_ gesture: UITapGestureRecognizer) {
        click(QDP_BUTTON_LEFT)
    }

    @objc private func twoFingerTapped(_ gesture: UITapGestureRecognizer) {
        click(QDP_BUTTON_RIGHT)
    }

    @objc private func threeFingerTapped(_ gesture: UITapGestureRecognizer) {
        keyboard.becomeFirstResponder()
    }

    @objc private func held(_ gesture: UILongPressGestureRecognizer) {
        guard let session = model.session else { return }
        switch gesture.state {
        case .began:
            dragging = true
            qdp_session_button(session, QDP_BUTTON_LEFT, true)
        case .ended, .cancelled, .failed:
            if dragging {
                dragging = false
                qdp_session_button(session, QDP_BUTTON_LEFT, false)
            }
        default:
            break
        }
    }

    @objc private func hovered(_ gesture: UIHoverGestureRecognizer) {
        guard let session = model.session, !model.relativePointer,
              let point = videoPoint(gesture.location(in: view)) else { return }
        pointer = point
        qdp_session_pointer(session, Float(point.x), Float(point.y))
    }

    /// Follows a move of the host's pointer that the user did not make.
    private func warped(to point: CGPoint) {
        pointer = point
        if let session = model.session {
            qdp_session_cursor_at(session, Float(point.x), Float(point.y))
        }
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            if touch.type == .indirectPointer {
                pointerButton(touch, pressed: true, event: event)
            } else if touchMode == .direct {
                send(touch, phase: QDP_TOUCH_DOWN)
            }
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            if touch.type == .indirectPointer {
                pointerMoved(touch)
            } else if touchMode == .direct {
                send(touch, phase: QDP_TOUCH_MOVE)
            }
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            if touch.type == .indirectPointer {
                pointerButton(touch, pressed: false, event: event)
            } else if touchMode == .direct {
                send(touch, phase: QDP_TOUCH_UP)
            }
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            if touch.type == .indirectPointer {
                pointerButton(touch, pressed: false, event: event)
            } else if touchMode == .direct {
                send(touch, phase: QDP_TOUCH_CANCEL)
            }
        }
    }

    /// Sends a finger touching the host's touchscreen.
    private func send(_ touch: UITouch, phase: QdpTouchPhase) {
        guard let session = model.session, let point = videoPoint(touch.location(in: view)) else { return }
        let id = ObjectIdentifier(touch)
        let finger: UInt32
        if let known = fingers[id] {
            finger = known
        } else {
            finger = (0...UInt32(fingers.count)).first { !fingers.values.contains($0) } ?? 0
            fingers[id] = finger
        }
        if phase == QDP_TOUCH_UP || phase == QDP_TOUCH_CANCEL {
            fingers[id] = nil
        }
        let pressure = touch.maximumPossibleForce > 0 ? Float(touch.force / touch.maximumPossibleForce) : -1
        qdp_session_touch(session, finger, phase, Float(point.x), Float(point.y), pressure)
    }

    /// Presses or releases the button of an attached mouse or trackpad.
    private func pointerButton(_ touch: UITouch, pressed: Bool, event: UIEvent?) {
        guard let session = model.session, !model.relativePointer else { return }
        pointerMoved(touch)
        let button = event?.buttonMask.contains(.secondary) == true ? QDP_BUTTON_RIGHT : QDP_BUTTON_LEFT
        qdp_session_button(session, button, pressed)
    }

    private func pointerMoved(_ touch: UITouch) {
        guard let session = model.session, !model.relativePointer,
              let point = videoPoint(touch.location(in: view)) else { return }
        pointer = point
        qdp_session_pointer(session, Float(point.x), Float(point.y))
    }

    /// Follows attached mice, whose movement drives a relative pointer while
    /// it is locked.
    private func followMice() {
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: .GCMouseDidConnect, object: nil, queue: .main) { [weak self] note in
            guard let mouse = note.object as? GCMouse else { return }
            MainActor.assumeIsolated { self?.follow(mouse) }
        })
        GCMouse.mice().forEach(follow)
    }

    private func follow(_ mouse: GCMouse) {
        guard let input = mouse.mouseInput else { return }
        input.mouseMovedHandler = { [weak self] _, deltaX, deltaY in
            MainActor.assumeIsolated {
                guard let self, self.model.relativePointer, let session = self.model.session else { return }
                // The Game Controller framework reports up as positive.
                qdp_session_mouse_move(session, deltaX, -deltaY)
            }
        }
        let buttons: [(GCControllerButtonInput?, QdpButton)] = [
            (input.leftButton, QDP_BUTTON_LEFT), (input.rightButton, QDP_BUTTON_RIGHT), (input.middleButton, QDP_BUTTON_MIDDLE),
        ]
        for (control, button) in buttons {
            control?.pressedChangedHandler = { [weak self] _, _, pressed in
                MainActor.assumeIsolated {
                    guard let self, self.model.relativePointer, let session = self.model.session else { return }
                    qdp_session_button(session, button, pressed)
                }
            }
        }
    }

    // MARK: Keys

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !handle(presses, pressed: true) { super.pressesBegan(presses, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !handle(presses, pressed: false) { super.pressesEnded(presses, with: event) }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !handle(presses, pressed: false) { super.pressesCancelled(presses, with: event) }
    }

    /// Sends the keyboard keys among `presses`, returning whether there
    /// were any. A key's code is its HID usage.
    func handle(_ presses: Set<UIPress>, pressed: Bool) -> Bool {
        guard let session = model.session else { return false }
        var handled = false
        for press in presses {
            guard let key = press.key else { continue }
            qdp_session_key(session, UInt16(truncatingIfNeeded: key.keyCode.rawValue), pressed)
            handled = true
        }
        return handled
    }

    /// Sends text typed on the on-screen keyboard.
    func type(_ text: String) {
        guard let session = model.session else { return }
        if text == "\n" {
            tap(Self.returnUsage)
        } else {
            text.withCString { qdp_session_text(session, $0) }
        }
    }

    /// Presses and releases a key.
    func tap(_ usage: UInt16) {
        guard let session = model.session else { return }
        qdp_session_key(session, usage, true)
        qdp_session_key(session, usage, false)
    }
}

extension StreamViewController: UIGestureRecognizerDelegate {
    /// Moving and a held press go together, so that a held finger drags.
    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
    ) -> Bool {
        true
    }
}

/// A hidden field that shows the on-screen keyboard, passing what is typed,
/// and a row of keys it lacks, to the stream.
final class KeyboardField: UIView, UIKeyInput {
    weak var controller: StreamViewController?

    /// The usages of the extra keys: Escape, Tab and the arrows.
    private static let extraKeys: [(String, UInt16)] = [
        ("esc", 0x29), ("tab", 0x2B), ("←", 0x50), ("↑", 0x52), ("↓", 0x51), ("→", 0x4F),
    ]

    override var canBecomeFirstResponder: Bool { true }
    var hasText: Bool { true }
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no

    func insertText(_ text: String) {
        controller?.type(text)
    }

    func deleteBackward() {
        controller?.tap(StreamViewController.backspaceUsage)
    }

    override var inputAccessoryView: UIView? {
        let bar = UIToolbar(frame: CGRect(x: 0, y: 0, width: 320, height: 44))
        var items = Self.extraKeys.map { key in
            UIBarButtonItem(title: key.0, primaryAction: UIAction { [weak self] _ in self?.controller?.tap(key.1) })
        }
        items.append(.flexibleSpace())
        items.append(UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak self] _ in
            self?.resignFirstResponder()
            self?.controller?.becomeFirstResponder()
        }))
        bar.items = items
        return bar
    }

    // Hardware keys go to the stream as keys, not as text.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if controller?.handle(presses, pressed: true) != true { super.pressesBegan(presses, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if controller?.handle(presses, pressed: false) != true { super.pressesEnded(presses, with: event) }
    }
}
