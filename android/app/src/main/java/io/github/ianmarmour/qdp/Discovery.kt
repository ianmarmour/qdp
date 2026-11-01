package io.github.ianmarmour.qdp

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress

/**
 * A host advertising itself on the local network: its name, its address
 * and port, the key pin it claims and whether it offers pairing. Nothing
 * here is authenticated: the pin only recognizes a host already paired
 * with, which the connection then verifies.
 */
data class DiscoveredHost(val name: String, val address: String, val pin: String?, val pairing: Boolean)

/**
 * Finds hosts on the local network with DNS-SD (Section 3.13 of the
 * specification), calling [changed] on the main thread with the hosts found
 * whenever they change, between [start] and [stop].
 */
class Discovery(context: Context, private val changed: (List<DiscoveredHost>) -> Unit) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val main = Handler(Looper.getMainLooper())

    /** The hosts found, by service name. */
    private val hosts = LinkedHashMap<String, DiscoveredHost>()

    /** The services waiting to be resolved, which NsdManager does one at a time. */
    private val unresolved = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private var listener: NsdManager.DiscoveryListener? = null

    /** Starts looking for hosts. */
    fun start() {
        if (listener != null || nsd == null) {
            return
        }
        val discovery = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}

            override fun onDiscoveryStopped(serviceType: String) {}

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "cannot look for hosts: error $errorCode")
                main.post { if (listener === this) listener = null }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}

            override fun onServiceFound(service: NsdServiceInfo) {
                main.post {
                    if (listener === this) {
                        unresolved.addLast(service)
                        resolveNext()
                    }
                }
            }

            override fun onServiceLost(service: NsdServiceInfo) {
                main.post {
                    if (listener === this && hosts.remove(service.serviceName) != null) {
                        changed(hosts.values.toList())
                    }
                }
            }
        }
        listener = discovery
        nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discovery)
    }

    /** Stops looking for hosts, forgetting those found. */
    fun stop() {
        val discovery = listener ?: return
        listener = null
        try {
            nsd?.stopServiceDiscovery(discovery)
        } catch (error: IllegalArgumentException) {
            // Discovery had not started.
        }
        unresolved.clear()
        hosts.clear()
        changed(emptyList())
    }

    /** Resolves the next service found, unless one is being resolved. */
    private fun resolveNext() {
        if (resolving || listener == null) {
            return
        }
        val service = unresolved.removeFirstOrNull() ?: return
        resolving = true
        val resolve = object : NsdManager.ResolveListener {
            override fun onResolveFailed(service: NsdServiceInfo, errorCode: Int) {
                main.post {
                    resolving = false
                    resolveNext()
                }
            }

            override fun onServiceResolved(service: NsdServiceInfo) {
                main.post {
                    resolving = false
                    val host = host(service)
                    if (listener != null && host != null) {
                        hosts[service.serviceName] = host
                        changed(hosts.values.toList())
                    }
                    resolveNext()
                }
            }
        }
        @Suppress("DEPRECATION")
        nsd?.resolveService(service, resolve)
    }

    private companion object {
        const val TAG = "qdp"

        /** The service type of QDP hosts. */
        const val SERVICE_TYPE = "_qdp._udp."

        /** The TXT key of the host's key pin. */
        const val PIN_KEY = "pin"

        /** The TXT key present while the host offers pairing. */
        const val PAIR_KEY = "pair"

        /** The value of [PAIR_KEY] while the host offers pairing. */
        const val PAIR_OFFERED = "1"

        /** Returns the host a resolved [service] describes, or `null` without an address. */
        fun host(service: NsdServiceInfo): DiscoveredHost? {
            val address = address(service) ?: return null
            // TXT keys are compared without regard to case, and the first counts.
            val attributes = service.attributes.entries
                .groupBy({ it.key.lowercase() }, { it.value })
                .mapValues { (_, values) -> values.first()?.decodeToString() }
            return DiscoveredHost(
                name = service.serviceName,
                address = address,
                pin = attributes[PIN_KEY]?.lowercase()?.takeIf { Native.isPin(it) },
                pairing = attributes[PAIR_KEY] == PAIR_OFFERED,
            )
        }

        /** Returns a resolved [service]'s address and port, an IPv4 address if it has one. */
        fun address(service: NsdServiceInfo): String? {
            val addresses: List<InetAddress> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                service.hostAddresses
            } else {
                @Suppress("DEPRECATION")
                listOfNotNull(service.host)
            }
            val address = addresses.firstOrNull { it is Inet4Address }
                ?: addresses.firstOrNull { it is Inet6Address && !it.isLinkLocalAddress }
                ?: return null
            val text = address.hostAddress ?: return null
            return if (address is Inet6Address) "[$text]:${service.port}" else "$text:${service.port}"
        }
    }
}
