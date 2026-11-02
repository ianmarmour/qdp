package io.github.ianmarmour.qdp

import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * One of the on-screen gamepad's controls as it is drawn: its [code], a
 * Standard Gamepad button index, [DPAD] or a stick; its frame in pixels;
 * whether a touch [held] it; a stick's tilt, each from -1 to 1, negative left
 * and up; and the directional pad's [directions] held, of [UP], [DOWN],
 * [LEFT] and [RIGHT].
 */
data class VirtualControl(
    val code: Int,
    val left: Float,
    val top: Float,
    val width: Float,
    val height: Float,
    val held: Boolean,
    val tiltX: Float,
    val tiltY: Float,
    val directions: Int,
) {
    companion object {
        /** The buttons' codes: their Standard Gamepad indices, A, B, X and Y the bottom, right, left and top. */
        const val A = 0
        const val B = 1
        const val X = 2
        const val Y = 3
        const val LEFT_SHOULDER = 4
        const val RIGHT_SHOULDER = 5
        const val LEFT_TRIGGER = 6
        const val RIGHT_TRIGGER = 7
        const val SELECT = 8
        const val START = 9
        const val LEFT_STICK_BUTTON = 10
        const val RIGHT_STICK_BUTTON = 11

        /** The directional pad. */
        const val DPAD = 12

        /** The left stick. */
        const val LEFT_STICK = 17

        /** The right stick. */
        const val RIGHT_STICK = 18

        /** [directions]' bit for up. */
        const val UP = 1

        /** [directions]' bit for down. */
        const val DOWN = 2

        /** [directions]' bit for left. */
        const val LEFT = 4

        /** [directions]' bit for right. */
        const val RIGHT = 8

        /** Returns the controls in [floats], as [Native.virtualGamepadControls] gives them. */
        fun list(floats: FloatArray?): List<VirtualControl> {
            if (floats == null) {
                return emptyList()
            }
            return (0 until floats.size / Native.VIRTUAL_CONTROL_FLOATS).map { index ->
                val at = index * Native.VIRTUAL_CONTROL_FLOATS
                VirtualControl(
                    code = floats[at].toInt(),
                    left = floats[at + 1],
                    top = floats[at + 2],
                    width = floats[at + 3],
                    height = floats[at + 4],
                    held = floats[at + 5] != 0f,
                    tiltX = floats[at + 6],
                    tiltY = floats[at + 7],
                    directions = floats[at + 8].toInt(),
                )
            }
        }
    }
}

/**
 * The on-screen gamepad, which the stream's menu shows and hides: while it
 * shows, every touch on the screen goes to it, the core works out the
 * gamepad's state from where they are and sends it as a gamepad of its own,
 * and [onControls] hears of the controls to draw. A press taps the device's
 * haptics, as a real button would click.
 */
class VirtualGamepadInput(private val view: View, private val onControls: (List<VirtualControl>) -> Unit) {
    /** Connects the gamepad to the session, or disconnects it with every control released. */
    fun connect(connected: Boolean) {
        Native.virtualGamepadConnected(connected)
        refresh()
    }

    /**
     * Lays the controls out on [view], inside its display cutout and the
     * system's edge gestures, so that a thumb on a stick does not swipe
     * Back.
     */
    fun layout() {
        val insets = ViewCompat.getRootWindowInsets(view)
            ?.getInsets(WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.systemGestures())
        Native.virtualGamepadLayout(
            view.width,
            view.height,
            view.resources.displayMetrics.density,
            insets?.left ?: 0,
            insets?.top ?: 0,
            insets?.right ?: 0,
            insets?.bottom ?: 0,
        )
        refresh()
    }

    /** Releases every control held, as when the stream's menu opens. */
    fun release() {
        Native.virtualGamepadRelease()
        refresh()
    }

    /** Takes a touch on the screen, in [view]'s coordinates. */
    fun onTouch(event: MotionEvent) {
        var outcome = 0
        fun send(index: Int, phase: Int) {
            outcome = outcome or
                Native.virtualGamepadTouch(event.getPointerId(index), phase, event.getX(index), event.getY(index))
        }
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> send(event.actionIndex, Native.TOUCH_DOWN)
            MotionEvent.ACTION_MOVE -> for (index in 0 until event.pointerCount) send(index, Native.TOUCH_MOVE)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> send(event.actionIndex, Native.TOUCH_UP)
            MotionEvent.ACTION_CANCEL -> for (index in 0 until event.pointerCount) send(index, Native.TOUCH_CANCEL)
        }
        if (outcome and Native.VIRTUAL_PRESSED != 0) {
            view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
        }
        if (outcome and Native.VIRTUAL_REDRAW != 0) {
            refresh()
        }
    }

    /** Tells [onControls] of the controls as they now are. */
    private fun refresh() {
        onControls(VirtualControl.list(Native.virtualGamepadControls()))
    }
}
