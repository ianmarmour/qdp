import AppKit
import QDPCore
import QuartzCore

/// The view a Mac stream draws into, which passes on the keyboard, mouse,
/// trackpad and gamepads, shows the host's cursor as the pointer, and
/// captures the mouse while the host's pointer is relative.
final class StreamNSView: NSView {
    private let model: StreamModel
    private var gamepads: Gamepads?
    private var displayLink: CADisplayLink?
    private var trackingArea: NSTrackingArea?
    private var observers: [NSObjectProtocol] = []
    private var cursor = NSCursor.arrow
    private var captured = false
    private var scrollRemainder = CGSize.zero
    private var pressedModifiers = Set<UInt16>()

    /// 1/120ths of a wheel detent per line a mouse wheel scrolls.
    private static let unitsPerLine = 120.0
    /// 1/120ths of a detent per point a trackpad scrolls.
    private static let unitsPerPoint = 3.0
    /// The frame rates the display is asked for while streaming.
    private static let frameRates = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
    /// Millihertz per hertz.
    private static let millihertz = 1000.0
    /// The refresh rate assumed when the screen's is unknown.
    private static let defaultRefresh = 60.0

    /// The virtual key codes of the modifier keys, which come as
    /// `flagsChanged` events, and the flag each one holds.
    private static let modifierKeys: [UInt16: NSEvent.ModifierFlags] = [
        0x38: .shift, 0x3C: .shift, 0x3B: .control, 0x3E: .control,
        0x3A: .option, 0x3D: .option, 0x37: .command, 0x36: .command,
        0x39: .capsLock, 0x3F: .function,
    ]
    /// The virtual key code of Z, which with Control, Option and Shift
    /// releases a captured mouse.
    private static let releaseKey: UInt16 = 0x06

    init(model: StreamModel) {
        self.model = model
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("not used")
    }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window, model.session == nil, let layer else { return }
        let refresh = Double(window.screen?.maximumFramesPerSecond ?? Int(Self.defaultRefresh))
        let started = model.start(
            layer: layer,
            pixels: pixelSize,
            scale: window.backingScaleFactor,
            refreshMillihertz: UInt32(refresh * Self.millihertz)
        )
        guard started else { return }
        window.makeFirstResponder(self)
        model.onCursorChanged = { [weak self] in self?.updateCursor() }
        model.onPointerMode = { [weak self] relative in self?.setCaptured(relative) }
        let gamepads = Gamepads(model: model)
        model.onRumble = { [weak gamepads] slot, low, high in gamepads?.rumble(slot: slot, low: low, high: high) }
        gamepads.start()
        self.gamepads = gamepads
        let link = displayLink(target: self, selector: #selector(step))
        link.preferredFrameRateRange = Self.frameRates
        link.add(to: .main, forMode: .common)
        displayLink = link
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.focusChanged(true) }
        })
        observers.append(center.addObserver(forName: NSWindow.didResignKeyNotification, object: window, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.focusChanged(false) }
        })
    }

    /// Stops everything the view started, as its window closes.
    func tearDown() {
        setCaptured(false)
        observers.forEach { NotificationCenter.default.removeObserver($0) }
        observers.removeAll()
        displayLink?.invalidate()
        displayLink = nil
        gamepads?.stop()
        gamepads = nil
        model.stop()
    }

    @objc private func step(_ link: CADisplayLink) {}

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        resized()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        resized()
        updateCursor()
    }

    private var pixelSize: CGSize {
        convertToBacking(bounds).size
    }

    private func resized() {
        guard let window else { return }
        model.resize(pixels: pixelSize, scale: window.backingScaleFactor)
        let refresh = Double(window.screen?.maximumFramesPerSecond ?? Int(Self.defaultRefresh))
        model.displayChanged(
            pixels: pixelSize,
            scale: window.backingScaleFactor,
            refreshMillihertz: UInt32(refresh * Self.millihertz)
        )
        window.invalidateCursorRects(for: self)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea { removeTrackingArea(trackingArea) }
        let area = NSTrackingArea(
            rect: .zero,
            options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect, .cursorUpdate],
            owner: self
        )
        addTrackingArea(area)
        trackingArea = area
    }

    // MARK: Keyboard

    override func keyDown(with event: NSEvent) {
        if event.keyCode == Self.releaseKey,
           event.modifierFlags.contains([.control, .option, .shift]), captured {
            setCaptured(false)
            return
        }
        guard !event.isARepeat else { return }
        key(event.keyCode, pressed: true)
    }

    override func keyUp(with event: NSEvent) {
        key(event.keyCode, pressed: false)
    }

    override func flagsChanged(with event: NSEvent) {
        guard let flag = Self.modifierKeys[event.keyCode] else { return }
        let pressed: Bool
        if flag == .capsLock {
            // Caps Lock reports its state, not the key: each change is a
            // press and release.
            key(event.keyCode, pressed: true)
            key(event.keyCode, pressed: false)
            return
        } else if pressedModifiers.contains(event.keyCode) {
            pressed = false
        } else {
            pressed = event.modifierFlags.contains(flag)
        }
        if pressed {
            pressedModifiers.insert(event.keyCode)
        } else {
            pressedModifiers.remove(event.keyCode)
        }
        key(event.keyCode, pressed: pressed)
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // While the mouse is captured, Command shortcuts go to the host, not
        // the app's menus.
        guard captured, window?.firstResponder === self else {
            return super.performKeyEquivalent(with: event)
        }
        if event.type == .keyDown {
            keyDown(with: event)
        }
        return true
    }

    private func key(_ keyCode: UInt16, pressed: Bool) {
        guard let session = model.session else { return }
        let usage = qdp_mac_key_usage(keyCode)
        if usage != 0 {
            qdp_session_key(session, usage, pressed)
        }
    }

    // MARK: Mouse

    override func mouseMoved(with event: NSEvent) { moved(event) }
    override func mouseDragged(with event: NSEvent) { moved(event) }
    override func rightMouseDragged(with event: NSEvent) { moved(event) }
    override func otherMouseDragged(with event: NSEvent) { moved(event) }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        button(QDP_BUTTON_LEFT, pressed: true, event: event)
    }

    override func mouseUp(with event: NSEvent) { button(QDP_BUTTON_LEFT, pressed: false, event: event) }
    override func rightMouseDown(with event: NSEvent) { button(QDP_BUTTON_RIGHT, pressed: true, event: event) }
    override func rightMouseUp(with event: NSEvent) { button(QDP_BUTTON_RIGHT, pressed: false, event: event) }

    override func otherMouseDown(with event: NSEvent) {
        if let button = Self.otherButton(event.buttonNumber) { self.button(button, pressed: true, event: event) }
    }

    override func otherMouseUp(with event: NSEvent) {
        if let button = Self.otherButton(event.buttonNumber) { self.button(button, pressed: false, event: event) }
    }

    override func scrollWheel(with event: NSEvent) {
        guard let session = model.session else { return }
        let units = event.hasPreciseScrollingDeltas ? Self.unitsPerPoint : Self.unitsPerLine
        // Positive deltas move the content right and down: scrolling left
        // and up, away from the user.
        let horizontal = scrollRemainder.width - event.scrollingDeltaX * units
        let vertical = scrollRemainder.height + event.scrollingDeltaY * units
        let whole = (horizontal.rounded(.towardZero), vertical.rounded(.towardZero))
        scrollRemainder = CGSize(width: horizontal - whole.0, height: vertical - whole.1)
        if whole.0 != 0 || whole.1 != 0 {
            qdp_session_scroll(session, Int32(whole.1), Int32(whole.0))
        }
    }

    private static func otherButton(_ number: Int) -> QdpButton? {
        switch number {
        case 2: QDP_BUTTON_MIDDLE
        case 3: QDP_BUTTON_BACK
        case 4: QDP_BUTTON_FORWARD
        default: nil
        }
    }

    private func moved(_ event: NSEvent) {
        guard let session = model.session else { return }
        if captured {
            qdp_session_mouse_move(session, Float(event.deltaX), Float(event.deltaY))
        } else if let position = videoPosition(of: event) {
            qdp_session_pointer(session, Float(position.x), Float(position.y))
        }
    }

    private func button(_ button: QdpButton, pressed: Bool, event: NSEvent) {
        guard let session = model.session else { return }
        if !captured, let position = videoPosition(of: event) {
            qdp_session_pointer(session, Float(position.x), Float(position.y))
        }
        qdp_session_button(session, button, pressed)
    }

    /// Returns where `event` happened in the video's pixels, clamped to the
    /// picture, once a frame arrived.
    private func videoPosition(of event: NSEvent) -> CGPoint? {
        guard let video = model.videoSize else { return nil }
        let point = convertToBacking(convert(event.locationInWindow, from: nil))
        // Backing coordinates run up from the bottom; the picture's rows run
        // down from the top.
        let pixels = pixelSize
        let fromTop = CGPoint(x: point.x, y: pixels.height - point.y)
        guard let rect = Placement.fit(video: video, in: pixels) else { return nil }
        return rect.videoPoint(fromTop, video: video)
    }

    // MARK: Cursor and capture

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: cursor)
    }

    override func cursorUpdate(with event: NSEvent) {
        cursor.set()
    }

    /// Makes the host's cursor the pointer over the view, at the size it has
    /// on the host's monitor (Section 9.2), drawn at the screen's pixels.
    private func updateCursor() {
        guard let session = model.session, let window, let video = model.videoSize,
              let rect = Placement.fit(video: video, in: pixelSize) else { return }
        var image = QdpCursor()
        guard qdp_session_cursor(session, &image) else {
            cursor = Self.hiddenCursor
            window.invalidateCursorRects(for: self)
            cursor.set()
            return
        }
        defer { qdp_cursor_free(&image) }
        let width = Int(image.width)
        let height = Int(image.height)
        guard width > 0, height > 0, let pixels = image.rgba,
              let bitmap = NSBitmapImageRep(
                  bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
                  bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                  colorSpaceName: .deviceRGB, bitmapFormat: .alphaNonpremultiplied,
                  bytesPerRow: width * 4, bitsPerPixel: 32
              ),
              let data = bitmap.bitmapData else { return }
        data.update(from: pixels, count: width * height * 4)
        // Screen pixels per image pixel: the picture's scale, by the image's
        // scale relative to the video's.
        let factor = rect.width / video.width * 100 / Double(max(image.scale_percent, 1))
        let points = factor / window.backingScaleFactor
        let size = NSSize(width: Double(width) * points, height: Double(height) * points)
        let picture = NSImage(size: size)
        picture.addRepresentation(bitmap)
        cursor = NSCursor(
            image: picture,
            hotSpot: NSPoint(x: Double(image.hotspot_x) * points, y: Double(image.hotspot_y) * points)
        )
        window.invalidateCursorRects(for: self)
        cursor.set()
    }

    /// A pointer that shows nothing, for a host whose cursor is hidden.
    private static let hiddenCursor: NSCursor = {
        let image = NSImage(size: NSSize(width: 1, height: 1))
        return NSCursor(image: image, hotSpot: .zero)
    }()

    /// Captures the mouse while the host's pointer is relative, so that it
    /// moves the host's pointer and not the Mac's, or releases it.
    private func setCaptured(_ capture: Bool) {
        let capture = capture && window?.isKeyWindow == true
        guard capture != captured else { return }
        captured = capture
        CGAssociateMouseAndMouseCursorPosition(capture ? 0 : 1)
        if capture {
            NSCursor.hide()
        } else {
            NSCursor.unhide()
        }
    }

    private func focusChanged(_ focused: Bool) {
        guard let session = model.session else { return }
        qdp_session_focus(session, focused)
        if focused {
            setCaptured(model.relativePointer)
        } else {
            setCaptured(false)
            pressedModifiers.removeAll()
        }
    }
}
