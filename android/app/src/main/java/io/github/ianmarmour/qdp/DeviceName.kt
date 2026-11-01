package io.github.ianmarmour.qdp

import android.content.Context
import android.os.Build

/** The device's name, which a host shows when pairing and keeps for the device. */
object DeviceName {
    /** Returns the name the user gave the device, or its model if they gave none. */
    fun of(context: Context): String =
        android.provider.Settings.Global.getString(context.contentResolver, android.provider.Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() }
            ?: Build.MODEL
}
