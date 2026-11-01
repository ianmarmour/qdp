package io.github.ianmarmour.qdp

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.util.TypedValue
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
import android.view.View
import android.view.ViewConfiguration
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.TextView
import kotlin.concurrent.thread
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Streams the host's desktop full screen: its video in a surface the core
 * decodes into, the host's cursor over it, and the user's gamepads, touches,
 * mouse and keyboard sent back. The session runs while the surface exists,
 * so that leaving the app ends it and coming back connects again.
 *
 * Back, or Select, Start and both bumpers together, opens a menu with the
 * on-screen keyboard, the touch mode, statistics and disconnecting.
 */
class StreamActivity : Activity(), SurfaceHolder.Callback, VideoArea {
    private lateinit var settings: Settings
    private lateinit var root: FrameLayout
    private lateinit var videoFrame: FrameLayout
    private lateinit var surfaceView: SurfaceView
    private lateinit var cursorView: CursorView
    private lateinit var statsView: TextView
    private lateinit var statusView: TextView
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
    private var refreshMillihertz = DEFAULT_REFRESH_MILLIHERTZ
    private var relative = false
    private var menuOpen = false
    private var keyboardWanted = false
    private var statsShown = false

    override var videoWidth = 0
        private set
    override var videoHeight = 0
        private set

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Native.init(applicationContext)
        settings = Settings.load(this)
        statsShown = settings.showStats
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
        statsView = overlayText(Gravity.TOP or Gravity.START).apply {
            visibility = if (statsShown) View.VISIBLE else View.GONE
        }
        statusView = overlayText(Gravity.CENTER)
        root = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(videoFrame, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT, Gravity.CENTER))
            addView(typing, FrameLayout.LayoutParams(1, 1))
            addView(statsView, FrameLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT, Gravity.TOP or Gravity.START))
            addView(statusView, FrameLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT, Gravity.CENTER))
            // The host's cursor is drawn instead of Android's.
            pointerIcon = PointerIcon.getSystemIcon(this@StreamActivity, PointerIcon.TYPE_NULL)
            addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> layoutVideo() }
        }
        setContentView(root)

        gamepads = GamepadInput(this) { openMenu() }
        touch = TouchInput(this, ViewConfiguration.get(this)) { x, y -> cursorView.moveTo(x, y) }.apply {
            mode = settings.touchMode
        }
        wifiLock = getSystemService(WifiManager::class.java)
            ?.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, WIFI_LOCK_TAG)
            ?.apply { setReferenceCounted(false) }
        immersive()
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
        val (width, height) = requestedSize()
        videoWidth = width
        videoHeight = height
        layoutVideo()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // The display runs at the stream's rate.
            surface.setFrameRate(refreshMillihertz / MILLIHERTZ, Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE)
        }
        val microphone = settings.microphone &&
            checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val error = Native.connect(
            surface,
            settings.host,
            settings.pin,
            settings.token,
            width,
            height,
            refreshMillihertz,
            settings.decoder.code,
            microphone,
        )
        if (error != null) {
            fail(error)
            return
        }
        streaming = true
        status("Connecting to ${settings.host}…")
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
        events?.join()
        events = null
        touch.cancel()
        gamepads.stop()
        if (relative) {
            surfaceView.releasePointerCapture()
        }
        Native.disconnect()
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
            SessionEvent.Connecting -> status("Connecting to ${settings.host}…")
            SessionEvent.Connected -> status("Starting the stream…")
            is SessionEvent.Reconnecting -> status("Connection lost; reconnecting…")
            is SessionEvent.Video -> {
                videoWidth = event.width
                videoHeight = event.height
                layoutVideo()
                status(null)
            }
            SessionEvent.Cursor -> cursorView.setImage(Native.cursorImage())
            is SessionEvent.Warp -> {
                cursorView.moveTo(event.x, event.y)
                touch.pointerAt(event.x, event.y)
            }
            is SessionEvent.PointerMode -> setRelative(event.relative)
            is SessionEvent.Rumble -> gamepads.rumble(event.slot, event.low, event.high)
            is SessionEvent.Stats -> {
                val latency = event.latencyMillis?.let { "%.1f ms".format(it) } ?: "– ms"
                val summary = "%d fps · %s · decode %.1f ms".format(event.fps, latency, event.decodeMillis)
                // The slowest frame shows which stage a spike came from.
                statsView.text = event.slowest?.let {
                    summary + "\nslowest %.1f ms: host + network %.1f · transfer %.1f · waiting %.1f · decode %.1f"
                        .format(it.latencyMillis, it.toFirstFragmentMillis, it.transferMillis, it.queuedMillis, it.decodeMillis)
                } ?: summary
            }
            SessionEvent.Ended -> {
                stop()
                finish()
            }
            is SessionEvent.Failed -> {
                stop()
                fail(event.error)
            }
        }
    }

    /** Shows [error] and leaves the stream once it is read. */
    private fun fail(error: String) {
        status(null)
        AlertDialog.Builder(this)
            .setTitle("Disconnected")
            .setMessage(error)
            .setPositiveButton(android.R.string.ok) { _, _ -> finish() }
            .setOnCancelListener { finish() }
            .show()
    }

    /** Shows [text] over the video, or nothing for `null`. */
    private fun status(text: String?) {
        statusView.text = text
        statusView.visibility = if (text == null) View.GONE else View.VISIBLE
    }

    /** Follows the host's pointer mode: in relative mode the mouse is captured. */
    private fun setRelative(relative: Boolean) {
        this.relative = relative
        touch.relative = relative
        cursorView.shown = !relative
        if (relative) {
            surfaceView.requestFocus()
            surfaceView.requestPointerCapture()
        } else {
            surfaceView.releasePointerCapture()
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
        if (relative) {
            surfaceView.requestPointerCapture()
        }
        if (keyboardWanted) {
            keyboardWanted = false
            typing.show()
        }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (gamepads.onKey(event)) {
            return true
        }
        return when (event.keyCode) {
            // Volume stays the device's, and Back opens the menu.
            KeyEvent.KEYCODE_VOLUME_UP,
            KeyEvent.KEYCODE_VOLUME_DOWN,
            KeyEvent.KEYCODE_VOLUME_MUTE,
            KeyEvent.KEYCODE_BACK,
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
        if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            return mouse(event)
        }
        return touch.onTouch(event) || super.dispatchTouchEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (gamepads.onMotion(event)) {
            return true
        }
        if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            return mouse(event)
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

    @Deprecated("Back opens the stream's menu")
    @Suppress("DEPRECATION")
    override fun onBackPressed() {
        openMenu()
    }

    /** Opens the stream's menu. */
    private fun openMenu() {
        if (menuOpen) {
            return
        }
        menuOpen = true
        Native.releaseAll()
        val otherMode = if (touch.mode == TouchMode.TRACKPAD) TouchMode.DIRECT else TouchMode.TRACKPAD
        val items = arrayOf(
            "Keyboard",
            "Touch as ${otherMode.label.lowercase()}",
            if (statsShown) "Hide statistics" else "Show statistics",
            "Disconnect",
        )
        AlertDialog.Builder(this)
            .setItems(items) { _, which ->
                when (which) {
                    MENU_KEYBOARD -> keyboardWanted = true
                    MENU_TOUCH -> {
                        touch.mode = otherMode
                        settings = settings.copy(touchMode = otherMode).also { it.save(this) }
                    }
                    MENU_STATS -> {
                        statsShown = !statsShown
                        statsView.visibility = if (statsShown) View.VISIBLE else View.GONE
                        settings = settings.copy(showStats = statsShown).also { it.save(this) }
                    }
                    MENU_DISCONNECT -> {
                        stop()
                        finish()
                    }
                }
            }
            .setOnDismissListener { menuOpen = false }
            .show()
    }

    /** Hides the system bars, which a swipe shows for a moment. */
    private fun immersive() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.VANILLA_ICE_CREAM) {
                // Android 15 and later always draw behind the system bars.
                @Suppress("DEPRECATION")
                window.setDecorFitsSystemWindows(false)
            }
            window.insetsController?.apply {
                hide(WindowInsets.Type.systemBars())
                systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
        }
    }

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

    /** Returns the display size to ask the host for, landscape. */
    private fun requestedSize(): Pair<Int, Int> {
        val mode = currentDisplay()?.mode
        val long = max(mode?.physicalWidth ?: FALLBACK_WIDTH, mode?.physicalHeight ?: FALLBACK_HEIGHT)
        val short = min(mode?.physicalWidth ?: FALLBACK_WIDTH, mode?.physicalHeight ?: FALLBACK_HEIGHT)
        val height = settings.resolution.height
        if (height == 0) {
            return long to short
        }
        // The chosen height at the screen's aspect ratio, an even width.
        val width = (height * long.toFloat() / short).roundToInt() and 1.inv()
        return width to height
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

    /** Returns text drawn over the video. */
    private fun overlayText(gravity: Int): TextView = TextView(this).apply {
        this.gravity = gravity
        setTextColor(Color.WHITE)
        setShadowLayer(4f, 0f, 0f, Color.BLACK)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
        val padding = (12 * resources.displayMetrics.density).roundToInt()
        setPadding(padding, padding, padding, padding)
    }

    private companion object {
        /** How long the event thread waits for each event before checking it should stop. */
        const val EVENT_WAIT_MILLIS = 100

        /** The refresh rate asked for when the display's is unknown. */
        const val DEFAULT_REFRESH_MILLIHERTZ = 60_000

        /** Millihertz per hertz. */
        const val MILLIHERTZ = 1000f

        /** 1/120ths of a wheel detent per detent. */
        const val WHEEL_DETENT = 120f

        /** The display size assumed when the display's is unknown. */
        const val FALLBACK_WIDTH = 1920
        const val FALLBACK_HEIGHT = 1080

        const val WIFI_LOCK_TAG = "qdp:stream"

        const val MENU_KEYBOARD = 0
        const val MENU_TOUCH = 1
        const val MENU_STATS = 2
        const val MENU_DISCONNECT = 3
    }
}
