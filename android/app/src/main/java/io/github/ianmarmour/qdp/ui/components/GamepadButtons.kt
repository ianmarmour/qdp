package io.github.ianmarmour.qdp.ui.components

import android.hardware.input.InputManager
import android.view.InputDevice
import android.view.KeyEvent
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.platform.LocalContext
import io.github.ianmarmour.qdp.Gamepads

/**
 * The gamepad buttons the app's menus give a meaning to or swallow, by key
 * code rather than printed label, since Switch-style pads swap A and B. A
 * and B are left to Android, which clicks the focused item with A and goes
 * back with B.
 */
enum class GamepadButton(val keyCode: Int) {
    /** Deletes a digit, or adds a computer. */
    X(KeyEvent.KEYCODE_BUTTON_X),

    /** Opens the focused item's options. */
    Y(KeyEvent.KEYCODE_BUTTON_Y),

    /** Opens settings. */
    START(KeyEvent.KEYCODE_BUTTON_START),

    /** Does nothing in menus. */
    SELECT(KeyEvent.KEYCODE_BUTTON_SELECT),

    /** The left stick's click; does nothing in menus. */
    L3(KeyEvent.KEYCODE_BUTTON_THUMBL),

    /** The right stick's click; does nothing in menus. */
    R3(KeyEvent.KEYCODE_BUTTON_THUMBR);

    companion object {
        /** Returns the button with [keyCode], or `null` for another key. */
        fun of(keyCode: Int): GamepadButton? = entries.firstOrNull { it.keyCode == keyCode }
    }
}

/**
 * Calls [onPress] when a [GamepadButton] is pressed, and swallows those
 * buttons' presses and releases. Android sends a fallback key for a button
 * that an app leaves unhandled, and some of those click the focused item: Y
 * falls back to Space, and Start, L3 and R3 to D-pad center, so pressing any
 * of them would connect to the focused computer.
 */
fun Modifier.gamepadButtons(onPress: (GamepadButton) -> Unit = {}): Modifier = onPreviewKeyEvent { event ->
    val key = event.nativeKeyEvent
    val button = GamepadButton.of(key.keyCode) ?: return@onPreviewKeyEvent false
    if (key.action == KeyEvent.ACTION_DOWN && key.repeatCount == 0 && key.isFromSource(InputDevice.SOURCE_GAMEPAD)) {
        onPress(button)
    }
    true
}

/** Returns whether a gamepad is connected, following gamepads as they come and go. */
@Composable
fun rememberGamepadConnected(): Boolean {
    val context = LocalContext.current
    var connected by remember { mutableStateOf(Gamepads.anyConnected()) }
    DisposableEffect(context) {
        val manager = context.getSystemService(InputManager::class.java)
        val listener = object : InputManager.InputDeviceListener {
            override fun onInputDeviceAdded(deviceId: Int) {
                connected = Gamepads.anyConnected()
            }

            override fun onInputDeviceRemoved(deviceId: Int) {
                connected = Gamepads.anyConnected()
            }

            override fun onInputDeviceChanged(deviceId: Int) {
                connected = Gamepads.anyConnected()
            }
        }
        // A null handler calls the listener on this, the main, thread.
        manager?.registerInputDeviceListener(listener, null)
        connected = Gamepads.anyConnected()
        onDispose { manager?.unregisterInputDeviceListener(listener) }
    }
    return connected
}
