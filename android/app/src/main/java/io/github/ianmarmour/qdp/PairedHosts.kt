package io.github.ianmarmour.qdp

import android.content.Context
import androidx.core.content.edit
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/** A host the device paired with: its name, the address it was last reached at, and its key pin. */
data class PairedHost(val name: String, val address: String, val pin: String)

/**
 * The hosts the device paired with (Section 4.4.9 of the specification),
 * kept in the app's private storage: the device connects to them with its
 * own key, pinning theirs.
 */
object PairedHosts {
    private const val PREFERENCES = "paired_hosts"
    private const val HOSTS = "hosts"
    private const val NAME = "name"
    private const val ADDRESS = "address"
    private const val PIN = "pin"

    private fun preferences(context: Context) =
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    /** Returns the paired hosts, most recently kept first. */
    fun load(context: Context): List<PairedHost> {
        val text = preferences(context).getString(HOSTS, null) ?: return emptyList()
        return try {
            val array = JSONArray(text)
            (0 until array.length()).map { index ->
                val host = array.getJSONObject(index)
                PairedHost(host.getString(NAME), host.getString(ADDRESS), host.getString(PIN))
            }
        } catch (_: JSONException) {
            emptyList()
        }
    }

    /** Keeps [host], replacing the host with its pin. */
    fun keep(context: Context, host: PairedHost) {
        save(context, listOf(host) + load(context).filter { it.pin != host.pin })
    }

    /** Forgets the host whose pin is [pin]. */
    fun forget(context: Context, pin: String) {
        save(context, load(context).filter { it.pin != pin })
    }

    private fun save(context: Context, hosts: List<PairedHost>) {
        val array = JSONArray()
        for (host in hosts) {
            array.put(JSONObject().put(NAME, host.name).put(ADDRESS, host.address).put(PIN, host.pin))
        }
        preferences(context).edit { putString(HOSTS, array.toString()) }
    }
}
