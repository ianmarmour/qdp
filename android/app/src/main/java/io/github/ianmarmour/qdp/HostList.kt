package io.github.ianmarmour.qdp

/**
 * How a computer on the hosts screen stands. The list shows them in this
 * order: ready first, then new ones, then paired ones not found.
 */
enum class HostStatus {
    /** Paired, and found on the network or answering at its address. */
    READY,

    /** Reached through the organization's gateway. */
    GATEWAY,

    /** Found, not paired, and offering pairing. */
    NEW,

    /**
     * Found under a paired computer's name with another key, so it may pose
     * as that computer (Section 3.13).
     */
    KEY_MISMATCH,

    /** Found, not paired, and not offering pairing. */
    PAIRING_OFF,

    /** Paired, neither found on the network nor answering at its address. */
    NOT_SEEN,
    ;

    /** Whether the computer can be used now, as one paired and found or one reached through the gateway. */
    val ready: Boolean get() = this == READY || this == GATEWAY
}

/**
 * A computer on the hosts screen: its stable [key], what it shows, how it
 * stands, the stream that tapping it starts (`null` if it starts none), the
 * paired computer it is, the key pin it advertises, and the computer
 * reached through the gateway it is.
 */
data class HostCard(
    val key: String,
    val name: String,
    val address: String,
    val status: HostStatus,
    val target: StreamTarget?,
    val paired: PairedHost?,
    val advertisedPin: String?,
    val gateway: GatewayHost? = null,
)

/** Builds the hosts screen's list from the computers found and those paired with. */
object HostList {
    /** The key prefix of a paired computer's card, which keeps it whether found or not. */
    private const val PAIRED_KEY = "paired:"

    /** The key prefix of a found computer that is not paired. */
    private const val FOUND_KEY = "found:"

    /** The key prefix of a computer reached through the gateway. */
    private const val GATEWAY_KEY = "gateway:"

    /**
     * Returns the cards for the computers [discovered] on the network,
     * recognized as paired by their pin, the [paired] computers not found,
     * ready if their pins are among those [answering] at their addresses,
     * and those reached [throughGateway], in [HostStatus] order and by name.
     */
    fun cards(
        discovered: List<DiscoveredHost>,
        paired: List<PairedHost>,
        throughGateway: List<GatewayHost> = emptyList(),
        answering: Set<String> = emptySet(),
    ): List<HostCard> {
        val found = mutableSetOf<String>()
        val discoveredCards = discovered.map { host ->
            val match = paired.firstOrNull { it.pin == host.pin }
            // A paired computer's name with another key, listed in that
            // computer's place: its key changed, or another poses as it.
            val namesake = if (match == null) paired.firstOrNull { it.name == host.name } else null
            when {
                namesake != null -> {
                    found += namesake.pin
                    HostCard(
                        key = PAIRED_KEY + namesake.pin,
                        name = host.name,
                        address = host.address,
                        status = HostStatus.KEY_MISMATCH,
                        target = if (host.pairing) StreamTarget(host.name, host.address, Native.AUTH_PAIR) else null,
                        paired = namesake,
                        advertisedPin = host.pin,
                    )
                }
                match != null -> {
                    found += match.pin
                    HostCard(
                        key = PAIRED_KEY + match.pin,
                        name = host.name,
                        address = host.address,
                        status = HostStatus.READY,
                        target = StreamTarget(host.name, host.address, Native.AUTH_PAIRED, match.pin),
                        paired = match,
                        advertisedPin = host.pin,
                    )
                }
                else -> HostCard(
                    key = FOUND_KEY + host.name,
                    name = host.name,
                    address = host.address,
                    status = if (host.pairing) HostStatus.NEW else HostStatus.PAIRING_OFF,
                    target = if (host.pairing) StreamTarget(host.name, host.address, Native.AUTH_PAIR) else null,
                    paired = null,
                    advertisedPin = host.pin,
                )
            }
        }
        val absent = paired.filter { it.pin !in found }.map { host ->
            HostCard(
                key = PAIRED_KEY + host.pin,
                name = host.name,
                address = host.address,
                status = if (host.pin in answering) HostStatus.READY else HostStatus.NOT_SEEN,
                target = StreamTarget(host.name, host.address, Native.AUTH_PAIRED, host.pin),
                paired = host,
                advertisedPin = null,
            )
        }
        // Two advertisements of one paired computer would share a key, which
        // a lazy grid does not allow.
        val gatewayCards = throughGateway.map { host ->
            HostCard(
                key = GATEWAY_KEY + host.target,
                name = host.name,
                address = host.target,
                status = HostStatus.GATEWAY,
                target = host.stream(),
                paired = null,
                advertisedPin = null,
                gateway = host,
            )
        }
        return (discoveredCards + absent + gatewayCards)
            .sortedWith(compareBy<HostCard> { it.status.ordinal }.thenBy { it.name.lowercase() })
            .distinctBy { it.key }
    }
}
