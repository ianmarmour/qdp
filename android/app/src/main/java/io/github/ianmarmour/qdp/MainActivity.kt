package io.github.ianmarmour.qdp

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.ianmarmour.qdp.ui.home.HomeScreen
import io.github.ianmarmour.qdp.ui.home.HomeViewModel
import io.github.ianmarmour.qdp.ui.theme.QdpTheme

/**
 * The hosts screen: the computers on the Wi-Fi and those paired with, a
 * computer to add by its address, and the way to settings.
 *
 * Tapping a paired computer streams from it; tapping a new one pairs with
 * it, the stream asking for the code it shows. A connection link fills in
 * the Add by address dialog's Advanced section.
 */
class MainActivity : ComponentActivity() {
    private val viewModel: HomeViewModel by viewModels()

    /** Where the colors come from, read again when coming back from settings. */
    private var colorSource by mutableStateOf(ColorSource.WALLPAPER)

    /** Asks for the microphone before a stream that sends it; without it the stream goes on silently. */
    private val microphonePermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        val target = viewModel.pendingStream ?: return@registerForActivityResult
        viewModel.pendingStream = null
        stream(target)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        Native.init(applicationContext)
        colorSource = Settings.load(this).colorSource
        takeLink(intent)
        setContent {
            QdpTheme(colorSource) {
                HomeScreen(
                    viewModel = viewModel,
                    onStream = ::launch,
                    onPairByAddress = ::pairByAddress,
                    onConnectWithToken = ::connectWithToken,
                    onOpenSettings = ::openSettings,
                    onOpenWifiSettings = ::openWifiSettings,
                )
            }
        }
    }

    override fun onResume() {
        super.onResume()
        colorSource = Settings.load(this).colorSource
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        takeLink(intent)
    }

    /** Keeps what a connection link in [intent] gives, and shows it in the Add by address dialog. */
    private fun takeLink(intent: Intent?) {
        val link = intent?.data ?: return
        val settings = ConnectionLink.apply(link, Settings.load(this)) ?: return
        settings.save(this)
        // The link is used once, not again when the screen is recreated.
        intent.data = null
        viewModel.addHost(advanced = true)
    }

    /** Pairs with the computer at [address], keeping the address for next time. */
    private fun pairByAddress(address: String) {
        Settings.load(this).copy(host = address).save(this)
        launch(StreamTarget(address, address, Native.AUTH_PAIR))
    }

    /** Connects to the host at [address] with its key [pin] and [token], keeping them for next time. */
    private fun connectWithToken(address: String, pin: String, token: String) {
        Settings.load(this).copy(host = address, pin = pin, token = token).save(this)
        launch(StreamTarget(address, address, Native.AUTH_TOKEN, pin, token))
    }

    /** Streams [target], asking for the microphone first if it is wanted and not yet allowed. */
    private fun launch(target: StreamTarget) {
        val granted = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        if (Settings.load(this).microphone && !granted) {
            viewModel.pendingStream = target
            microphonePermission.launch(Manifest.permission.RECORD_AUDIO)
        } else {
            stream(target)
        }
    }

    /** Starts streaming [target] with the kept settings. */
    private fun stream(target: StreamTarget) {
        startActivity(target.into(Intent(this, StreamActivity::class.java)))
    }

    /** Opens settings. */
    private fun openSettings() {
        startActivity(Intent(this, SettingsActivity::class.java))
    }

    /** Opens the system's Wi-Fi panel, or its Wi-Fi settings where the panel is missing. */
    private fun openWifiSettings() {
        try {
            startActivity(Intent(android.provider.Settings.Panel.ACTION_WIFI))
        } catch (_: ActivityNotFoundException) {
            startActivity(Intent(android.provider.Settings.ACTION_WIFI_SETTINGS))
        }
    }
}
