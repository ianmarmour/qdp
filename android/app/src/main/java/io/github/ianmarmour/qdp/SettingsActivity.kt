package io.github.ianmarmour.qdp

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.ianmarmour.qdp.ui.settings.DeviceAbout
import io.github.ianmarmour.qdp.ui.settings.MicrophoneAccess
import io.github.ianmarmour.qdp.ui.settings.SettingsScreen
import io.github.ianmarmour.qdp.ui.theme.QdpTheme

/**
 * Settings, as an activity of its own, so that leaving it gets the system's
 * predictive back animation between activities. Every change is kept at
 * once.
 */
class SettingsActivity : ComponentActivity() {
    private var settings by mutableStateOf(Settings())
    private var microphoneAccess by mutableStateOf(MicrophoneAccess.NOT_REFUSED)

    /** Turns the microphone on once allowed; refused, it stays off and says how to allow it. */
    private val microphonePermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        microphoneAccess = if (granted) MicrophoneAccess.NOT_REFUSED else MicrophoneAccess.REFUSED
        update(settings.copy(microphone = granted))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        Native.init(applicationContext)
        settings = Settings.load(this)
        val about = DeviceAbout(
            name = DeviceName.of(this),
            pin = Native.clientPin(),
            version = packageManager.getPackageInfo(packageName, 0).versionName.orEmpty(),
        )
        setContent {
            QdpTheme(settings.colorSource) {
                SettingsScreen(
                    settings = settings,
                    microphoneAccess = microphoneAccess,
                    about = about,
                    onChange = ::update,
                    onMicrophoneChange = ::setMicrophone,
                    onBack = { onBackPressedDispatcher.onBackPressed() },
                )
            }
        }
    }

    /** Shows and keeps [changed]. */
    private fun update(changed: Settings) {
        settings = changed
        changed.save(this)
    }

    /** Turns the microphone [on] or off, asking for the permission before turning it on. */
    private fun setMicrophone(on: Boolean) {
        val granted = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        if (on && !granted) {
            microphonePermission.launch(Manifest.permission.RECORD_AUDIO)
        } else {
            update(settings.copy(microphone = on))
        }
    }
}
