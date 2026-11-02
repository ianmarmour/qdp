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

    /** The last second's video, and its slowest frame if any frame's latency is known. */
    data class Stats(
        val fps: Int,
        val latencyMillis: Float?,
        val decodeMillis: Float,
        val slowest: SlowestFrame?,
    ) : SessionEvent

    /**
     * A second's slowest frame: its latency, and where it went, in
     * milliseconds: from capture to its first fragment (the host's encoding
     * and the network), its transfer, waiting for the decoder, and decoding.
     */
    data class SlowestFrame(
        val latencyMillis: Float,
        val toFirstFragmentMillis: Float,
        val transferMillis: Float,
        val queuedMillis: Float,
        val decodeMillis: Float,
    )

    /**
     * Pairing needs the code the host shows, which [Native.pairingCode]
     * answers; [retry] after a code that did not work.
     */
    data class PairingCode(val retry: Boolean) : SessionEvent

    /** Pairing succeeded with the host whose key pin is [pin]. */
    data class Paired(val pin: String) : SessionEvent

    /**
     * The gateway named [gateway] needs the user's name and password, which
     * [Native.signIn] answers; [retry] after an attempt that failed.
     */
    data class GatewaySignIn(val retry: Boolean, val gateway: String) : SessionEvent

    /**
     * The gateway named [gateway] needs a code from the user's authenticator
     * app, which [Native.signInCode] answers; [enrollment] is an
     * `otpauth://` URI to add to the app first, if it has no secret for the
     * gateway yet; [retry] after a code that did not work.
     */
    data class GatewayCode(val retry: Boolean, val enrollment: String?, val gateway: String) : SessionEvent

    /**
     * The gateway gave a session [token], in hexadecimal, valid for
     * [lifetimeSeconds], with which the next sessions sign in without
     * asking the user.
     */
    data class GatewaySession(val token: String, val lifetimeSeconds: Long) : SessionEvent

    /** The session ended normally. */
    data object Ended : SessionEvent

    /** The session failed. */
    data class Failed(val error: String) : SessionEvent

    companion object {
        /** Returns the slowest frame a `stats` line's last five values give, or `null` for dashes. */
        private fun slowest(values: List<String>): SlowestFrame? {
            val millis = values.map { it.toFloatOrNull() ?: return null }
            if (millis.size < SLOWEST_VALUES) {
                return null
            }
            return SlowestFrame(millis[0], millis[1], millis[2], millis[3], millis[4])
        }

        /** The values of a `stats` line's slowest frame. */
        private const val SLOWEST_VALUES = 5

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
                "stats" -> Stats(values[0].toInt(), values[1].toFloatOrNull(), values[2].toFloat(), slowest(values.drop(3)))
                "pair" -> PairingCode(values[0] == "retry")
                "paired" -> Paired(values[0])
                "signin" -> GatewaySignIn(values[0] == "retry", rest.substringAfter(' ', ""))
                "totp" -> GatewayCode(
                    values[0] == "retry",
                    values.getOrNull(1)?.takeIf { it != "-" },
                    values.drop(2).joinToString(" "),
                )
                "gatewaysession" -> GatewaySession(values[0], values.getOrNull(1)?.toLongOrNull() ?: 0)
                "ended" -> Ended
                "failed" -> Failed(rest)
                else -> null
            }
        }
    }
}
