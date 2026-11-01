package io.github.ianmarmour.qdp

import android.view.Display
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** A display size in pixels, landscape: [width] is the long side. */
data class StreamSize(val width: Int, val height: Int) {
    companion object {
        /** The display size assumed when the display's is unknown. */
        private val FALLBACK = StreamSize(1920, 1080)

        /** Returns the size of [display]'s current mode, landscape. */
        fun screen(display: Display?): StreamSize {
            val mode = display?.mode ?: return FALLBACK
            return StreamSize(
                max(mode.physicalWidth, mode.physicalHeight),
                min(mode.physicalWidth, mode.physicalHeight),
            )
        }

        /**
         * Returns the size to ask the host for on [screen]: the screen's own,
         * or [resolution]'s height at the screen's aspect ratio, with an even
         * width.
         */
        fun requested(screen: StreamSize, resolution: Resolution): StreamSize {
            val height = resolution.height
            if (height == 0) {
                return screen
            }
            val width = (height * screen.width.toFloat() / screen.height).roundToInt() and 1.inv()
            return StreamSize(width, height)
        }
    }
}
