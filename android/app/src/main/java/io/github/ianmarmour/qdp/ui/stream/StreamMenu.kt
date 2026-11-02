package io.github.ianmarmour.qdp.ui.stream

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusGroup
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.TouchMode
import io.github.ianmarmour.qdp.ui.components.GamepadButton
import io.github.ianmarmour.qdp.ui.components.focusRing
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import io.github.ianmarmour.qdp.ui.components.rememberInitialFocus

/** The panel's width when it slides in from the side, in landscape. */
private val SIDE_PANEL_WIDTH = 360.dp

/** The panel's rounded corners, on the side facing the video. */
private val PANEL_CORNER = 28.dp

/** The smallest a tile is, beyond the usual 48dp, so that it is easy to hit on a handheld. */
private val TILE_MIN_HEIGHT = 64.dp

/** The tiles in each row. */
private const val TILE_COLUMNS = 2

/** How far the panel follows a predictive back gesture, as a share of its size, before it closes. */
private const val BACK_TRAVEL = 0.2f

/** A tile of the menu: its icon, its name, the setting it shows, and what pressing it does. */
private class Tile(@param:DrawableRes val icon: Int, val label: String, val value: String?, val onClick: () -> Unit)

/**
 * The stream's menu, over a dark scrim that closes it when tapped: a panel
 * from the right in landscape and from the bottom in portrait, headed by
 * the host's name and live statistics, with Resume (focused first),
 * Keyboard, Controller, Touch and Stats, and Disconnect set apart at the
 * bottom in the error color. B, Back, Escape and Start close it; focus stays inside.
 */
@Composable
fun StreamMenu(ui: StreamUiState, actions: StreamActions) {
    val open = ui.menu == Visibility.SHOWN
    val scrim = MaterialTheme.colorScheme.surfaceContainerLowest.copy(alpha = OVERLAY_ALPHA)
    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val side = maxWidth > maxHeight
        val panelHeight = maxHeight
        AnimatedVisibility(visible = open, enter = fadeIn(), exit = fadeOut()) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .graphicsLayer { alpha = 1 - ui.backProgress }
                    .background(scrim)
                    .pointerInput(actions) { detectTapGestures { actions.resume() } },
            )
        }
        AnimatedVisibility(
            visible = open,
            modifier = Modifier.align(if (side) Alignment.CenterEnd else Alignment.BottomCenter),
            enter = if (side) slideInHorizontally { it } else slideInVertically { it },
            exit = if (side) slideOutHorizontally { it } else slideOutVertically { it },
        ) {
            MenuPanel(
                ui = ui,
                actions = actions,
                side = side,
                maxHeight = panelHeight,
                modifier = Modifier.graphicsLayer {
                    val travel = ui.backProgress * BACK_TRAVEL
                    if (side) translationX = size.width * travel else translationY = size.height * travel
                },
            )
        }
    }
}

/** The menu's panel: [side] in landscape, at the bottom otherwise, at most [maxHeight] tall. */
@Composable
private fun MenuPanel(ui: StreamUiState, actions: StreamActions, side: Boolean, maxHeight: Dp, modifier: Modifier) {
    val resume = rememberInitialFocus()
    val shape = if (side) {
        RoundedCornerShape(topStart = PANEL_CORNER, bottomStart = PANEL_CORNER)
    } else {
        RoundedCornerShape(topStart = PANEL_CORNER, topEnd = PANEL_CORNER)
    }
    val insets = if (side) {
        WindowInsets.safeDrawing.only(WindowInsetsSides.Vertical + WindowInsetsSides.End)
    } else {
        WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Bottom)
    }
    val tiles = listOf(
        Tile(R.drawable.ic_play_arrow_fill1, stringResource(R.string.menu_resume), null, actions.resume),
        Tile(R.drawable.ic_keyboard, stringResource(R.string.menu_keyboard), null, actions.showKeyboard),
        Tile(
            icon = R.drawable.ic_sports_esports,
            label = stringResource(R.string.menu_controller),
            value = stringResource(
                if (ui.controller == Visibility.SHOWN) R.string.controller_shown else R.string.controller_hidden,
            ),
            onClick = actions.toggleController,
        ),
        Tile(
            icon = if (ui.touchMode == TouchMode.TRACKPAD) R.drawable.ic_mouse else R.drawable.ic_touch_app,
            label = stringResource(R.string.menu_touch),
            value = stringResource(ui.touchMode.label),
            onClick = actions.toggleTouchMode,
        ),
        Tile(
            icon = R.drawable.ic_monitoring,
            label = stringResource(R.string.menu_stats),
            value = stringResource(statsLevelLabel(ui.statsLevel)),
            onClick = actions.cycleStats,
        ),
    )
    Column(
        modifier = modifier
            .then(if (side) Modifier.fillMaxHeight().width(SIDE_PANEL_WIDTH) else Modifier.fillMaxWidth())
            .clip(shape)
            .background(MaterialTheme.colorScheme.surfaceContainer)
            // Taps between the tiles stay in the panel rather than reaching
            // the scrim, which would close it.
            .pointerInput(Unit) { detectTapGestures { } }
            .focusProperties { onExit = { cancelFocusChange() } }
            .focusGroup()
            .gamepadButtons { button -> if (button == GamepadButton.START) actions.resume() }
            .onPreviewKeyEvent { event ->
                val escape = event.key == Key.Escape
                if (escape && event.type == KeyEventType.KeyUp) {
                    actions.resume()
                }
                escape
            }
            .verticalScroll(rememberScrollState())
            .heightIn(min = if (side) maxHeight else 0.dp)
            .windowInsetsPadding(insets)
            .padding(24.dp),
        verticalArrangement = Arrangement.SpaceBetween,
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Column {
                Text(
                    text = ui.hostName,
                    style = MaterialTheme.typography.titleLarge,
                    modifier = Modifier.semantics { heading() },
                )
                ui.stats?.let {
                    Text(
                        text = statsSummary(it),
                        style = MaterialTheme.typography.labelLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                for (row in tiles.chunked(TILE_COLUMNS)) {
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        for (tile in row) {
                            // Resume stands out, and takes focus first.
                            val first = tile === tiles.first()
                            val colors = MaterialTheme.colorScheme
                            MenuTile(
                                tile = tile,
                                background = if (first) colors.primaryContainer else colors.surfaceContainerHighest,
                                content = if (first) colors.onPrimaryContainer else colors.onSurface,
                                modifier = Modifier
                                    .weight(1f)
                                    .then(if (first) Modifier.focusRequester(resume) else Modifier),
                            )
                        }
                    }
                }
            }
        }
        Column(modifier = Modifier.padding(top = 16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            HorizontalDivider()
            MenuTile(
                tile = Tile(R.drawable.ic_logout, stringResource(R.string.action_disconnect), null, actions.disconnect),
                background = MaterialTheme.colorScheme.errorContainer,
                content = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

/** A menu tile, at least 64dp tall, drawn in [background] and [content] colors. */
@Composable
private fun MenuTile(tile: Tile, background: Color, content: Color, modifier: Modifier) {
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.large
    Row(
        modifier = modifier
            .focusRing(interaction, shape)
            .clip(shape)
            .background(background)
            .clickable(interactionSource = interaction, indication = ripple(), role = Role.Button, onClick = tile.onClick)
            .heightIn(min = TILE_MIN_HEIGHT)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(tile.icon), contentDescription = null, tint = content)
        Column {
            Text(tile.label, style = MaterialTheme.typography.titleSmall, color = content)
            if (tile.value != null) {
                Text(tile.value, style = MaterialTheme.typography.labelMedium, color = content)
            }
        }
    }
}

/** Returns the name of statistics [level], as the Stats tile shows it. */
@StringRes
private fun statsLevelLabel(level: StatsLevel): Int = when (level) {
    StatsLevel.HIDDEN -> R.string.stats_hidden
    StatsLevel.SUMMARY -> R.string.stats_summary_level
    StatsLevel.DETAILED -> R.string.stats_detailed_level
}
