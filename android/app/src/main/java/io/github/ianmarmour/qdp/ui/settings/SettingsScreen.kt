package io.github.ianmarmour.qdp.ui.settings

import android.content.ClipData
import android.content.ClipboardManager
import android.os.Build
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.adaptive.currentWindowAdaptiveInfo
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.window.core.layout.WindowSizeClass
import io.github.ianmarmour.qdp.ColorSource
import io.github.ianmarmour.qdp.Decoder
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.Resolution
import io.github.ianmarmour.qdp.Settings
import io.github.ianmarmour.qdp.StreamSize
import io.github.ianmarmour.qdp.ui.components.EDGE_ICON_BUTTON_PADDING
import io.github.ianmarmour.qdp.ui.components.QdpIconButton
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import kotlinx.coroutines.launch

/** The widest the settings run, on tablets. */
private val MAX_SETTINGS_WIDTH = 640.dp

/** Whether the user refused the microphone when last asked. */
enum class MicrophoneAccess {
    /** Allowed, or not refused yet. */
    NOT_REFUSED,

    /** Refused, so the switch stays off and says how to allow it. */
    REFUSED,
}

/** What this device tells computers about itself, shown under About this device. */
data class DeviceAbout(val name: String, val pin: String?, val version: String)

/**
 * Settings, in sections: Stream (resolution and microphone), Controls
 * (touch and the performance overlay), Appearance on Android 12 and later
 * (wallpaper colors), Advanced (the decoder) and About this device (its
 * name, pin and the app's version). Each change is answered through
 * [onChange] at once; [onMicrophoneChange] asks for the permission first.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    settings: Settings,
    microphoneAccess: MicrophoneAccess,
    about: DeviceAbout,
    onChange: (Settings) -> Unit,
    onMicrophoneChange: (Boolean) -> Unit,
    onBack: () -> Unit,
) {
    val sizeClass = currentWindowAdaptiveInfo().windowSizeClass
    val collapsing = !sizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_MEDIUM_LOWER_BOUND) &&
        sizeClass.isHeightAtLeastBreakpoint(WindowSizeClass.HEIGHT_DP_MEDIUM_LOWER_BOUND)
    val scrollBehavior = if (collapsing) {
        TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    } else {
        TopAppBarDefaults.pinnedScrollBehavior()
    }
    val snackbar = remember { SnackbarHostState() }
    val title = stringResource(R.string.action_settings)
    val back: @Composable () -> Unit = {
        QdpIconButton(
            onClick = onBack,
            icon = R.drawable.ic_arrow_back,
            description = stringResource(R.string.action_back),
            modifier = Modifier.padding(start = EDGE_ICON_BUTTON_PADDING),
        )
    }
    Scaffold(
        modifier = Modifier.nestedScroll(scrollBehavior.nestedScrollConnection).gamepadButtons(),
        topBar = {
            if (collapsing) {
                LargeTopAppBar(title = { Text(title) }, navigationIcon = back, scrollBehavior = scrollBehavior)
            } else {
                TopAppBar(
                    title = { Text(title, style = MaterialTheme.typography.headlineSmall) },
                    navigationIcon = back,
                    scrollBehavior = scrollBehavior,
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                modifier = Modifier
                    .widthIn(max = MAX_SETTINGS_WIDTH)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(padding)
                    .padding(horizontal = 8.dp, vertical = 8.dp),
            ) {
                StreamSection(settings, microphoneAccess, onChange, onMicrophoneChange)
                ControlsSection(settings, onChange)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    SectionHeader(R.string.settings_appearance)
                    SwitchRow(
                        title = stringResource(R.string.settings_wallpaper_colors),
                        supporting = stringResource(R.string.settings_wallpaper_colors_supporting),
                        checked = settings.colorSource == ColorSource.WALLPAPER,
                    ) { on -> onChange(settings.copy(colorSource = if (on) ColorSource.WALLPAPER else ColorSource.EMBER)) }
                }
                SectionHeader(R.string.settings_advanced)
                ChoiceRow(
                    title = stringResource(R.string.settings_decoder),
                    supporting = stringResource(settings.decoder.description),
                    choices = Decoder.entries,
                    selected = settings.decoder,
                    label = { stringResource(it.label) },
                    onSelect = { onChange(settings.copy(decoder = it)) },
                    description = { stringResource(it.description) },
                )
                GatewaySection()
                AboutSection(about, snackbar)
            }
        }
    }
}

/** The Stream section: resolution, microphone and the performance overlay. */
@Composable
private fun StreamSection(
    settings: Settings,
    microphoneAccess: MicrophoneAccess,
    onChange: (Settings) -> Unit,
    onMicrophoneChange: (Boolean) -> Unit,
) {
    val screen = StreamSize.screen(LocalView.current.display)
    val requested = StreamSize.requested(screen, settings.resolution)
    SectionHeader(R.string.settings_stream)
    ChoiceRow(
        title = stringResource(R.string.settings_resolution),
        supporting = if (settings.resolution == Resolution.SCREEN) {
            stringResource(R.string.settings_resolution_screen, screen.width, screen.height)
        } else {
            stringResource(R.string.settings_resolution_scaled, requested.width, requested.height)
        },
        choices = Resolution.entries,
        selected = settings.resolution,
        label = { stringResource(it.label) },
        onSelect = { onChange(settings.copy(resolution = it)) },
    )
    SwitchRow(
        title = stringResource(R.string.settings_microphone),
        supporting = stringResource(
            if (microphoneAccess == MicrophoneAccess.REFUSED) {
                R.string.settings_microphone_denied
            } else {
                R.string.settings_microphone_supporting
            },
        ),
        checked = settings.microphone,
        onCheckedChange = onMicrophoneChange,
    )
    SwitchRow(
        title = stringResource(R.string.settings_stats),
        supporting = stringResource(R.string.settings_stats_supporting),
        checked = settings.showStats,
    ) { onChange(settings.copy(showStats = it)) }
}

/** The Controls section: touch mode. */
@Composable
private fun ControlsSection(settings: Settings, onChange: (Settings) -> Unit) {
    SectionHeader(R.string.settings_controls)
    TouchModePicker(settings.touchMode) { onChange(settings.copy(touchMode = it)) }
}

/** About this device: its name, its pin with a Copy button, and the app's version. */
@Composable
private fun AboutSection(about: DeviceAbout, snackbar: SnackbarHostState) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val copied = stringResource(R.string.copied)
    val pinLabel = stringResource(R.string.settings_device_pin)
    SectionHeader(R.string.settings_about)
    InfoRow(stringResource(R.string.settings_device_name), about.name)
    ListItem(
        headlineContent = { Text(pinLabel) },
        supportingContent = {
            Column {
                if (about.pin != null) {
                    SelectionContainer { Text(about.pin, fontFamily = FontFamily.Monospace) }
                    Text(stringResource(R.string.settings_device_pin_supporting))
                } else {
                    Text(stringResource(R.string.settings_device_pin_unavailable))
                }
            }
        },
        trailingContent = about.pin?.let { pin ->
            {
                QdpIconButton(
                    onClick = {
                        context.getSystemService(ClipboardManager::class.java)
                            ?.setPrimaryClip(ClipData.newPlainText(pinLabel, pin))
                        // Android 13 and later confirm a copy themselves.
                        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
                            scope.launch { snackbar.showSnackbar(copied) }
                        }
                    },
                    icon = R.drawable.ic_content_copy,
                    description = stringResource(R.string.action_copy),
                )
            }
        },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
    )
    InfoRow(stringResource(R.string.settings_version), about.version)
}
