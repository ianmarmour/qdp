package io.github.ianmarmour.qdp.ui.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.FocusGrowth
import io.github.ianmarmour.qdp.ui.components.QdpButton
import io.github.ianmarmour.qdp.ui.components.focusRing
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import io.github.ianmarmour.qdp.ui.components.rememberInitialFocus

/** The narrowest width that shows the choices as segmented buttons. */
private val SEGMENTED_MIN_WIDTH = 400.dp

/** The font scale above which the choices move into a dialog, where their names have room. */
private const val LARGE_FONT_SCALE = 1.3f

/**
 * A setting with a few named choices, of which [selected] is chosen:
 * segmented buttons below its [title] where they fit, or, on narrow windows
 * and at large font sizes, a row that opens a dialog of radio buttons.
 * [supporting] explains the choice; [description], if given, explains each
 * choice in the dialog.
 *
 * Material 3 Expressive replaces segmented buttons with connected button
 * groups; keeping them behind this one function makes that a one-file
 * change.
 */
@Composable
fun <T> ChoiceRow(
    title: String,
    supporting: String,
    choices: List<T>,
    selected: T,
    label: @Composable (T) -> String,
    onSelect: (T) -> Unit,
    description: (@Composable (T) -> String)? = null,
) {
    BoxWithConstraints {
        val fits = maxWidth >= SEGMENTED_MIN_WIDTH && LocalDensity.current.fontScale <= LARGE_FONT_SCALE
        if (fits) {
            SegmentedChoice(title, supporting, choices, selected, label, onSelect)
        } else {
            DialogChoice(title, supporting, choices, selected, label, onSelect, description)
        }
    }
}

/** The choices as a row of segmented buttons. */
@Composable
private fun <T> SegmentedChoice(
    title: String,
    supporting: String,
    choices: List<T>,
    selected: T,
    label: @Composable (T) -> String,
    onSelect: (T) -> Unit,
) {
    Column(
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(title, style = MaterialTheme.typography.bodyLarge)
        SingleChoiceSegmentedButtonRow(modifier = Modifier.fillMaxWidth()) {
            choices.forEachIndexed { index, choice ->
                val interaction = remember { MutableInteractionSource() }
                val shape = SegmentedButtonDefaults.itemShape(index, choices.size)
                SegmentedButton(
                    selected = choice == selected,
                    onClick = { onSelect(choice) },
                    shape = shape,
                    modifier = Modifier.focusRing(interaction, shape),
                    interactionSource = interaction,
                    icon = {},
                    label = { Text(label(choice), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                )
            }
        }
        Text(supporting, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** The setting as a row showing the choice, which opens a dialog of radio buttons. */
@Composable
private fun <T> DialogChoice(
    title: String,
    supporting: String,
    choices: List<T>,
    selected: T,
    label: @Composable (T) -> String,
    onSelect: (T) -> Unit,
    description: (@Composable (T) -> String)?,
) {
    var open by rememberSaveable { mutableStateOf(false) }
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.medium
    ListItem(
        headlineContent = { Text(title) },
        supportingContent = { Text(label(selected) + " · " + supporting) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier
            .focusRing(interaction, shape, FocusGrowth.NONE)
            .clip(shape)
            .clickable(interactionSource = interaction, indication = ripple(), role = Role.Button) { open = true },
    )
    if (open) {
        AlertDialog(
            onDismissRequest = { open = false },
            modifier = Modifier.gamepadButtons(),
            title = { Text(title) },
            text = {
                val current = rememberInitialFocus()
                Column(modifier = Modifier.selectableGroup().verticalScroll(rememberScrollState())) {
                    for (choice in choices) {
                        val isSelected = choice == selected
                        RadioRow(
                            text = label(choice),
                            description = description?.invoke(choice),
                            selected = isSelected,
                            modifier = if (isSelected) Modifier.focusRequester(current) else Modifier,
                        ) {
                            onSelect(choice)
                            open = false
                        }
                    }
                }
            },
            confirmButton = {
                QdpButton(onClick = { open = false }, emphasis = Emphasis.LOWEST) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }
}

/** A radio button with its [text] and optional [description], the whole row selecting it. */
@Composable
private fun RadioRow(text: String, description: String?, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.small
    Row(
        modifier = modifier
            .fillMaxWidth()
            .focusRing(interaction, shape, FocusGrowth.NONE)
            .clip(shape)
            .selectable(
                selected = selected,
                interactionSource = interaction,
                indication = ripple(),
                role = Role.RadioButton,
                onClick = onClick,
            )
            .padding(vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = selected, onClick = null)
        Column {
            Text(text, style = MaterialTheme.typography.bodyLarge)
            if (description != null) {
                Text(
                    text = description,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
