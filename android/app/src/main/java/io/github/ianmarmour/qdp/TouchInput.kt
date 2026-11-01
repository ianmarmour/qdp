package io.github.ianmarmour.qdp

import android.os.Handler
import android.os.Looper
import android.view.MotionEvent
import android.view.ViewConfiguration
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.roundToInt

/** Where the video is on screen, to convert between its pixels and the view's. */
interface VideoArea {
    /** The video's width in pixels. */
    val videoWidth: Int

    /** The video's height in pixels. */
    val videoHeight: Int

    /** Returns the video's horizontal pixel at the view's [x]. */
    fun videoX(x: Float): Float

    /** Returns the video's vertical pixel at the view's [y]. */
    fun videoY(y: Float): Float

    /** Returns the video's pixels per view pixel. */
    fun videoPerView(): Float
}

/**
 * Turns touches into input: the host's own touches where fingers land, or
 * a trackpad's pointer motion, clicks and scrolling.
 *
 * As a trackpad, one finger moves the pointer, a tap clicks, a two-finger
 * tap right-clicks, a three-finger tap middle-clicks, a press held without
 * moving holds the left button to drag, and two fingers scroll.
 */
class TouchInput(
    private val area: VideoArea,
    viewConfiguration: ViewConfiguration,
    private val onPointer: (Float, Float) -> Unit,
) {
    /** What touching does. */
    var mode = TouchMode.TRACKPAD
        set(value) {
            cancel()
            field = value
        }

    /** Whether the host captures the pointer, so that motion is sent as a mouse's. */
    var relative = false

    private val handler = Handler(Looper.getMainLooper())
    private val slop = viewConfiguration.scaledTouchSlop.toFloat()
    private var pointerX = Float.NaN
    private var pointerY = Float.NaN
    private var downAt = 0L
    private var downX = 0f
    private var downY = 0f
    private var lastX = 0f
    private var lastY = 0f
    private var fingers = 0
    private var moved = false
    private var dragging = false
    private var scrollX = 0f
    private var scrollY = 0f
    private val startDrag = Runnable {
        if (!moved && fingers == 1) {
            dragging = true
            Native.button(Native.BUTTON_LEFT, true)
        }
    }

    /** Takes a touch on the video, returning whether it was used. */
    fun onTouch(event: MotionEvent): Boolean = when (mode) {
        TouchMode.DIRECT -> direct(event)
        TouchMode.TRACKPAD -> trackpad(event)
    }

    /** Moves the trackpad's pointer to [x], [y] in video pixels, as when the host warps it. */
    fun pointerAt(x: Float, y: Float) {
        pointerX = x
        pointerY = y
    }

    /** Ends a drag or touch in progress, as when the app loses focus. */
    fun cancel() {
        handler.removeCallbacks(startDrag)
        if (dragging) {
            Native.button(Native.BUTTON_LEFT, false)
        }
        dragging = false
        fingers = 0
    }

    /** Sends the touches as the host's own. */
    private fun direct(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN ->
                touch(event, event.actionIndex, Native.TOUCH_DOWN)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP ->
                touch(event, event.actionIndex, Native.TOUCH_UP)
            MotionEvent.ACTION_MOVE -> for (index in 0 until event.pointerCount) {
                touch(event, index, Native.TOUCH_MOVE)
            }
            MotionEvent.ACTION_CANCEL -> for (index in 0 until event.pointerCount) {
                touch(event, index, Native.TOUCH_CANCEL)
            }
            else -> return false
        }
        return true
    }

    /** Sends finger [index] of [event] in [phase]. */
    private fun touch(event: MotionEvent, index: Int, phase: Int) {
        Native.touch(
            event.getPointerId(index),
            phase,
            area.videoX(event.getX(index)),
            area.videoY(event.getY(index)),
            UNKNOWN_PRESSURE,
        )
    }

    /** Uses the touches as a trackpad. */
    private fun trackpad(event: MotionEvent): Boolean {
        val (x, y) = centroid(event)
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downAt = event.eventTime
                downX = x
                downY = y
                lastX = x
                lastY = y
                fingers = 1
                moved = false
                dragging = false
                handler.postDelayed(startDrag, DRAG_DELAY_MILLIS)
            }
            MotionEvent.ACTION_POINTER_DOWN, MotionEvent.ACTION_POINTER_UP -> {
                handler.removeCallbacks(startDrag)
                if (event.actionMasked == MotionEvent.ACTION_POINTER_DOWN) {
                    fingers = maxOf(fingers, event.pointerCount)
                }
                // The centroid jumps as fingers come and go.
                val (remainingX, remainingY) = centroid(event, without = event.actionIndex)
                lastX = remainingX
                lastY = remainingY
            }
            MotionEvent.ACTION_MOVE -> {
                if (!moved && hypot(x - downX, y - downY) > slop) {
                    moved = true
                    handler.removeCallbacks(startDrag)
                }
                if (moved) {
                    val dx = x - lastX
                    val dy = y - lastY
                    if (event.pointerCount == 1) move(dx, dy) else scroll(dx, dy)
                }
                lastX = x
                lastY = y
            }
            MotionEvent.ACTION_UP -> {
                handler.removeCallbacks(startDrag)
                if (dragging) {
                    Native.button(Native.BUTTON_LEFT, false)
                    dragging = false
                } else if (!moved && event.eventTime - downAt < TAP_MILLIS) {
                    click(
                        when (fingers) {
                            1 -> Native.BUTTON_LEFT
                            2 -> Native.BUTTON_RIGHT
                            else -> Native.BUTTON_MIDDLE
                        },
                    )
                }
                fingers = 0
                scrollX = 0f
                scrollY = 0f
            }
            MotionEvent.ACTION_CANCEL -> cancel()
            else -> return false
        }
        return true
    }

    /** Moves the pointer by [dx], [dy] view pixels. */
    private fun move(dx: Float, dy: Float) {
        val scale = area.videoPerView() * TRACKPAD_SPEED
        if (relative) {
            Native.mouseMove(dx * scale, dy * scale)
            return
        }
        if (pointerX.isNaN()) {
            pointerX = area.videoWidth / 2f
            pointerY = area.videoHeight / 2f
        }
        pointerX = (pointerX + dx * scale).coerceIn(0f, (area.videoWidth - 1).toFloat())
        pointerY = (pointerY + dy * scale).coerceIn(0f, (area.videoHeight - 1).toFloat())
        Native.pointer(pointerX, pointerY)
        onPointer(pointerX, pointerY)
    }

    /** Scrolls as two fingers moved by [dx], [dy] view pixels: the content follows them. */
    private fun scroll(dx: Float, dy: Float) {
        scrollX += -dx * SCROLL_PER_PIXEL
        scrollY += dy * SCROLL_PER_PIXEL
        val horizontal = scrollX.roundToInt()
        val vertical = scrollY.roundToInt()
        if (horizontal != 0 || vertical != 0) {
            // Mostly vertical scrolling stays vertical.
            val keepHorizontal = abs(scrollX) > abs(scrollY) / 2
            Native.scroll(vertical, if (keepHorizontal) horizontal else 0)
            scrollX -= horizontal
            scrollY -= vertical
        }
    }

    /** Clicks [button]. */
    private fun click(button: Int) {
        Native.button(button, true)
        Native.button(button, false)
    }

    /** Returns the middle of [event]'s fingers, leaving out finger [without]. */
    private fun centroid(event: MotionEvent, without: Int = -1): Pair<Float, Float> {
        var x = 0f
        var y = 0f
        var count = 0
        for (index in 0 until event.pointerCount) {
            if (index != without) {
                x += event.getX(index)
                y += event.getY(index)
                count++
            }
        }
        return if (count == 0) event.x to event.y else x / count to y / count
    }

    private companion object {
        /** Tells the host the screen does not sense pressure. */
        const val UNKNOWN_PRESSURE = -1f

        /** How much faster the pointer moves than the finger, across the video. */
        const val TRACKPAD_SPEED = 1.5f

        /** The longest touch that is a tap. */
        const val TAP_MILLIS = 250L

        /** How long a still finger waits before holding the left button. */
        const val DRAG_DELAY_MILLIS = 400L

        /** 1/120ths of a wheel detent per pixel two fingers move. */
        const val SCROLL_PER_PIXEL = 3f
    }
}
