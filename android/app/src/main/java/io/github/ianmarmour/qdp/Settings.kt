package io.github.ianmarmour.qdp

import android.content.Context
import androidx.annotation.StringRes
import androidx.core.content.edit

/**
 * The display the app asks the host for: the screen's own size, or a height
 * in pixels at the screen's aspect ratio.
 */
enum class Resolution(@param:StringRes val label: Int, val height: Int) {
    SCREEN(R.string.resolution_screen, 0),
    P720(R.string.resolution_720p, 720),
    P1080(R.string.resolution_1080p, 1080),
    P1440(R.string.resolution_1440p, 1440),
    P2160(R.string.resolution_4k, 2160),
}

/** Where video is decoded, with its name and a line describing it. */
enum class Decoder(@param:StringRes val label: Int, @param:StringRes val description: Int, val code: Int) {
    AUTO(R.string.decoder_auto, R.string.decoder_auto_supporting, Native.DECODER_AUTO),
    HARDWARE(R.string.decoder_hardware, R.string.decoder_hardware_supporting, Native.DECODER_HARDWARE),
    SOFTWARE(R.string.decoder_software, R.string.decoder_software_supporting, Native.DECODER_SOFTWARE),
}

/** What touching the screen does, with its name and a line describing it. */
enum class TouchMode(@param:StringRes val label: Int, @param:StringRes val description: Int) {
    /** Fingers move the host's pointer as on a laptop's trackpad. */
    TRACKPAD(R.string.touch_trackpad, R.string.touch_trackpad_supporting),

    /** Fingers touch the host's screen where they land. */
    DIRECT(R.string.touch_direct, R.string.touch_direct_supporting),
}

/** Where the app's colors come from. */
enum class ColorSource {
    /** The wallpaper's colors, on Android 12 and later; Ember before. */
    WALLPAPER,

    /** QDP's own Ember palette. */
    EMBER,
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
    val colorSource: ColorSource = ColorSource.WALLPAPER,
) {
    /** Keeps the settings for the next launch. */
    fun save(context: Context) {
        preferences(context).edit {
            putString(HOST, host)
            putString(PIN, pin)
            putString(TOKEN, token)
            putString(RESOLUTION, resolution.name)
            putString(DECODER, decoder.name)
            putString(TOUCH_MODE, touchMode.name)
            putBoolean(SHOW_STATS, showStats)
            putBoolean(MICROPHONE, microphone)
            putString(COLOR_SOURCE, colorSource.name)
        }
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
        private const val COLOR_SOURCE = "color_source"

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
                colorSource = enumOr(stored.getString(COLOR_SOURCE, null), defaults.colorSource),
            )
        }

        private inline fun <reified T : Enum<T>> enumOr(name: String?, default: T): T =
            enumValues<T>().firstOrNull { it.name == name } ?: default
    }
}
