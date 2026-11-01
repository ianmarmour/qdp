package io.github.ianmarmour.qdp

import androidx.annotation.StringRes

/**
 * Why a stream failed, as the app explains it: a title naming the problem
 * and the computer, and one sentence of cause and fix.
 *
 * The core reports failures as text, so [of] recognizes them by their
 * wording; stable error kinds from the core would make this exact.
 */
enum class ErrorKind(@param:StringRes val title: Int, @param:StringRes val body: Int) {
    /** The address names no computer. */
    NOT_FOUND(R.string.error_not_found_title, R.string.error_not_found_body),

    /** Nothing answered at the address. */
    UNREACHABLE(R.string.error_unreachable_title, R.string.error_unreachable_body),

    /** The computer's key is not the one paired with. */
    KEY_CHANGED(R.string.error_key_changed_title, R.string.error_key_changed_body),

    /** The computer is not pairing: another device is, or codes were wrong too often. */
    PAIRING_BUSY(R.string.error_pairing_busy_title, R.string.error_pairing_busy_body),

    /** The computer refused this device's key or token. */
    NOT_ACCEPTED(R.string.error_not_accepted_title, R.string.error_not_accepted_body),

    /** The device could not decode the video. */
    DECODER(R.string.error_decoder_title, R.string.error_decoder_body),

    /** Anything else. */
    OTHER(R.string.error_other_title, R.string.error_other_body);

    companion object {
        /** Words in the core's messages, in the order they are looked for, and the kind each means. */
        private val PHRASES = listOf(
            "invalid peer certificate" to KEY_CHANGED,
            "cannot resolve" to NOT_FOUND,
            "has no address" to NOT_FOUND,
            "not pairing now" to PAIRING_BUSY,
            "another client is pairing" to PAIRING_BUSY,
            "pairing failed too often" to PAIRING_BUSY,
            "authentication failed" to NOT_ACCEPTED,
            "no authentication method" to NOT_ACCEPTED,
            "decod" to DECODER,
            "cannot connect to" to UNREACHABLE,
            "timed out" to UNREACHABLE,
        )

        /** Returns the kind of failure the core's [message] describes. */
        fun of(message: String): ErrorKind {
            val lower = message.lowercase()
            return PHRASES.firstOrNull { (phrase, _) -> phrase in lower }?.second ?: OTHER
        }
    }
}

/** A failed stream: what kind of failure, and the core's own words, shown under "Show details". */
data class StreamError(val kind: ErrorKind, val details: String) {
    companion object {
        /** Returns the failure the core's [message] describes. */
        fun of(message: String): StreamError = StreamError(ErrorKind.of(message), message)
    }
}
