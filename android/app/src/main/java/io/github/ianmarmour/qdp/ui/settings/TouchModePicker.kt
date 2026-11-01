package io.github.ianmarmour.qdp.ui.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.TouchMode
import io.github.ianmarmour.qdp.ui.components.focusRing

/** The narrowest width that puts the two touch modes side by side. */
private val SIDE_BY_SIDE_MIN_WIDTH = 400.dp

/** The outline of the chosen touch mode's card. */
private val SELECTED_OUTLINE = 2.dp

/** The outline of the other card. */
private val UNSELECTED_OUTLINE = 1.dp

/** The size of the check mark on the chosen card. */
private val CHECK_SIZE = 20.dp

/**
 * What touching the screen does while streaming, as two selectable cards,
 * side by side where they fit: "Trackpad: drag to move the pointer" and
 * "Direct touch: tap where you want to click". The chosen one is outlined
 * and checked, not only colored.
 */
@Composable
fun TouchModePicker(selected: TouchMode, onSelect: (TouchMode) -> Unit) {
    Column(
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(stringResource(R.string.settings_touch), style = MaterialTheme.typography.bodyLarge)
        BoxWithConstraints {
            if (maxWidth >= SIDE_BY_SIDE_MIN_WIDTH) {
                Row(modifier = Modifier.selectableGroup(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    for (mode in TouchMode.entries) {
                        TouchModeCard(mode, mode == selected, Modifier.weight(1f)) { onSelect(mode) }
                    }
                }
            } else {
                Column(modifier = Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    for (mode in TouchMode.entries) {
                        TouchModeCard(mode, mode == selected, Modifier.fillMaxWidth()) { onSelect(mode) }
                    }
                }
            }
        }
    }
}

/** A touch mode's card: its icon, name and description, checked while [selected]. */
@Composable
private fun TouchModeCard(mode: TouchMode, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.large
    val colors = MaterialTheme.colorScheme
    Column(
        modifier = modifier
            .focusRing(interaction, shape)
            .clip(shape)
            .background(if (selected) colors.secondaryContainer else colors.surfaceContainerLow)
            .border(
                width = if (selected) SELECTED_OUTLINE else UNSELECTED_OUTLINE,
                color = if (selected) colors.primary else colors.outlineVariant,
                shape = shape,
            )
            .selectable(
                selected = selected,
                interactionSource = interaction,
                indication = ripple(),
                role = Role.RadioButton,
                onClick = onClick,
            )
            .padding(16.dp),
    ) {
        Row {
            Icon(
                painter = painterResource(if (mode == TouchMode.TRACKPAD) R.drawable.ic_mouse else R.drawable.ic_touch_app),
                contentDescription = null,
                tint = colors.primary,
            )
            Spacer(modifier = Modifier.weight(1f))
            if (selected) {
                Icon(
                    painter = painterResource(R.drawable.ic_check),
                    contentDescription = null,
                    tint = colors.primary,
                    modifier = Modifier.size(CHECK_SIZE),
                )
            }
        }
        Spacer(modifier = Modifier.height(12.dp))
        Text(stringResource(mode.label), style = MaterialTheme.typography.titleMedium)
        Text(
            text = stringResource(mode.description),
            style = MaterialTheme.typography.bodyMedium,
            color = colors.onSurfaceVariant,
        )
    }
}
