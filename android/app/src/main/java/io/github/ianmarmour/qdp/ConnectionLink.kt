package io.github.ianmarmour.qdp

import android.net.Uri

/**
 * A link that fills in the connection screen, so that a host's details need
 * not be typed on the device:
 *
 * `qdp://HOST[:PORT]?pin=PIN&token=TOKEN[&resolution=RESOLUTION]`
 *
 * RESOLUTION is `screen`, a height such as `1080p`, or a size such as
 * `1920x1080`, whose height is used at the screen's aspect ratio. A computer
 * opens it on a device connected with USB debugging through
 * `adb shell am start`.
 */
object ConnectionLink {
    /** The links' scheme. */
    const val SCHEME = "qdp"

    /** Returns [settings] with what [link] gives filled in, or `null` if it is not a connection link. */
    fun apply(link: Uri, settings: Settings): Settings? {
        if (link.scheme != SCHEME) {
            return null
        }
        val host = link.host?.takeIf { it.isNotEmpty() } ?: return null
        val address = if (link.port == -1) host.removeSurrounding("[", "]") else "$host:${link.port}"
        return settings.copy(
            host = address,
            pin = link.getQueryParameter("pin") ?: settings.pin,
            token = link.getQueryParameter("token") ?: settings.token,
            resolution = link.getQueryParameter("resolution")?.let(::resolution) ?: settings.resolution,
        )
    }

    /** Returns the resolution [text] names, or `null` if it names none the app offers. */
    fun resolution(text: String): Resolution? {
        val name = text.trim().lowercase()
        if (name == "screen") {
            return Resolution.SCREEN
        }
        val height = name.removeSuffix("p").substringAfter('x').toIntOrNull() ?: return null
        return Resolution.entries.firstOrNull { it.height == height }
    }
}
