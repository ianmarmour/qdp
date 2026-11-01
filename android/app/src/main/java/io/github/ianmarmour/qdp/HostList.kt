package io.github.ianmarmour.qdp

/**
 * How a computer on the hosts screen stands. The list shows them in this
 * order: ready first, then new ones, then paired ones not found.
 */
enum class HostStatus {
    /** Paired, and found on the network. */
    READY,

    /** Found, not paired, and offering pairing. */
    NEW,

    /**
     * Found under a paired computer's name with another key, so it may pose
     * as that computer (Section 3.13).
     */
    KEY_MISMATCH,

    /** Found, not paired, and not offering pairing. */
    PAIRING_OFF,

    /** Paired, and not found on the network. */
    NOT_SEEN,
}

/**
 * A computer on the hosts screen: its stable [key], what it shows, how it
 * stands, the stream that tapping it starts (`null` if it starts none), the
 * paired computer it is, and the key pin it advertises.
 */
data class HostCard(
    val key: String,
    val name: String,
    val address: String,
    val status: HostStatus,
    val target: StreamTarget?,
    val paired: PairedHost?,
    val advertisedPin: String?,
)

/** Builds the hosts screen's list from the computers found and those paired with. */
object HostList {
    /** The key prefix of a paired computer's card, which keeps it whether found or not. */
    private const val PAIRED_KEY = "paired:"

    /** The key prefix of a found computer that is not paired. */
    private const val FOUND_KEY = "found:"

    /**
     * Returns the cards for the computers [discovered] on the network,
     * recognized as paired by their pin, and the [paired] computers not
     * found, in [HostStatus] order and by name.
     */
    fun cards(discovered: List<DiscoveredHost>, paired: List<PairedHost>): List<HostCard> {
        val found = mutableSetOf<String>()
        val discoveredCards = discovered.map { host ->
            val match = paired.firstOrNull { it.pin == host.pin }
            when {
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
                    status = when {
                        paired.any { it.name == host.name } -> HostStatus.KEY_MISMATCH
                        host.pairing -> HostStatus.NEW
                        else -> HostStatus.PAIRING_OFF
                    },
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
                status = HostStatus.NOT_SEEN,
                target = StreamTarget(host.name, host.address, Native.AUTH_PAIRED, host.pin),
                paired = host,
                advertisedPin = null,
            )
        }
        // Two advertisements of one paired computer would share a key, which
        // a lazy grid does not allow.
        return (discoveredCards + absent)
            .sortedWith(compareBy<HostCard> { it.status.ordinal }.thenBy { it.name.lowercase() })
            .distinctBy { it.key }
    }
}
