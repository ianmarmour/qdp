package io.github.ianmarmour.qdp.ui.pairing

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.ColorSource
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.focusRing
import io.github.ianmarmour.qdp.ui.theme.QdpTheme

/** The size of a key, beyond the 48dp minimum, so that a gamepad user sees each clearly. */
private val KEY_WIDTH = 72.dp

/** The height of a key. */
private val KEY_HEIGHT = 52.dp

/** The space between keys. */
private val KEY_SPACING = 8.dp

/** The keypad's digits, row by row, above its bottom row of Delete and 0. */
private val DIGIT_ROWS = listOf("123", "456", "789")

/**
 * A 3×4 number pad for entering the pairing code with a gamepad or D-pad,
 * shown instead of the keyboard on devices driven that way: A presses the
 * focused key, X deletes (handled by the dialog), B cancels. [firstKey]
 * focuses the 1 key.
 */
@Composable
fun Keypad(onDigit: (Char) -> Unit, onDelete: () -> Unit, firstKey: FocusRequester, modifier: Modifier = Modifier) {
    Column(modifier = modifier, verticalArrangement = Arrangement.spacedBy(KEY_SPACING)) {
        for (row in DIGIT_ROWS) {
            Row(horizontalArrangement = Arrangement.spacedBy(KEY_SPACING)) {
                for (digit in row) {
                    Key(
                        onClick = { onDigit(digit) },
                        modifier = if (digit == DIGIT_ROWS.first().first()) Modifier.focusRequester(firstKey) else Modifier,
                    ) {
                        Text(digit.toString(), style = MaterialTheme.typography.titleLarge)
                    }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(KEY_SPACING)) {
            val delete = stringResource(R.string.keypad_delete)
            Key(onClick = onDelete, modifier = Modifier.semantics { contentDescription = delete }) {
                Icon(painterResource(R.drawable.ic_backspace), contentDescription = null)
            }
            Key(onClick = { onDigit('0') }) {
                Text("0", style = MaterialTheme.typography.titleLarge)
            }
            Spacer(modifier = Modifier.width(KEY_WIDTH))
        }
    }
}

/** A key of the pad, showing [content]. */
@Composable
private fun Key(onClick: () -> Unit, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.medium
    Box(
        modifier = modifier
            .focusRing(interaction, shape)
            .widthIn(min = KEY_WIDTH)
            .heightIn(min = KEY_HEIGHT)
            .clip(shape)
            .background(MaterialTheme.colorScheme.surfaceContainerHighest)
            .clickable(interactionSource = interaction, indication = ripple(), role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        content()
    }
}

/** The keypad in Ember's dark theme, as the stream shows it. */
@Preview
@Composable
private fun KeypadPreview() {
    QdpTheme(ColorSource.EMBER, dark = true) {
        Keypad(onDigit = {}, onDelete = {}, firstKey = remember { FocusRequester() })
    }
}
