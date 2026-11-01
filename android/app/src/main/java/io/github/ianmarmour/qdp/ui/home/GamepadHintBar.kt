package io.github.ianmarmour.qdp.ui.home

import androidx.annotation.StringRes
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R

/** The smallest a button glyph draws, so that a single letter sits in a circle. */
private val GLYPH_MIN_SIZE = 24.dp

/** The width of a button glyph's outline. */
private val GLYPH_OUTLINE = 1.5.dp

/**
 * The bar along the bottom of the hosts screen while a gamepad is
 * connected, naming what its buttons do: A does [actionLabel], the focused
 * computer's action; Y opens its options; X adds a computer; and Start opens
 * settings. B goes back, as everywhere.
 */
@Composable
fun GamepadHintBar(actionLabel: String, modifier: Modifier = Modifier) {
    Surface(color = MaterialTheme.colorScheme.surfaceContainer, modifier = modifier) {
        FlowRow(
            modifier = Modifier
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Bottom))
                .fillMaxWidth()
                .semantics(mergeDescendants = true) {}
                .padding(horizontal = 16.dp, vertical = 10.dp),
            horizontalArrangement = Arrangement.spacedBy(24.dp, Alignment.CenterHorizontally),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Hint(R.string.button_a, actionLabel)
            Hint(R.string.button_y, stringResource(R.string.action_options))
            Hint(R.string.button_x, stringResource(R.string.hint_add))
            Hint(R.string.button_start, stringResource(R.string.action_settings))
        }
    }
}

/** A gamepad [button]'s glyph, then what it does. */
@Composable
private fun Hint(@StringRes button: Int, label: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(
            modifier = Modifier
                .defaultMinSize(GLYPH_MIN_SIZE, GLYPH_MIN_SIZE)
                .border(GLYPH_OUTLINE, MaterialTheme.colorScheme.onSurfaceVariant, CircleShape)
                .padding(horizontal = 6.dp, vertical = 2.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(stringResource(button), style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold)
        }
        Text(label, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
