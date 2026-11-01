package io.github.ianmarmour.qdp.ui.settings

import androidx.annotation.StringRes
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.ui.components.FocusGrowth
import io.github.ianmarmour.qdp.ui.components.focusRing

/** A settings section's title, which screen readers can jump between. */
@Composable
fun SectionHeader(@StringRes title: Int) {
    Text(
        text = stringResource(title),
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier
            .padding(start = 16.dp, end = 16.dp, top = 24.dp, bottom = 8.dp)
            .semantics { heading() },
    )
}

/** A setting turned on or off with a switch; the whole row toggles it. */
@Composable
fun SwitchRow(title: String, supporting: String?, checked: Boolean, onCheckedChange: (Boolean) -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.medium
    ListItem(
        headlineContent = { Text(title) },
        supportingContent = supporting?.let { { Text(it) } },
        trailingContent = { Switch(checked = checked, onCheckedChange = null) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = Modifier
            .focusRing(interaction, shape, FocusGrowth.NONE)
            .clip(shape)
            .toggleable(
                value = checked,
                interactionSource = interaction,
                indication = ripple(),
                role = Role.Switch,
                onValueChange = onCheckedChange,
            ),
    )
}

/** A fact shown in settings: its [title] and [value], which can be selected and copied. */
@Composable
fun InfoRow(title: String, value: String) {
    ListItem(
        headlineContent = { Text(title) },
        supportingContent = { SelectionContainer { Text(value) } },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
    )
}
