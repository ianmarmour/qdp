package io.github.ianmarmour.qdp

import android.Manifest
import android.annotation.SuppressLint
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.Display
import android.view.Gravity
import android.view.InputDevice
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.PointerIcon
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.ViewConfiguration
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.WindowManager
import android.widget.FrameLayout
import androidx.activity.BackEventCompat
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.compose.ui.platform.ComposeView
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import io.github.ianmarmour.qdp.ui.stream.ConnectingStep
import io.github.ianmarmour.qdp.ui.stream.GatewaySignInUi
import io.github.ianmarmour.qdp.ui.stream.PairingStep
import io.github.ianmarmour.qdp.ui.stream.PairingUi
import io.github.ianmarmour.qdp.ui.stream.StatsLevel
import io.github.ianmarmour.qdp.ui.stream.StreamActions
import io.github.ianmarmour.qdp.ui.stream.StreamNotice
import io.github.ianmarmour.qdp.ui.stream.StreamOverlay
import io.github.ianmarmour.qdp.ui.stream.StreamPhase
import io.github.ianmarmour.qdp.ui.stream.StreamUiState
import io.github.ianmarmour.qdp.ui.stream.Visibility
import io.github.ianmarmour.qdp.ui.theme.QdpTheme
import kotlin.concurrent.thread
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Streams the host's desktop full screen: its video in a surface the core
 * decodes into, the host's cursor over it, and the user's gamepads, touches,
 * mouse and keyboard sent back. The session runs while the surface exists,
 * so that leaving the app ends it and coming back connects again.
 *
 * A Compose overlay draws everything else over the video: the connecting
 * screen, the reconnecting banner, statistics, and the menu, which Back or
 * Select, Start and both bumpers together open, with the on-screen keyboard,
 * the touch mode, statistics and disconnecting. While the menu or a status
 * screen shows, input goes to the overlay instead of the host.
 *
 * A stream that pairs asks for the code the host shows, and keeps the host
 * once paired, connecting to it with the device's key from then on.
 */
class StreamActivity : ComponentActivity(), SurfaceHolder.Callback, VideoArea {
    private lateinit var settings: Settings
    private lateinit var target: StreamTarget
    private lateinit var ui: StreamUiState
    private lateinit var root: FrameLayout
    private lateinit var videoFrame: FrameLayout
    private lateinit var surfaceView: SurfaceView
    private lateinit var cursorView: CursorView
    private lateinit var overlayView: ComposeView
    private lateinit var typing: TypingView
    private lateinit var gamepads: GamepadInput
    private lateinit var touch: TouchInput
    private var wifiLock: WifiManager.WifiLock? = null
    private var events: Thread? = null

    /** Whether the event thread keeps waiting for events. */
    @Volatile
    private var listening = false

    /** Whether a session runs in the surface. */
    private var streaming = false

    /** Whether the core waits for a pairing code, which it is answered exactly once. */
    private var codeRequested = false

    /** Whether the core waits for the gateway's sign-in, which it is answered exactly once. */
    private var signInRequested = false
    private var refreshMillihertz = DEFAULT_REFRESH_MILLIHERTZ
    private var relative = false

    override var videoWidth = 0
        private set
    override var videoHeight = 0
        private set

    /** What the overlay's buttons do. */
    private val actions = StreamActions(
        resume = ::closeMenu,
        showKeyboard = ::showKeyboard,
        toggleTouchMode = ::toggleTouchMode,
        cycleStats = ::cycleStats,
        disconnect = ::leave,
        submitCode = ::answerCode,
        cancelPairing = ::cancelPairing,
        submitSignIn = ::answerSignIn,
        submitSignInCode = ::answerSignInCode,
        cancelSignIn = ::cancelSignIn,
        tryAgain = ::tryAgain,
        leave = ::leave,
    )

    /**
     * Back closes the menu, following a predictive back gesture as it goes;
     * cancels while connecting; and opens the menu otherwise. Android 16
     * sends apps that target it neither onBackPressed nor the Back key.
     */
    private val back = object : OnBackPressedCallback(true) {
        override fun handleOnBackProgressed(backEvent: BackEventCompat) {
            if (ui.menu == Visibility.SHOWN) {
                ui.backProgress = backEvent.progress
            }
        }

        override fun handleOnBackCancelled() {
            ui.backProgress = 0f
        }

        override fun handleOnBackPressed() {
            ui.backProgress = 0f
            when {
                ui.menu == Visibility.SHOWN -> closeMenu()
                ui.phase == StreamPhase.CONNECTING -> leave()
                else -> openMenu()
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        Native.init(applicationContext)
        settings = Settings.load(this)
        target = StreamTarget.from(intent) ?: run {
            finish()
            return
        }
        ui = StreamUiState(
            hostName = target.name,
            deviceName = DeviceName.of(this),
            touchMode = settings.touchMode,
            statsLevel = if (settings.showStats) StatsLevel.SUMMARY else StatsLevel.HIDDEN,
        )
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val fastest = fastestMode()
        window.attributes = window.attributes.apply {
            layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            if (fastest != null) {
                preferredDisplayModeId = fastest.modeId
            }
        }
        if (fastest != null) {
            refreshMillihertz = (fastest.refreshRate * MILLIHERTZ).roundToInt()
        }

        surfaceView = SurfaceView(this).apply {
            isFocusable = true
            isFocusableInTouchMode = true
            holder.addCallback(this@StreamActivity)
            setOnCapturedPointerListener { _, event -> capturedPointer(event) }
        }
        cursorView = CursorView(this)
        videoFrame = FrameLayout(this).apply {
            addView(surfaceView, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
            addView(cursorView, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
        }
        typing = TypingView(this) { event -> keyboard(event) }
        overlayView = ComposeView(this).apply {
            setContent {
                QdpTheme(settings.colorSource, dark = true) {
                    StreamOverlay(ui, actions, gatewayUser = Gateway.load(this@StreamActivity)?.user.orEmpty())
                }
            }
        }
        root = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(videoFrame, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT, Gravity.CENTER))
            addView(typing, FrameLayout.LayoutParams(1, 1))
            addView(overlayView, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
            // The host's cursor is drawn instead of Android's.
            pointerIcon = hiddenPointer()
            addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> layoutVideo() }
        }
        setContentView(root)
        onBackPressedDispatcher.addCallback(this, back)

        gamepads = GamepadInput(this) { openMenu() }
        touch = TouchInput(this, ViewConfiguration.get(this)) { x, y -> cursorView.moveTo(x, y) }.apply {
            mode = settings.touchMode
        }
        wifiLock = getSystemService(WifiManager::class.java)
            ?.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, WIFI_LOCK_TAG)
            ?.apply { setReferenceCounted(false) }
        immersive()
        surfaceView.requestFocus()
    }

    override fun onResume() {
        super.onResume()
        wifiLock?.acquire()
        immersive()
    }

    override fun onPause() {
        wifiLock?.release()
        super.onPause()
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        start(holder.surface)
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        stop()
    }

    /** Starts a session showing its video in [surface]. */
    private fun start(surface: Surface) {
        if (streaming) {
            return
        }
        val size = StreamSize.requested(StreamSize.screen(currentDisplay()), settings.resolution)
        videoWidth = size.width
        videoHeight = size.height
        layoutVideo()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // The display runs at the stream's rate.
            surface.setFrameRate(refreshMillihertz / MILLIHERTZ, Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE)
        }
        ui.error = null
        ui.pairing = null
        ui.gatewaySignIn = null
        ui.menu = Visibility.HIDDEN
        ui.connectingStep = ConnectingStep.REACHING
        setPhase(StreamPhase.CONNECTING)
        val microphone = settings.microphone &&
            checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        // A computer reached through the gateway goes through the one set
        // now, with the session its last sign-in left.
        val gateway = Gateway.load(this).takeIf { target.authentication == Native.AUTH_GATEWAY }
        val error = Native.connect(
            surface,
            target.address,
            target.authentication,
            target.pin,
            target.token,
            ui.deviceName,
            gateway?.address.orEmpty(),
            gateway?.pin.orEmpty(),
            if (gateway != null) Gateway.sessionToken(this) else "",
            size.width,
            size.height,
            refreshMillihertz,
            settings.decoder.code,
            microphone,
        )
        if (error != null) {
            ui.error = StreamError.of(error)
            return
        }
        streaming = true
        listening = true
        events = thread(name = "qdp-events", isDaemon = true) { listen() }
        gamepads.start()
        Native.focus(hasWindowFocus())
    }

    /** Ends the session, returning once the surface is no longer used. */
    private fun stop() {
        if (!streaming) {
            return
        }
        streaming = false
        listening = false
        codeRequested = false
        signInRequested = false
        events?.join()
        events = null
        touch.cancel()
        gamepads.stop()
        if (relative) {
            surfaceView.releasePointerCapture()
        }
        Native.disconnect()
    }

    /** Ends the session and leaves the stream. */
    private fun leave() {
        stop()
        finish()
    }

    /** Connects again after a failure, in the surface there is. */
    private fun tryAgain() {
        ui.error = null
        ui.pairing = null
        val surface = surfaceView.holder.surface
        if (surface != null && surface.isValid) {
            start(surface)
        }
    }

    /** Waits for the session's events and handles them on the main thread. */
    private fun listen() {
        while (listening) {
            val line = Native.nextEvent(EVENT_WAIT_MILLIS) ?: continue
            val event = SessionEvent.parse(line) ?: continue
            runOnUiThread { onEvent(event) }
            if (event is SessionEvent.Ended || event is SessionEvent.Failed) {
                return
            }
        }
    }

    /** Handles one of the session's events. */
    private fun onEvent(event: SessionEvent) {
        if (!streaming) {
            return
        }
        when (event) {
            SessionEvent.Connecting -> ui.connectingStep = ConnectingStep.REACHING
            SessionEvent.Connected -> when (ui.phase) {
                StreamPhase.CONNECTING -> ui.connectingStep = ConnectingStep.STARTING
                // A connection made again shows the video at its size, so
                // no video event follows.
                StreamPhase.RECONNECTING -> setPhase(StreamPhase.LIVE)
                StreamPhase.LIVE -> Unit
            }
            is SessionEvent.Reconnecting -> {
                ui.reconnectAt = SystemClock.uptimeMillis() + event.delayMillis
                if (ui.phase == StreamPhase.LIVE) {
                    setPhase(StreamPhase.RECONNECTING)
                }
            }
            is SessionEvent.Video -> {
                videoWidth = event.width
                videoHeight = event.height
                layoutVideo()
                setPhase(StreamPhase.LIVE)
            }
            SessionEvent.Cursor -> cursorView.setImage(Native.cursorImage())
            is SessionEvent.Warp -> {
                cursorView.moveTo(event.x, event.y)
                touch.pointerAt(event.x, event.y)
            }
            is SessionEvent.PointerMode -> setRelative(event.relative)
            is SessionEvent.Rumble -> gamepads.rumble(event.slot, event.low, event.high)
            is SessionEvent.Stats -> ui.stats = event
            is SessionEvent.PairingCode -> askForCode(event.retry)
            is SessionEvent.GatewaySignIn -> {
                signInRequested = true
                ui.gatewaySignIn = GatewaySignInUi(event.gateway, event.retry, code = false)
            }
            is SessionEvent.GatewayCode -> {
                signInRequested = true
                ui.gatewaySignIn = GatewaySignInUi(event.gateway, event.retry, code = true, enrollment = event.enrollment)
            }
            is SessionEvent.GatewaySession -> Gateway.keepSession(this, event.token, event.lifetimeSeconds)
            is SessionEvent.Paired -> {
                // From now on the host is reached with the device's key,
                // also when the stream connects again.
                PairedHosts.keep(this, PairedHost(target.name, target.address, event.pin))
                target = target.copy(authentication = Native.AUTH_PAIRED, pin = event.pin)
                ui.pairing = null
                ui.notice = StreamNotice.PAIRED
            }
            SessionEvent.Ended -> leave()
            is SessionEvent.Failed -> {
                stop()
                ui.gatewaySignIn = null
                val error = StreamError.of(event.error)
                // Pairing's own dialog explains a failure while pairing.
                if (ui.pairing != null) {
                    ui.pairing = PairingUi(PairingStep.FAILED, failure = error)
                } else {
                    ui.error = error
                }
                routeInput()
            }
        }
    }

    /** Moves the stream to [phase], showing the menu's tip the first time the video is live. */
    private fun setPhase(phase: StreamPhase) {
        ui.phase = phase
        if (phase == StreamPhase.LIVE && Tip.STREAM_MENU.pending(this)) {
            ui.coachmark = Visibility.SHOWN
            Tip.STREAM_MENU.shown(this)
        }
        routeInput()
    }

    /** Asks for the code the host shows; [retry] after a code that did not match. */
    private fun askForCode(retry: Boolean) {
        codeRequested = true
        val wrongCodes = ui.pairing?.wrongCodes ?: 0
        ui.pairing = PairingUi(PairingStep.ENTERING, wrongCodes = if (retry) wrongCodes + 1 else wrongCodes)
    }

    /** Answers the core's request for a pairing code with [code]. */
    private fun answerCode(code: String) {
        if (!codeRequested) {
            return
        }
        codeRequested = false
        Native.pairingCode(code)
        ui.pairing = ui.pairing?.copy(step = PairingStep.CHECKING)
    }

    /** Declines to pair, which ends the session, or leaves if no code is awaited. */
    private fun cancelPairing() {
        ui.pairing = null
        if (codeRequested) {
            codeRequested = false
            // The core ends the session, whose end leaves the stream.
            Native.pairingCode(null)
        } else {
            leave()
        }
    }

    /** Answers the gateway's request for the user's name and password. */
    private fun answerSignIn(user: String, password: String) {
        if (!signInRequested) {
            return
        }
        signInRequested = false
        ui.gatewaySignIn = null
        Native.signIn(user, password)
    }

    /** Answers the gateway's request for a code from the user's authenticator app. */
    private fun answerSignInCode(code: String) {
        if (!signInRequested) {
            return
        }
        signInRequested = false
        ui.gatewaySignIn = null
        Native.signInCode(code)
    }

    /** Declines to sign in, which ends the session, or leaves if nothing is asked. */
    private fun cancelSignIn() {
        ui.gatewaySignIn = null
        if (signInRequested) {
            signInRequested = false
            // The core ends the session, whose end leaves the stream.
            Native.signIn(null, "")
        } else {
            leave()
        }
    }

    /** Follows the host's pointer mode: in relative mode the mouse is captured, unless the overlay takes input. */
    private fun setRelative(relative: Boolean) {
        this.relative = relative
        touch.relative = relative
        cursorView.shown = !relative
        if (relative && !ui.capturesTouch) {
            surfaceView.requestFocus()
            surfaceView.requestPointerCapture()
        } else {
            surfaceView.releasePointerCapture()
        }
    }

    /** Opens the stream's menu, releasing everything held so that the host sees nothing stuck. */
    private fun openMenu() {
        if (ui.menu == Visibility.SHOWN || ui.phase == StreamPhase.CONNECTING) {
            return
        }
        ui.menu = Visibility.SHOWN
        ui.coachmark = Visibility.HIDDEN
        touch.cancel()
        gamepads.reset()
        Native.releaseAll()
        typing.hide()
        if (relative) {
            surfaceView.releasePointerCapture()
        }
        routeInput()
    }

    /** Closes the stream's menu. */
    private fun closeMenu() {
        if (ui.menu == Visibility.HIDDEN) {
            return
        }
        ui.menu = Visibility.HIDDEN
        routeInput()
    }

    /** Closes the menu and shows the on-screen keyboard. */
    private fun showKeyboard() {
        closeMenu()
        // Once focus has left the menu.
        root.post { typing.show() }
    }

    /** Switches between trackpad and direct touch, keeping the choice. */
    private fun toggleTouchMode() {
        val mode = if (touch.mode == TouchMode.TRACKPAD) TouchMode.DIRECT else TouchMode.TRACKPAD
        touch.mode = mode
        ui.touchMode = mode
        settings = settings.copy(touchMode = mode).also { it.save(this) }
    }

    /** Shows more statistics, or none after the most, keeping whether they show. */
    private fun cycleStats() {
        ui.statsLevel = ui.statsLevel.next()
        settings = settings.copy(showStats = ui.statsLevel != StatsLevel.HIDDEN).also { it.save(this) }
    }

    /**
     * Gives input back to the host once the overlay no longer takes it:
     * focus and the pointer capture return to the video, and Android's
     * pointer, which the overlay shows while it takes input, hides again.
     */
    private fun routeInput() {
        if (ui.capturesTouch) {
            return
        }
        overlayView.pointerIcon = hiddenPointer()
        surfaceView.requestFocus()
        if (relative) {
            surfaceView.requestPointerCapture()
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (streaming) {
            Native.focus(hasFocus)
        }
        if (!hasFocus) {
            touch.cancel()
            return
        }
        immersive()
        if (relative && !ui.capturesTouch) {
            surfaceView.requestPointerCapture()
        }
    }

    // androidx.core's ComponentActivity marks its override of the framework's
    // public Activity.dispatchKeyEvent as restricted, which lint reports for
    // every override; overriding it is the framework's own API.
    @SuppressLint("RestrictedApi")
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (ui.capturesKeys) {
            return super.dispatchKeyEvent(event)
        }
        if (gamepads.onKey(event)) {
            return true
        }
        return when (event.keyCode) {
            // Volume stays the device's. Back is no key the host is sent, so
            // it goes on to the back callback, which opens the menu.
            KeyEvent.KEYCODE_VOLUME_UP,
            KeyEvent.KEYCODE_VOLUME_DOWN,
            KeyEvent.KEYCODE_VOLUME_MUTE,
            -> super.dispatchKeyEvent(event)
            else -> keyboard(event) || super.dispatchKeyEvent(event)
        }
    }

    /** Sends a key from a keyboard or the on-screen keyboard, returning whether it was sent. */
    private fun keyboard(event: KeyEvent): Boolean {
        // Some input methods still send text as one event of characters.
        @Suppress("DEPRECATION")
        if (event.action == KeyEvent.ACTION_MULTIPLE) {
            val characters = event.characters ?: return false
            Native.text(characters)
            return true
        }
        // The host repeats held keys itself.
        if (event.repeatCount > 0) {
            return true
        }
        val pressed = event.action == KeyEvent.ACTION_DOWN
        if (Native.key(event.keyCode, pressed)) {
            return true
        }
        // A key QDP has no usage for still types its character.
        val character = event.unicodeChar
        if (pressed && character != 0 && character and KeyCharacterMap.COMBINING_ACCENT == 0) {
            Native.text(String(Character.toChars(character)))
            return true
        }
        return false
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        if (ui.capturesTouch) {
            return super.dispatchTouchEvent(event)
        }
        if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            return mouse(event)
        }
        return touch.onTouch(event) || super.dispatchTouchEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        // Unhandled, a gamepad's sticks and hat move focus in the menu as a D-pad would.
        if (ui.capturesKeys && event.isFromSource(InputDevice.SOURCE_JOYSTICK)) {
            return super.dispatchGenericMotionEvent(event)
        }
        if (gamepads.onMotion(event)) {
            return true
        }
        if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            return if (ui.capturesTouch) super.dispatchGenericMotionEvent(event) else mouse(event)
        }
        return super.dispatchGenericMotionEvent(event)
    }

    /** Sends a mouse's motion, buttons and wheel, while the pointer is not captured. */
    private fun mouse(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_HOVER_MOVE, MotionEvent.ACTION_MOVE -> if (!relative) {
                val x = videoX(event.x)
                val y = videoY(event.y)
                Native.pointer(x, y)
                cursorView.moveTo(x, y)
                touch.pointerAt(x, y)
            }
            MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE,
            MotionEvent.ACTION_SCROLL,
            -> mouseButtonsAndWheel(event)
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP,
            MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_EXIT,
            -> {}
            else -> return false
        }
        return true
    }

    /** Sends the captured mouse's motion, buttons and wheel, in relative mode. */
    private fun capturedPointer(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_MOVE, MotionEvent.ACTION_HOVER_MOVE -> {
                if (!event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE)) {
                    return false
                }
                var dx = event.x
                var dy = event.y
                for (index in 0 until event.historySize) {
                    dx += event.getHistoricalX(index)
                    dy += event.getHistoricalY(index)
                }
                Native.mouseMove(dx, dy)
            }
            MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE,
            MotionEvent.ACTION_SCROLL,
            -> mouseButtonsAndWheel(event)
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP -> {}
            else -> return false
        }
        return true
    }

    /** Sends a mouse button pressed or released, or its wheel turned. */
    private fun mouseButtonsAndWheel(event: MotionEvent) {
        when (event.actionMasked) {
            MotionEvent.ACTION_SCROLL -> Native.scroll(
                (event.getAxisValue(MotionEvent.AXIS_VSCROLL) * WHEEL_DETENT).roundToInt(),
                (event.getAxisValue(MotionEvent.AXIS_HSCROLL) * WHEEL_DETENT).roundToInt(),
            )
            else -> {
                val button = when (event.actionButton) {
                    MotionEvent.BUTTON_PRIMARY -> Native.BUTTON_LEFT
                    MotionEvent.BUTTON_SECONDARY -> Native.BUTTON_RIGHT
                    MotionEvent.BUTTON_TERTIARY -> Native.BUTTON_MIDDLE
                    MotionEvent.BUTTON_BACK -> Native.BUTTON_BACK
                    MotionEvent.BUTTON_FORWARD -> Native.BUTTON_FORWARD
                    else -> return
                }
                Native.button(button, event.actionMasked == MotionEvent.ACTION_BUTTON_PRESS)
            }
        }
    }

    /** Hides the system bars, which a swipe shows for a moment. */
    private fun immersive() {
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    /** Returns the pointer icon that shows nothing, as the host's cursor is drawn instead. */
    private fun hiddenPointer(): PointerIcon = PointerIcon.getSystemIcon(this, PointerIcon.TYPE_NULL)

    /** Returns the display the activity is on. */
    private fun currentDisplay(): Display? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        display
    } else {
        @Suppress("DEPRECATION")
        windowManager.defaultDisplay
    }

    /** Returns the display's fastest mode at its current resolution. */
    private fun fastestMode(): Display.Mode? {
        val display = currentDisplay() ?: return null
        val current = display.mode
        return display.supportedModes
            .filter { it.physicalWidth == current.physicalWidth && it.physicalHeight == current.physicalHeight }
            .maxByOrNull { it.refreshRate }
    }

    /** Sizes the video to fit the screen with its aspect ratio. */
    private fun layoutVideo() {
        val width = root.width
        val height = root.height
        if (width == 0 || height == 0 || videoWidth == 0 || videoHeight == 0) {
            return
        }
        val scale = min(width.toFloat() / videoWidth, height.toFloat() / videoHeight)
        val fittedWidth = (videoWidth * scale).roundToInt()
        val fittedHeight = (videoHeight * scale).roundToInt()
        val params = videoFrame.layoutParams as FrameLayout.LayoutParams
        if (params.width != fittedWidth || params.height != fittedHeight) {
            params.width = fittedWidth
            params.height = fittedHeight
            params.gravity = Gravity.CENTER
            videoFrame.layoutParams = params
        }
        cursorView.setVideoWidth(videoWidth)
    }

    override fun videoX(x: Float): Float =
        ((x - videoFrame.left) * videoPerView()).coerceIn(0f, (videoWidth - 1).coerceAtLeast(0).toFloat())

    override fun videoY(y: Float): Float =
        ((y - videoFrame.top) * videoPerView()).coerceIn(0f, (videoHeight - 1).coerceAtLeast(0).toFloat())

    override fun videoPerView(): Float = videoWidth.toFloat() / videoFrame.width.coerceAtLeast(1)

    private companion object {
        /** How long the event thread waits for each event before checking it should stop. */
        const val EVENT_WAIT_MILLIS = 100

        /** The refresh rate asked for when the display's is unknown. */
        const val DEFAULT_REFRESH_MILLIHERTZ = 60_000

        /** Millihertz per hertz. */
        const val MILLIHERTZ = 1000f

        /** 1/120ths of a wheel detent per detent. */
        const val WHEEL_DETENT = 120f

        const val WIFI_LOCK_TAG = "qdp:stream"
    }
}
