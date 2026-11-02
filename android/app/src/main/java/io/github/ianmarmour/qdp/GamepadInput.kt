package io.github.ianmarmour.qdp

import android.content.Context
import android.hardware.input.InputManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import kotlin.math.abs
import kotlin.math.max

/**
 * Sends the device's gamepads to the host: a handheld's built-in controls
 * and any connected controller, each in a slot of its own, with its whole
 * state on each change, and plays the host's rumble on them.
 */
class GamepadInput(
    context: Context,
    private val onMenuCombo: () -> Unit,
    private val onConnectedChanged: (Boolean) -> Unit,
) : InputManager.InputDeviceListener {
    private val inputManager = context.getSystemService(InputManager::class.java)
    private val systemVibrator: Vibrator? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        context.getSystemService(VibratorManager::class.java)?.defaultVibrator
    } else {
        @Suppress("DEPRECATION")
        context.getSystemService(Vibrator::class.java)
    }
    private val pads = HashMap<Int, Pad>()
    private var nextSlot = 0

    /** A gamepad and its state. */
    private class Pad(val slot: Int, val device: InputDevice, val vibrator: Vibrator?) {
        /** The buttons held, from key events. */
        var keys = 0

        /** The directional pad held, from its hat axes. */
        var hat = 0
        var leftX = 0f
        var leftY = 0f
        var rightX = 0f
        var rightY = 0f
        var leftTrigger = 0f
        var rightTrigger = 0f
        val hasLeftTrigger = device.getMotionRange(MotionEvent.AXIS_LTRIGGER) != null ||
            device.getMotionRange(MotionEvent.AXIS_BRAKE) != null
        val hasRightTrigger = device.getMotionRange(MotionEvent.AXIS_RTRIGGER) != null ||
            device.getMotionRange(MotionEvent.AXIS_GAS) != null

        /** The right stick's axes: Z and RZ, or RX and RY on some controllers. */
        val rightAxes = if (device.getMotionRange(MotionEvent.AXIS_Z) != null) {
            MotionEvent.AXIS_Z to MotionEvent.AXIS_RZ
        } else {
            MotionEvent.AXIS_RX to MotionEvent.AXIS_RY
        }

        /** Returns the buttons held, a mask of Standard Gamepad indices. */
        fun buttons(): Int {
            var buttons = keys or hat
            if (leftTrigger > TRIGGER_PRESSED) buttons = buttons or (1 shl LEFT_TRIGGER)
            if (rightTrigger > TRIGGER_PRESSED) buttons = buttons or (1 shl RIGHT_TRIGGER)
            return buttons
        }

        /** Sends the pad's state. */
        fun send() {
            // A trigger reported only as a button is pulled all the way.
            val left = if (hasLeftTrigger) leftTrigger else if (keys and (1 shl LEFT_TRIGGER) != 0) 1f else 0f
            val right = if (hasRightTrigger) rightTrigger else if (keys and (1 shl RIGHT_TRIGGER) != 0) 1f else 0f
            Native.gamepad(slot, buttons(), leftX, leftY, rightX, rightY, left, right)
        }
    }

    /**
     * Connects the gamepads there are, and follows those that come and go,
     * telling whether any is connected now and each time that changes.
     */
    fun start() {
        inputManager.registerInputDeviceListener(this, Handler(Looper.getMainLooper()))
        for (id in InputDevice.getDeviceIds()) {
            onInputDeviceAdded(id)
        }
        onConnectedChanged(pads.isNotEmpty())
    }

    /** Disconnects every gamepad and stops their rumble; [start] tells what is connected again. */
    fun stop() {
        inputManager.unregisterInputDeviceListener(this)
        for (pad in pads.values) {
            pad.vibrator?.cancel()
            Native.gamepadDisconnected(pad.slot)
        }
        pads.clear()
    }

    /**
     * Forgets the buttons, sticks and triggers held and sends every pad at
     * rest, as when the stream's menu takes the gamepads: the presses that
     * opened it are released to the menu, not to the host.
     */
    fun reset() {
        for (pad in pads.values) {
            pad.keys = 0
            pad.hat = 0
            pad.leftX = 0f
            pad.leftY = 0f
            pad.rightX = 0f
            pad.rightY = 0f
            pad.leftTrigger = 0f
            pad.rightTrigger = 0f
            pad.send()
        }
    }

    override fun onInputDeviceAdded(deviceId: Int) {
        val device = InputDevice.getDevice(deviceId) ?: return
        if (!Gamepads.isGamepad(device) || pads.containsKey(deviceId)) {
            return
        }
        val own = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            device.vibratorManager.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            device.vibrator
        }
        // A handheld's built-in controls rumble with the device's own motor.
        val vibrator = own.takeIf { it.hasVibrator() }
            ?: systemVibrator.takeIf { !device.isExternal && it?.hasVibrator() == true }
        val pad = Pad(nextSlot++, device, vibrator)
        pads[deviceId] = pad
        Native.gamepadConnected(pad.slot, device.name, vibrator != null)
        if (pads.size == 1) {
            onConnectedChanged(true)
        }
    }

    override fun onInputDeviceRemoved(deviceId: Int) {
        val pad = pads.remove(deviceId) ?: return
        pad.vibrator?.cancel()
        Native.gamepadDisconnected(pad.slot)
        if (pads.isEmpty()) {
            onConnectedChanged(false)
        }
    }

    override fun onInputDeviceChanged(deviceId: Int) {}

    /** Takes a gamepad's button, returning whether it was one. */
    fun onKey(event: KeyEvent): Boolean {
        val bit = BUTTONS[event.keyCode] ?: return false
        val pad = pad(event.deviceId) ?: return false
        if (event.repeatCount > 0) {
            return true
        }
        val mask = 1 shl bit
        pad.keys = when (event.action) {
            KeyEvent.ACTION_DOWN -> pad.keys or mask
            KeyEvent.ACTION_UP -> pad.keys and mask.inv()
            else -> return true
        }
        pad.send()
        if (pad.buttons() and MENU_COMBO == MENU_COMBO) {
            onMenuCombo()
        }
        return true
    }

    /** Takes a gamepad's sticks, triggers and hat, returning whether it was a gamepad's motion. */
    fun onMotion(event: MotionEvent): Boolean {
        if (!event.isFromSource(InputDevice.SOURCE_JOYSTICK) || event.actionMasked != MotionEvent.ACTION_MOVE) {
            return false
        }
        val pad = pad(event.deviceId) ?: return false
        pad.leftX = axis(event, MotionEvent.AXIS_X)
        pad.leftY = axis(event, MotionEvent.AXIS_Y)
        pad.rightX = axis(event, pad.rightAxes.first)
        pad.rightY = axis(event, pad.rightAxes.second)
        pad.leftTrigger = max(axis(event, MotionEvent.AXIS_LTRIGGER), axis(event, MotionEvent.AXIS_BRAKE))
        pad.rightTrigger = max(axis(event, MotionEvent.AXIS_RTRIGGER), axis(event, MotionEvent.AXIS_GAS))
        val hatX = event.getAxisValue(MotionEvent.AXIS_HAT_X)
        val hatY = event.getAxisValue(MotionEvent.AXIS_HAT_Y)
        pad.hat = (if (hatX < -HAT_PRESSED) 1 shl DPAD_LEFT else 0) or
            (if (hatX > HAT_PRESSED) 1 shl DPAD_RIGHT else 0) or
            (if (hatY < -HAT_PRESSED) 1 shl DPAD_UP else 0) or
            (if (hatY > HAT_PRESSED) 1 shl DPAD_DOWN else 0)
        pad.send()
        return true
    }

    /** Plays a gamepad's rumble, the stronger of its motors, until told otherwise. */
    fun rumble(slot: Int, low: Int, high: Int) {
        val vibrator = pads.values.firstOrNull { it.slot == slot }?.vibrator ?: return
        val amplitude = max(low, high) * MAX_AMPLITUDE / MAX_RUMBLE
        if (amplitude == 0) {
            vibrator.cancel()
        } else {
            // The pattern repeats until cancelled.
            vibrator.vibrate(
                VibrationEffect.createWaveform(longArrayOf(RUMBLE_PERIOD_MILLIS), intArrayOf(amplitude), 0),
            )
        }
    }

    /** Returns the pad for [deviceId], connecting it if it is a gamepad not seen yet. */
    private fun pad(deviceId: Int): Pad? {
        pads[deviceId]?.let { return it }
        onInputDeviceAdded(deviceId)
        return pads[deviceId]
    }

    /** Returns [axis]'s value, 0 within the device's dead zone. */
    private fun axis(event: MotionEvent, axis: Int): Float {
        val range = event.device?.getMotionRange(axis, event.source) ?: return 0f
        val value = event.getAxisValue(axis)
        return if (abs(value) <= range.flat) 0f else value
    }

    private companion object {
        /** Standard Gamepad button indices. */
        const val LEFT_TRIGGER = 6
        const val RIGHT_TRIGGER = 7
        const val DPAD_UP = 12
        const val DPAD_DOWN = 13
        const val DPAD_LEFT = 14
        const val DPAD_RIGHT = 15

        /** Android's gamepad key codes and their Standard Gamepad indices. */
        val BUTTONS = mapOf(
            KeyEvent.KEYCODE_BUTTON_A to 0,
            KeyEvent.KEYCODE_BUTTON_B to 1,
            KeyEvent.KEYCODE_BUTTON_X to 2,
            KeyEvent.KEYCODE_BUTTON_Y to 3,
            KeyEvent.KEYCODE_BUTTON_L1 to 4,
            KeyEvent.KEYCODE_BUTTON_R1 to 5,
            KeyEvent.KEYCODE_BUTTON_L2 to LEFT_TRIGGER,
            KeyEvent.KEYCODE_BUTTON_R2 to RIGHT_TRIGGER,
            KeyEvent.KEYCODE_BUTTON_SELECT to 8,
            KeyEvent.KEYCODE_BUTTON_START to 9,
            KeyEvent.KEYCODE_BUTTON_THUMBL to 10,
            KeyEvent.KEYCODE_BUTTON_THUMBR to 11,
            KeyEvent.KEYCODE_DPAD_UP to DPAD_UP,
            KeyEvent.KEYCODE_DPAD_DOWN to DPAD_DOWN,
            KeyEvent.KEYCODE_DPAD_LEFT to DPAD_LEFT,
            KeyEvent.KEYCODE_DPAD_RIGHT to DPAD_RIGHT,
            KeyEvent.KEYCODE_BUTTON_MODE to 16,
        )

        /** Select, Start, and both bumpers together open the stream's menu. */
        const val MENU_COMBO = (1 shl 8) or (1 shl 9) or (1 shl 4) or (1 shl 5)

        /** How far a trigger is pulled to count as pressed. */
        const val TRIGGER_PRESSED = 0.5f

        /** How far a hat axis goes to count as pressed. */
        const val HAT_PRESSED = 0.5f

        const val MAX_RUMBLE = 65535
        const val MAX_AMPLITUDE = 255

        /** The length of one repetition of a rumble's pattern. */
        const val RUMBLE_PERIOD_MILLIS = 1000L
    }
}
