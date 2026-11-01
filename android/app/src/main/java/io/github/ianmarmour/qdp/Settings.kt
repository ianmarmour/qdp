package io.github.ianmarmour.qdp

import android.content.Context

/**
 * The display the app asks the host for: the screen's own size, or a height
 * in pixels at the screen's aspect ratio.
 */
enum class Resolution(val label: String, val height: Int) {
    SCREEN("Screen", 0),
    P720("720p", 720),
    P1080("1080p", 1080),
    P1440("1440p", 1440),
    P2160("2160p", 2160),
}

/** Where video is decoded. */
enum class Decoder(val label: String, val code: Int) {
    AUTO("Hardware when available", Native.DECODER_AUTO),
    HARDWARE("Hardware only", Native.DECODER_HARDWARE),
    SOFTWARE("Software", Native.DECODER_SOFTWARE),
}

/** What touching the screen does. */
enum class TouchMode(val label: String) {
    /** Fingers move the host's pointer as on a laptop's trackpad. */
    TRACKPAD("Trackpad"),

    /** Fingers touch the host's screen where they land. */
    DIRECT("Touchscreen"),
}

/** What the app connects with, kept between launches. */
data class Settings(
    val host: String = "",
    val pin: String = "",
    val token: String = "",
    val resolution: Resolution = Resolution.SCREEN,
    val decoder: Decoder = Decoder.AUTO,
    val touchMode: TouchMode = TouchMode.TRACKPAD,
    val showStats: Boolean = false,
    val microphone: Boolean = false,
) {
    /** Keeps the settings for the next launch. */
    fun save(context: Context) {
        preferences(context).edit()
            .putString(HOST, host)
            .putString(PIN, pin)
            .putString(TOKEN, token)
            .putString(RESOLUTION, resolution.name)
            .putString(DECODER, decoder.name)
            .putString(TOUCH_MODE, touchMode.name)
            .putBoolean(SHOW_STATS, showStats)
            .putBoolean(MICROPHONE, microphone)
            .apply()
    }

    companion object {
        private const val PREFERENCES = "connection"
        private const val HOST = "host"
        private const val PIN = "pin"
        private const val TOKEN = "token"
        private const val RESOLUTION = "resolution"
        private const val DECODER = "decoder"
        private const val TOUCH_MODE = "touch_mode"
        private const val SHOW_STATS = "show_stats"
        private const val MICROPHONE = "microphone"

        private fun preferences(context: Context) =
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

        /** Returns the settings kept from the last launch. */
        fun load(context: Context): Settings {
            val stored = preferences(context)
            val defaults = Settings()
            return Settings(
                host = stored.getString(HOST, null) ?: defaults.host,
                pin = stored.getString(PIN, null) ?: defaults.pin,
                token = stored.getString(TOKEN, null) ?: defaults.token,
                resolution = enumOr(stored.getString(RESOLUTION, null), defaults.resolution),
                decoder = enumOr(stored.getString(DECODER, null), defaults.decoder),
                touchMode = enumOr(stored.getString(TOUCH_MODE, null), defaults.touchMode),
                showStats = stored.getBoolean(SHOW_STATS, defaults.showStats),
                microphone = stored.getBoolean(MICROPHONE, defaults.microphone),
            )
        }

        private inline fun <reified T : Enum<T>> enumOr(name: String?, default: T): T =
            enumValues<T>().firstOrNull { it.name == name } ?: default
    }
}
