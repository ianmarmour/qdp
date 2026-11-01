package io.github.ianmarmour.qdp

import android.view.InputDevice

/** Recognizes gamepads among the device's inputs, a handheld's built-in controls among them. */
object Gamepads {
    /** Returns whether [device] is a gamepad or joystick. */
    fun isGamepad(device: InputDevice): Boolean {
        if (device.isVirtual) {
            return false
        }
        val sources = device.sources
        return sources and InputDevice.SOURCE_GAMEPAD == InputDevice.SOURCE_GAMEPAD ||
            sources and InputDevice.SOURCE_JOYSTICK == InputDevice.SOURCE_JOYSTICK
    }

    /** Returns whether any gamepad is connected. */
    fun anyConnected(): Boolean =
        InputDevice.getDeviceIds().any { id -> InputDevice.getDevice(id)?.let(::isGamepad) == true }
}
