package io.github.ianmarmour.qdp

import android.content.Context
import android.view.Surface

/**
 * The app's Rust core, `libqdp_android.so`: it runs one session at a time,
 * decoding its video into the surface given to [connect].
 *
 * Positions are in video pixels. Input sent while no session runs is
 * dropped.
 */
object Native {
    init {
        System.loadLibrary("qdp_android")
    }

    /** Decodes in hardware when the device can, and in software otherwise. */
    const val DECODER_AUTO = 0

    /** Decodes with MediaCodec, failing without an AV1 decoder. */
    const val DECODER_HARDWARE = 1

    /** Decodes in software, with rav1d. */
    const val DECODER_SOFTWARE = 2

    /** A finger touched the screen. */
    const val TOUCH_DOWN = 0

    /** A finger moved. */
    const val TOUCH_MOVE = 1

    /** A finger lifted. */
    const val TOUCH_UP = 2

    /** A touch ended without effect. */
    const val TOUCH_CANCEL = 3

    /** [connect] presents a token. */
    const val AUTH_TOKEN = 0

    /** [connect] reaches a host the device paired with, which accepts the device's key. */
    const val AUTH_PAIRED = 1

    /** [connect] pairs with the code the host shows, asking for it with a `pair` event. */
    const val AUTH_PAIR = 2

    /**
     * [connect] reaches the host through the organization's gateway, which
     * vouches for the user, asking for their password and a code with
     * `signin` and `totp` events.
     */
    const val AUTH_GATEWAY = 3

    /** The left pointer button. */
    const val BUTTON_LEFT = 0

    /** The right pointer button. */
    const val BUTTON_RIGHT = 1

    /** The middle pointer button. */
    const val BUTTON_MIDDLE = 2

    /** The back pointer button. */
    const val BUTTON_BACK = 3

    /** The forward pointer button. */
    const val BUTTON_FORWARD = 4

    /** [virtualGamepadTouch]'s bit saying the on-screen gamepad looks different. */
    const val VIRTUAL_REDRAW = 1

    /** [virtualGamepadTouch]'s bit saying a button or direction was pressed. */
    const val VIRTUAL_PRESSED = 2

    /** The numbers [virtualGamepadControls] gives each control. */
    const val VIRTUAL_CONTROL_FLOATS = 9

    /** Sets up logging, and gives the core the app's context and its files directory for the device's key. */
    fun init(context: Context) {
        init(context, context.filesDir.absolutePath)
    }

    @JvmStatic private external fun init(context: Context, filesDirectory: String)

    /**
     * Starts a session with [host] (an address or name, with or without a
     * port), authenticating as [authentication], one of the `AUTH_` values,
     * says: with the host's key pin [pin] and [token], or the device's name
     * [deviceName] for pairing. Through a gateway, [gatewayAddress] is its
     * address, [gatewayPin] its key pin if the user set one, and
     * [gatewayToken] the token of the last `gatewaysession` event, all empty
     * otherwise, and [host] is the computer's ID on the gateway or its name
     * inside the network. It shows the session in [surface] on a display of
     * [width] × [height] pixels refreshing at [refreshMillihertz]. Returns
     * `null` once it has started, or why it could not.
     */
    @JvmStatic external fun connect(
        surface: Surface,
        host: String,
        authentication: Int,
        pin: String,
        token: String,
        deviceName: String,
        gatewayAddress: String,
        gatewayPin: String,
        gatewayToken: String,
        width: Int,
        height: Int,
        refreshMillihertz: Int,
        decoder: Int,
        microphone: Boolean,
    ): String?

    /** Ends the session, returning once [connect]'s surface is no longer used. */
    @JvmStatic external fun disconnect()

    /**
     * Returns the session's next event as a line of text, waiting up to
     * [timeoutMillis] for one, or `null` if none came or no session runs.
     */
    @JvmStatic external fun nextEvent(timeoutMillis: Int): String?

    /**
     * Returns the host's cursor: its width, height, hotspot column and
     * hotspot row, then its pixels as `0xAARRGGBB`; or `null` while hidden.
     */
    @JvmStatic external fun cursorImage(): IntArray?

    /** Sends a key, returning whether it is one the host is sent. */
    @JvmStatic external fun key(keyCode: Int, pressed: Boolean): Boolean

    /** Sends text typed without key presses. */
    @JvmStatic external fun text(text: String)

    /** Moves the pointer to [x], [y]. */
    @JvmStatic external fun pointer(x: Float, y: Float)

    /** Moves the captured mouse by [dx], [dy]. */
    @JvmStatic external fun mouseMove(dx: Float, dy: Float)

    /** Presses or releases a pointer button, one of the `BUTTON_` values. */
    @JvmStatic external fun button(button: Int, pressed: Boolean)

    /** Scrolls by 1/120ths of a wheel detent, positive up and right. */
    @JvmStatic external fun scroll(vertical: Int, horizontal: Int)

    /**
     * Sends a finger's touch, in one of the `TOUCH_` phases, pressing with
     * [pressure] from 0 to 1, or a negative pressure if unknown.
     */
    @JvmStatic external fun touch(finger: Int, phase: Int, x: Float, y: Float, pressure: Float)

    /** Connects gamepad [slot]. */
    @JvmStatic external fun gamepadConnected(slot: Int, name: String, rumble: Boolean)

    /** Disconnects gamepad [slot]. */
    @JvmStatic external fun gamepadDisconnected(slot: Int)

    /**
     * Sends gamepad [slot]'s state: [buttons] a mask of Standard Gamepad
     * button indices, sticks from -1 to 1 (negative left and up), triggers
     * from 0 to 1.
     */
    @JvmStatic external fun gamepad(
        slot: Int,
        buttons: Int,
        leftX: Float,
        leftY: Float,
        rightX: Float,
        rightY: Float,
        leftTrigger: Float,
        rightTrigger: Float,
    )

    /** Connects the on-screen gamepad as a gamepad of its own, or disconnects it with every control released. */
    @JvmStatic external fun virtualGamepadConnected(connected: Boolean)

    /**
     * Lays the on-screen gamepad out on a screen [width] × [height] pixels,
     * [density] pixels a dp, inside the safe area the insets leave.
     */
    @JvmStatic external fun virtualGamepadLayout(
        width: Int,
        height: Int,
        density: Float,
        left: Int,
        top: Int,
        right: Int,
        bottom: Int,
    )

    /**
     * Takes a finger's touch on the on-screen gamepad, in one of the
     * `TOUCH_` phases at [x], [y] on the screen, sending the gamepad's state
     * if it changed; returns [VIRTUAL_REDRAW] and [VIRTUAL_PRESSED] bits.
     */
    @JvmStatic external fun virtualGamepadTouch(finger: Int, phase: Int, x: Float, y: Float): Int

    /** Releases every control of the on-screen gamepad held. */
    @JvmStatic external fun virtualGamepadRelease()

    /**
     * Returns the on-screen gamepad's controls, [VIRTUAL_CONTROL_FLOATS]
     * numbers each: its code, its frame's left, top, width and height in
     * pixels, 1 if held, a stick's tilt across and down, and the
     * directional pad's directions held.
     */
    @JvmStatic external fun virtualGamepadControls(): FloatArray?

    /** Tells the host the display changed. */
    @JvmStatic external fun display(width: Int, height: Int, refreshMillihertz: Int)

    /** Tells the host whether the app has focus; losing it releases everything. */
    @JvmStatic external fun focus(focused: Boolean)

    /** Turns the microphone on or off. */
    @JvmStatic external fun microphone(on: Boolean)

    /** Releases every key and button held. */
    @JvmStatic external fun releaseAll()

    /** Answers a `pair` event with the [code] the user typed, or `null` if they declined, which ends the session. */
    @JvmStatic external fun pairingCode(code: String?)

    /**
     * Answers a `signin` event with the [user]'s name and [password], or
     * declines with a `null` user, which ends the session.
     */
    @JvmStatic external fun signIn(user: String?, password: String)

    /** Answers a `totp` event with the [code] the user typed, or `null` if they declined, which ends the session. */
    @JvmStatic external fun signInCode(code: String?)

    /**
     * Returns the QR code of [text]: its modules on a side, then each
     * module, row by row, 1 if dark and 0 if light; or `null` if it is too
     * long for a QR code.
     */
    @JvmStatic external fun qrModules(text: String): IntArray?

    /** Returns the device's key pin, for `qdp serve --client-key`, or `null` if its key cannot be kept. */
    @JvmStatic external fun clientPin(): String?

    /** Returns whether [pin] is a key pin. */
    @JvmStatic external fun isPin(pin: String): Boolean

    /**
     * Returns whether a QDP host answers at [address], "host" or
     * "host:port", without connecting to it. Blocks for up to a second and
     * a half: call it off the main thread.
     */
    @JvmStatic external fun hostReachable(address: String): Boolean
}
