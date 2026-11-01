package io.github.ianmarmour.qdp

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities

/** Whether the device is on a local network that computers can be found on. */
enum class NetworkReach {
    /** On Wi-Fi or Ethernet. */
    LOCAL,

    /** On mobile data only, or on no network. */
    NONE,
}

/**
 * Follows whether the device is on Wi-Fi or Ethernet, calling [changed],
 * from a system thread, whenever that changes between [start] and [stop].
 */
class LocalNetwork(context: Context, private val changed: (NetworkReach) -> Unit) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private var callback: ConnectivityManager.NetworkCallback? = null

    /** Starts following the default network, reporting where it stands at once. */
    fun start() {
        val manager = connectivity ?: return
        if (callback != null) {
            return
        }
        changed(reach(manager.getNetworkCapabilities(manager.activeNetwork)))
        val follow = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                changed(reach(capabilities))
            }

            override fun onLost(network: Network) {
                changed(NetworkReach.NONE)
            }
        }
        callback = follow
        manager.registerDefaultNetworkCallback(follow)
    }

    /** Stops following the network. */
    fun stop() {
        val follow = callback ?: return
        callback = null
        connectivity?.unregisterNetworkCallback(follow)
    }

    private companion object {
        /** Returns where a network with [capabilities] stands, or none for `null`. */
        fun reach(capabilities: NetworkCapabilities?): NetworkReach = when {
            capabilities == null -> NetworkReach.NONE
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> NetworkReach.LOCAL
            else -> NetworkReach.NONE
        }
    }
}
