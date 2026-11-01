package io.github.ianmarmour.qdp

import android.content.Intent

/**
 * The host a stream connects to and how it authenticates, as the
 * connection screen hands it to the stream.
 *
 * [authentication] is one of [Native]'s `AUTH_` values; [pin] is the host's
 * key pin, empty while pairing, and [token] is used only with
 * [Native.AUTH_TOKEN].
 */
data class StreamTarget(
    val name: String,
    val address: String,
    val authentication: Int,
    val pin: String = "",
    val token: String = "",
) {
    /** Returns [intent] carrying the target. */
    fun into(intent: Intent): Intent = intent
        .putExtra(NAME, name)
        .putExtra(ADDRESS, address)
        .putExtra(AUTHENTICATION, authentication)
        .putExtra(PIN, pin)
        .putExtra(TOKEN, token)

    companion object {
        private const val NAME = "io.github.ianmarmour.qdp.NAME"
        private const val ADDRESS = "io.github.ianmarmour.qdp.ADDRESS"
        private const val AUTHENTICATION = "io.github.ianmarmour.qdp.AUTHENTICATION"
        private const val PIN = "io.github.ianmarmour.qdp.PIN"
        private const val TOKEN = "io.github.ianmarmour.qdp.TOKEN"

        /** Returns the target [intent] carries, or `null` if it carries none. */
        fun from(intent: Intent): StreamTarget? {
            val address = intent.getStringExtra(ADDRESS) ?: return null
            return StreamTarget(
                name = intent.getStringExtra(NAME) ?: address,
                address = address,
                authentication = intent.getIntExtra(AUTHENTICATION, Native.AUTH_TOKEN),
                pin = intent.getStringExtra(PIN).orEmpty(),
                token = intent.getStringExtra(TOKEN).orEmpty(),
            )
        }
    }
}
