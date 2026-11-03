package io.github.ianmarmour.qdp.ui.home

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.ianmarmour.qdp.DiscoveredHost
import io.github.ianmarmour.qdp.Discovery
import io.github.ianmarmour.qdp.HostCard
import io.github.ianmarmour.qdp.HostList
import io.github.ianmarmour.qdp.LocalNetwork
import io.github.ianmarmour.qdp.Native
import io.github.ianmarmour.qdp.NetworkReach
import io.github.ianmarmour.qdp.Gateway
import io.github.ianmarmour.qdp.GatewayHost
import io.github.ianmarmour.qdp.PairedHosts
import io.github.ianmarmour.qdp.StreamTarget
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** How the search for computers on the local network goes. */
enum class SearchPhase {
    /** Looking, with nothing found yet. */
    SEARCHING,

    /** Looking for a while without finding anything, so the screen suggests what to check. */
    QUIET,

    /** At least one computer was found. */
    FOUND,

    /** Not on Wi-Fi or Ethernet, so there is nowhere to look. */
    NO_WIFI,
}

/** What the hosts screen shows: the computers, and how the search for more goes. */
data class HomeState(val cards: List<HostCard>, val search: SearchPhase)

/** A dialog the hosts screen shows over its list. */
sealed interface HomeDialog {
    /**
     * Adding a computer by its address, with the Advanced section open if
     * [advanced]; [request] tells each opening apart, so that a connection
     * link opened while the dialog shows fills it in afresh.
     */
    data class AddHost(val advanced: Boolean, val request: Int) : HomeDialog

    /** Asking whether to forget [card]'s computer. */
    data class Forget(val card: HostCard) : HomeDialog

    /** [card]'s address, status and key pin. */
    data class Details(val card: HostCard) : HomeDialog

    /** How to turn pairing on for [card]'s computer. */
    data class PairingOff(val card: HostCard) : HomeDialog

    /** Why [card]'s computer, named like a paired one, is not connected to automatically. */
    data class KeyMismatch(val card: HostCard) : HomeDialog

    /** How to set up a computer for QDP. */
    data object SetupHelp : HomeDialog
}

/**
 * The hosts screen's state, which outlives configuration changes: the
 * computers found on the network and those paired with, whether the device
 * is on a network to look on, how long it has looked, and the dialog open.
 */
class HomeViewModel(application: Application) : AndroidViewModel(application) {
    private val discovered = MutableStateFlow<List<DiscoveredHost>>(emptyList())
    private val paired = MutableStateFlow(PairedHosts.load(application))
    private val throughGateway = MutableStateFlow(gatewayHosts(application))
    private val reach = MutableStateFlow(NetworkReach.LOCAL)
    private val searchedLong = MutableStateFlow(false)

    /** The pins of the paired computers not found on the network that answered at their addresses when last checked. */
    private val answering = MutableStateFlow<Set<String>>(emptySet())
    private val discovery = Discovery(application) { hosts -> discovered.value = hosts }
    private val network = LocalNetwork(application) { value -> reach.value = value }
    private val openDialog = MutableStateFlow<HomeDialog?>(null)
    private var quietTimer: Job? = null

    /** Checks the paired computers not found at their addresses, while the screen shows. */
    private var checking: Job? = null

    /** How many times the Add by address dialog was opened. */
    private var addRequests = 0

    /** The stream waiting for the user to answer the microphone permission. */
    var pendingStream: StreamTarget? = null

    /** The dialog open, if any. */
    val dialog: StateFlow<HomeDialog?> = openDialog.asStateFlow()

    /** The computers and the search's progress. */
    val state: StateFlow<HomeState> =
        combine(discovered, combine(paired, answering, ::Pair), throughGateway, reach, searchedLong) {
                found, (pairedHosts, answered), gatewayHosts, network, long ->
            HomeState(HostList.cards(found, pairedHosts, gatewayHosts, answered), phase(found, network, long))
        }.stateIn(
            viewModelScope,
            SharingStarted.Eagerly,
            HomeState(HostList.cards(emptyList(), paired.value, throughGateway.value), SearchPhase.SEARCHING),
        )

    /** Starts looking for computers, as the screen comes into view; a stream may have paired with one meanwhile. */
    fun startSearching() {
        paired.value = PairedHosts.load(getApplication())
        // The gateway may have been set or cleared in the settings.
        throughGateway.value = gatewayHosts(getApplication())
        network.start()
        discovery.start()
        searchedLong.value = false
        quietTimer?.cancel()
        quietTimer = viewModelScope.launch {
            delay(QUIET_AFTER_MILLIS)
            searchedLong.value = true
        }
        checking?.cancel()
        checking = viewModelScope.launch {
            while (isActive) {
                answering.value = answeringPins()
                delay(CHECK_INTERVAL_MILLIS)
            }
        }
    }

    /** Stops looking, as the screen goes out of view. */
    fun stopSearching() {
        quietTimer?.cancel()
        checking?.cancel()
        discovery.stop()
        network.stop()
    }

    /** Opens the Add by address dialog, with its Advanced section open if [advanced]. */
    fun addHost(advanced: Boolean) {
        addRequests++
        openDialog.value = HomeDialog.AddHost(advanced, addRequests)
    }

    /** Shows [dialog], or closes the one open for `null`. */
    fun show(dialog: HomeDialog?) {
        openDialog.value = dialog
    }

    /**
     * Keeps the computer the gateway knows as [target], to reach through it,
     * and returns the stream that reaches it.
     */
    fun addGatewayHost(target: String): StreamTarget {
        val host = GatewayHost(target, target)
        Gateway.keepHost(getApplication(), host)
        throughGateway.value = gatewayHosts(getApplication())
        return host.stream()
    }

    /** Forgets [card]'s paired computer, or the one it reaches through the gateway. */
    fun forget(card: HostCard) {
        card.gateway?.let { host ->
            Gateway.forgetHost(getApplication(), host.target)
            throughGateway.value = gatewayHosts(getApplication())
            return
        }
        val host = card.paired ?: return
        PairedHosts.forget(getApplication(), host.pin)
        paired.value = PairedHosts.load(getApplication())
    }

    /** Remembers the name and address a paired computer was found with, so that it is reached there from now on. */
    fun rememberAddress(card: HostCard) {
        val host = card.paired ?: return
        if (host.address != card.address || host.name != card.name) {
            PairedHosts.keep(getApplication(), host.copy(name = card.name, address = card.address))
        }
    }

    override fun onCleared() {
        stopSearching()
    }

    /**
     * Checks each paired computer the search has not found at its address,
     * all at once off the main thread, as on mobile data, where there is no
     * local network to find it on, and returns the pins of those that answer.
     */
    private suspend fun answeringPins(): Set<String> = withContext(Dispatchers.IO) {
        val found = discovered.value.mapNotNull { it.pin }.toSet()
        paired.value
            .filter { it.pin !in found }
            .map { host -> async { host.pin.takeIf { Native.hostReachable(host.address) } } }
            .awaitAll()
            .filterNotNull()
            .toSet()
    }

    private companion object {
        /** Returns the computers reached through the gateway, while one is set. */
        fun gatewayHosts(application: Application): List<GatewayHost> =
            if (Gateway.load(application) != null) Gateway.hosts(application) else emptyList()

        /** How long the search runs without finding anything before the screen suggests what to check. */
        const val QUIET_AFTER_MILLIS = 15_000L

        /** How long after one check of the paired computers' addresses the next starts. */
        const val CHECK_INTERVAL_MILLIS = 8_000L

        /** Returns how the search goes, having [found] computers on a network of [reach], searching [long] or not. */
        fun phase(found: List<DiscoveredHost>, reach: NetworkReach, long: Boolean): SearchPhase = when {
            reach == NetworkReach.NONE -> SearchPhase.NO_WIFI
            found.isNotEmpty() -> SearchPhase.FOUND
            long -> SearchPhase.QUIET
            else -> SearchPhase.SEARCHING
        }
    }
}
