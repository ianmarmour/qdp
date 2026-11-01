package io.github.ianmarmour.qdp

import android.content.Context
import androidx.core.content.edit

/** One-time tips, each shown until the user has seen it once. */
enum class Tip(val key: String) {
    /** How to open the stream's menu: Back, or Select, Start and both bumpers. */
    STREAM_MENU("stream_menu");

    /** Returns whether the tip is still to be shown. */
    fun pending(context: Context): Boolean = !preferences(context).getBoolean(key, false)

    /** Remembers that the tip was shown. */
    fun shown(context: Context) {
        preferences(context).edit { putBoolean(key, true) }
    }

    private companion object {
        /** The preferences file the tips are kept in. */
        const val PREFERENCES = "tips"

        /** Returns the tips' preferences. */
        fun preferences(context: Context) = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
    }
}
