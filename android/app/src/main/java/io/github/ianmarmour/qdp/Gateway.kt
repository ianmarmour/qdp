package io.github.ianmarmour.qdp

import android.content.Context
import androidx.core.content.edit
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * The organization's gateway the app reaches computers through (Section 13
 * of the specification), as the user set it in the settings: its
 * [address], its key [pin] if its administrator gave one, and the [user]'s
 * name in the organization's directory, if set. Empty fields are unset.
 */
data class Gateway(val address: String, val pin: String = "", val user: String = "") {
    companion object {
        private const val PREFERENCES = "gateway"
        private const val ADDRESS = "address"
        private const val PIN = "pin"
        private const val USER = "user"
        private const val TOKEN = "token"
        private const val TOKEN_EXPIRES = "token_expires"
        private const val HOSTS = "hosts"
        private const val NAME = "name"
        private const val TARGET = "target"
        private const val MILLIS_PER_SECOND = 1000L

        private fun preferences(context: Context) =
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

        /** Returns the gateway the user set, or `null` if none. */
        fun load(context: Context): Gateway? {
            val preferences = preferences(context)
            val address = preferences.getString(ADDRESS, null).orEmpty()
            if (address.isBlank()) {
                return null
            }
            return Gateway(
                address,
                preferences.getString(PIN, null).orEmpty(),
                preferences.getString(USER, null).orEmpty(),
            )
        }

        /**
         * Keeps [gateway], or forgets the gateway if it is `null` or its
         * address empty, forgetting the session of one whose address
         * changed.
         */
        fun save(context: Context, gateway: Gateway?) {
            val old = load(context)
            preferences(context).edit {
                putString(ADDRESS, gateway?.address?.trim().orEmpty())
                putString(PIN, gateway?.pin?.trim().orEmpty())
                putString(USER, gateway?.user?.trim().orEmpty())
                if (old?.address != gateway?.address?.trim()) {
                    remove(TOKEN)
                    remove(TOKEN_EXPIRES)
                }
            }
        }

        /**
         * Returns the session token of the last sign-in, in hexadecimal, or
         * an empty string if there is none or it expired.
         */
        fun sessionToken(context: Context): String {
            val preferences = preferences(context)
            val expires = preferences.getLong(TOKEN_EXPIRES, 0)
            return if (System.currentTimeMillis() < expires) preferences.getString(TOKEN, null).orEmpty() else ""
        }

        /**
         * Keeps the session [token] a `gatewaysession` event gave, valid for
         * [lifetimeSeconds], in the app's private storage, as the app keeps
         * hosts' tokens.
         */
        fun keepSession(context: Context, token: String, lifetimeSeconds: Long) {
            preferences(context).edit {
                putString(TOKEN, token)
                putLong(TOKEN_EXPIRES, System.currentTimeMillis() + lifetimeSeconds * MILLIS_PER_SECOND)
            }
        }

        /** Returns the computers reached through the gateway, most recently added first. */
        fun hosts(context: Context): List<GatewayHost> {
            val text = preferences(context).getString(HOSTS, null) ?: return emptyList()
            return try {
                val array = JSONArray(text)
                (0 until array.length()).map { index ->
                    val host = array.getJSONObject(index)
                    GatewayHost(host.getString(NAME), host.getString(TARGET))
                }
            } catch (_: JSONException) {
                emptyList()
            }
        }

        /** Keeps [host], replacing the one with its target. */
        fun keepHost(context: Context, host: GatewayHost) {
            saveHosts(context, listOf(host) + hosts(context).filter { it.target != host.target })
        }

        /** Forgets the computer whose target is [target]. */
        fun forgetHost(context: Context, target: String) {
            saveHosts(context, hosts(context).filter { it.target != target })
        }

        private fun saveHosts(context: Context, hosts: List<GatewayHost>) {
            val array = JSONArray()
            for (host in hosts) {
                array.put(JSONObject().put(NAME, host.name).put(TARGET, host.target))
            }
            preferences(context).edit { putString(HOSTS, array.toString()) }
        }
    }
}

/**
 * A computer reached through the gateway: its [name] for users, and the
 * [target] the gateway knows it by, its ID there or its name inside the
 * network.
 */
data class GatewayHost(val name: String, val target: String) {
    /** Returns the stream that reaches it. */
    fun stream(): StreamTarget = StreamTarget(name, target, Native.AUTH_GATEWAY)
}
