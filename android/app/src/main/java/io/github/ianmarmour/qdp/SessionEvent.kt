package io.github.ianmarmour.qdp

/** Something the core tells the app, from a line [Native.nextEvent] returns. */
sealed interface SessionEvent {
    /** Connecting to the host. */
    data object Connecting : SessionEvent

    /** Connected; the session is starting. */
    data object Connected : SessionEvent

    /** The connection was lost, and is made again in [delayMillis]. */
    data class Reconnecting(val delayMillis: Long, val error: String) : SessionEvent

    /** The video's size changed. */
    data class Video(val width: Int, val height: Int) : SessionEvent

    /** The host's cursor changed; [Native.cursorImage] returns it. */
    data object Cursor : SessionEvent

    /**
     * The host's pointer moved without the user, to [x], [y]; [moves] if the
     * app's pointer follows, rather than only the cursor drawn.
     */
    data class Warp(val x: Float, val y: Float, val moves: Boolean) : SessionEvent

    /** The host's pointer mode changed; in relative mode the pointer is captured. */
    data class PointerMode(val relative: Boolean) : SessionEvent

    /** A gamepad's rumble, each motor from 0 to 65535. */
    data class Rumble(val slot: Int, val low: Int, val high: Int) : SessionEvent

    /** The last second's video. */
    data class Stats(val fps: Int, val latencyMillis: Float?, val decodeMillis: Float) : SessionEvent

    /** The session ended normally. */
    data object Ended : SessionEvent

    /** The session failed. */
    data class Failed(val error: String) : SessionEvent

    companion object {
        /** Returns the event a line describes, or `null` for one this app does not know. */
        fun parse(line: String): SessionEvent? {
            val word = line.substringBefore(' ')
            val rest = line.substringAfter(' ', "")
            val values = rest.split(' ')
            return when (word) {
                "connecting" -> Connecting
                "connected" -> Connected
                "reconnecting" -> Reconnecting(
                    values[0].toLongOrNull() ?: 0,
                    rest.substringAfter(' ', ""),
                )
                "video" -> Video(values[0].toInt(), values[1].toInt())
                "cursor" -> Cursor
                "warp" -> Warp(values[0].toFloat(), values[1].toFloat(), values[2] == "move")
                "pointer" -> PointerMode(values[0] == "relative")
                "rumble" -> Rumble(values[0].toInt(), values[1].toInt(), values[2].toInt())
                "stats" -> Stats(values[0].toInt(), values[1].toFloatOrNull(), values[2].toFloat())
                "ended" -> Ended
                "failed" -> Failed(rest)
                else -> null
            }
        }
    }
}
