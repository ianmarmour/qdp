package io.github.ianmarmour.qdp.ui.stream

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.QdpButton
import io.github.ianmarmour.qdp.ui.components.rememberInitialFocus
import kotlinx.coroutines.delay
import kotlin.math.ceil

/** How often the reconnecting countdown updates. */
private const val COUNTDOWN_TICK_MILLIS = 250L

/** Milliseconds per second. */
private const val MILLIS_PER_SECOND = 1000.0

/** The widest the reconnecting banner and the tip run. */
private val BANNER_MAX_WIDTH = 560.dp

/**
 * The screen before the first frame: black, with the host's name, progress,
 * what is happening, and Cancel, which B and Back also do.
 */
@Composable
fun ConnectingScreen(hostName: String, step: ConnectingStep, onCancel: () -> Unit) {
    val cancel = rememberInitialFocus()
    Box(modifier = Modifier.fillMaxSize().background(Color.Black), contentAlignment = Alignment.Center) {
        Column(
            modifier = Modifier.windowInsetsPadding(WindowInsets.safeDrawing).padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text(hostName, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
            CircularProgressIndicator()
            Text(
                text = when (step) {
                    ConnectingStep.REACHING -> stringResource(R.string.stream_connecting, hostName)
                    ConnectingStep.STARTING -> stringResource(R.string.stream_starting)
                },
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )
            QdpButton(onClick = onCancel, emphasis = Emphasis.LOW, modifier = Modifier.focusRequester(cancel)) {
                Text(stringResource(R.string.action_cancel))
            }
        }
    }
}

/**
 * The banner across the top while the connection is made again: "Connection
 * dropped. Reconnecting in 3 s…", counting down to [reconnectAt], with
 * Disconnect.
 */
@Composable
fun ReconnectingBanner(reconnectAt: Long, onDisconnect: () -> Unit, modifier: Modifier = Modifier) {
    var now by remember { mutableLongStateOf(SystemClock.uptimeMillis()) }
    LaunchedEffect(reconnectAt) {
        now = SystemClock.uptimeMillis()
        while (now < reconnectAt) {
            delay(COUNTDOWN_TICK_MILLIS)
            now = SystemClock.uptimeMillis()
        }
    }
    val seconds = ceil((reconnectAt - now) / MILLIS_PER_SECOND).toInt()
    Row(
        modifier = modifier
            .windowInsetsPadding(WindowInsets.safeDrawing)
            .padding(12.dp)
            .widthIn(max = BANNER_MAX_WIDTH)
            .clip(MaterialTheme.shapes.large)
            .background(MaterialTheme.colorScheme.surfaceContainerHigh)
            .padding(start = 16.dp, end = 8.dp, top = 8.dp, bottom = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_wifi_off), contentDescription = null)
        Text(
            text = if (seconds > 0) {
                stringResource(R.string.stream_reconnecting_in, seconds)
            } else {
                stringResource(R.string.stream_reconnecting)
            },
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.weight(1f, fill = false).semantics { liveRegion = LiveRegionMode.Polite },
        )
        QdpButton(
            onClick = onDisconnect,
            emphasis = Emphasis.LOWEST,
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
        ) {
            Text(stringResource(R.string.action_disconnect))
        }
    }
}

/**
 * The tip on the first stream that teaches how to open the menu: Back, and
 * with a gamepad Select, Start and both bumpers, since Android takes the
 * Guide button as Home.
 */
@Composable
fun MenuCoachmark(gamepad: Boolean, modifier: Modifier = Modifier) {
    Row(
        modifier = modifier
            .windowInsetsPadding(WindowInsets.safeDrawing)
            .padding(16.dp)
            .widthIn(max = BANNER_MAX_WIDTH)
            .clip(MaterialTheme.shapes.large)
            .background(MaterialTheme.colorScheme.inverseSurface)
            .padding(16.dp)
            .semantics { liveRegion = LiveRegionMode.Polite },
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            painter = painterResource(R.drawable.ic_info),
            contentDescription = null,
            tint = MaterialTheme.colorScheme.inverseOnSurface,
        )
        Text(
            text = stringResource(if (gamepad) R.string.coachmark_menu_gamepad else R.string.coachmark_menu),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.inverseOnSurface,
        )
    }
}
