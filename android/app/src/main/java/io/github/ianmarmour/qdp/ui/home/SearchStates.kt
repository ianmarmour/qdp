package io.github.ianmarmour.qdp.ui.home

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.QdpButton
import io.github.ianmarmour.qdp.ui.components.animationsEnabled
import io.github.ianmarmour.qdp.ui.components.rememberKeyDriven

/** How long one slow pulse of the search takes. */
private const val PULSE_MILLIS = 2400

/** How far the search's ring grows, beyond its circle. */
private const val PULSE_GROWTH = 0.6f

/** The size of the empty state's icon circle. */
private val ICON_CIRCLE_SIZE = 88.dp

/** The size of the icon in the empty state's circle. */
private val ICON_SIZE = 40.dp

/** The widest the empty state's text runs. */
private val MAX_TEXT_WIDTH = 480.dp

/** The size of the inline search's dot. */
private val DOT_SIZE = 10.dp

/** The checklist shown after searching for a while without finding anything. */
private val CHECKLIST = listOf(
    R.drawable.ic_wifi to R.string.check_same_wifi,
    R.drawable.ic_computer to R.string.check_qdp_running,
    R.drawable.ic_shield to R.string.check_firewall,
)

/**
 * What the hosts screen shows while it lists no computer: a slow pulse and
 * "Looking for computers on your Wi-Fi…" while searching, a checklist once
 * it has searched for a while, or a way to Wi-Fi settings when not on
 * Wi-Fi. Its main button takes focus when keys drive the screen.
 */
@Composable
fun EmptyState(
    phase: SearchPhase,
    onAdd: () -> Unit,
    onSetupHelp: () -> Unit,
    onWifiSettings: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(modifier = modifier.verticalScroll(rememberScrollState()), contentAlignment = Alignment.TopCenter) {
        AnimatedContent(targetState = phase, label = "search") { shown ->
            Column(
                modifier = Modifier.widthIn(max = MAX_TEXT_WIDTH).fillMaxWidth().padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                val main = remember { FocusRequester() }
                val keyDriven = rememberKeyDriven()
                LaunchedEffect(keyDriven) {
                    if (keyDriven) {
                        withFrameNanos {}
                        main.requestFocus()
                    }
                }
                when (shown) {
                    SearchPhase.NO_WIFI -> {
                        IconCircle(R.drawable.ic_wifi_off, pulsing = false)
                        Heading(R.string.no_wifi_title)
                        Body(R.string.no_wifi_body)
                        Actions {
                            QdpButton(
                                onClick = onWifiSettings,
                                emphasis = Emphasis.HIGH,
                                modifier = Modifier.focusRequester(main),
                            ) {
                                Text(stringResource(R.string.action_wifi_settings))
                            }
                            QdpButton(onClick = onAdd, emphasis = Emphasis.LOWEST) {
                                Text(stringResource(R.string.action_add_by_address))
                            }
                        }
                    }
                    SearchPhase.QUIET -> {
                        IconCircle(R.drawable.ic_wifi_find, pulsing = true)
                        Heading(R.string.quiet_title)
                        Body(R.string.quiet_body)
                        Checklist()
                        SearchActions(onAdd, onSetupHelp, main)
                    }
                    SearchPhase.SEARCHING, SearchPhase.FOUND -> {
                        IconCircle(R.drawable.ic_wifi_find, pulsing = true)
                        Heading(R.string.searching_title)
                        Body(R.string.searching_body)
                        SearchActions(onAdd, onSetupHelp, main)
                    }
                }
            }
        }
    }
}

/**
 * The line above the grid while computers are listed but the search goes
 * on: a pulsing dot and "Looking for more computers…", or, off Wi-Fi, a
 * tonal banner with a way to Wi-Fi settings.
 */
@Composable
fun SearchBanner(phase: SearchPhase, onWifiSettings: () -> Unit, modifier: Modifier = Modifier) {
    when (phase) {
        SearchPhase.FOUND -> Unit
        SearchPhase.SEARCHING, SearchPhase.QUIET -> Row(
            modifier = modifier.padding(vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PulseDot()
            Text(
                text = stringResource(R.string.searching_inline),
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        SearchPhase.NO_WIFI -> Row(
            modifier = modifier
                .clip(MaterialTheme.shapes.large)
                .background(MaterialTheme.colorScheme.secondaryContainer)
                .padding(16.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(painterResource(R.drawable.ic_wifi_off), contentDescription = null)
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = stringResource(R.string.no_wifi_title),
                    style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.semantics { heading() },
                )
                Text(stringResource(R.string.no_wifi_body), style = MaterialTheme.typography.bodyMedium)
            }
            QdpButton(onClick = onWifiSettings, emphasis = Emphasis.HIGH) {
                Text(stringResource(R.string.action_wifi_settings))
            }
        }
    }
}

/** The searching states' buttons: [onAdd], which [main] focuses, and [onSetupHelp]. */
@Composable
private fun SearchActions(onAdd: () -> Unit, onSetupHelp: () -> Unit, main: FocusRequester) {
    Actions {
        QdpButton(onClick = onAdd, emphasis = Emphasis.MEDIUM, modifier = Modifier.focusRequester(main)) {
            Text(stringResource(R.string.action_add_by_address))
        }
        QdpButton(onClick = onSetupHelp, emphasis = Emphasis.LOWEST) {
            Text(stringResource(R.string.action_setup_help))
        }
    }
}

/** Lays out [content]'s buttons side by side, wrapping when they do not fit. */
@Composable
private fun Actions(content: @Composable () -> Unit) {
    FlowRow(
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        content()
    }
}

/** The empty state's title. */
@Composable
private fun Heading(@StringRes text: Int) {
    Text(
        text = stringResource(text),
        style = MaterialTheme.typography.headlineSmall,
        textAlign = TextAlign.Center,
        modifier = Modifier.semantics { heading() },
    )
}

/** The empty state's explanation. */
@Composable
private fun Body(@StringRes text: Int) {
    Text(
        text = stringResource(text),
        style = MaterialTheme.typography.bodyLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
    )
}

/** What to check when nothing is found. */
@Composable
private fun Checklist() {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        for ((icon, text) in CHECKLIST) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(painterResource(icon), contentDescription = null, tint = MaterialTheme.colorScheme.primary)
                Text(stringResource(text), style = MaterialTheme.typography.bodyLarge)
            }
        }
    }
}

/** Returns a value that runs from 0 to 1 over and over while [running] and animations are on, and stays 0 otherwise. */
@Composable
private fun pulse(running: Boolean, repeatMode: RepeatMode): State<Float> {
    if (!running || !animationsEnabled()) {
        return remember { mutableFloatStateOf(0f) }
    }
    return rememberInfiniteTransition(label = "pulse").animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(PULSE_MILLIS, easing = LinearEasing), repeatMode),
        label = "pulse",
    )
}

/** The drawable [icon] in a tonal circle, with a ring that slowly grows from it and fades while [pulsing]. */
@Composable
private fun IconCircle(@DrawableRes icon: Int, pulsing: Boolean) {
    val progress = pulse(pulsing, RepeatMode.Restart)
    Box(modifier = Modifier.size(ICON_CIRCLE_SIZE * (1 + PULSE_GROWTH)), contentAlignment = Alignment.Center) {
        if (pulsing) {
            Box(
                modifier = Modifier
                    .size(ICON_CIRCLE_SIZE)
                    .graphicsLayer {
                        val scale = 1 + PULSE_GROWTH * progress.value
                        scaleX = scale
                        scaleY = scale
                        alpha = 1 - progress.value
                    }
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.primaryContainer),
            )
        }
        Box(
            modifier = Modifier
                .size(ICON_CIRCLE_SIZE)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.primaryContainer),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painter = painterResource(icon),
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onPrimaryContainer,
                modifier = Modifier.size(ICON_SIZE),
            )
        }
    }
}

/** A small dot that slowly brightens and dims while the search runs. */
@Composable
private fun PulseDot() {
    val progress = pulse(running = true, repeatMode = RepeatMode.Reverse)
    Box(
        modifier = Modifier
            .size(DOT_SIZE)
            .graphicsLayer { alpha = DOT_MIN_ALPHA + (1 - DOT_MIN_ALPHA) * progress.value }
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.primary),
    )
}

/** The dot's opacity at its dimmest. */
private const val DOT_MIN_ALPHA = 0.3f
