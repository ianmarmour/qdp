package io.github.ianmarmour.qdp.ui.pairing

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.InputTransformation
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.maxLength
import androidx.compose.foundation.text.input.then
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R

/** The digits of a pairing code. */
const val CODE_LENGTH = 4

/** The smallest a code cell draws; it grows with the font. */
private val CELL_MIN_WIDTH = 56.dp

/** The smallest height of a code cell. */
private val CELL_MIN_HEIGHT = 72.dp

/** The outline of the cell the next digit goes in. */
private val ACTIVE_OUTLINE = 2.dp

/** The outline of the other cells. */
private val CELL_OUTLINE = 1.dp

/** Tabular figures, so that every digit takes the same width. */
private const val TABULAR_FIGURES = "tnum"

/** Accepts only digits, as typed or pasted. */
private val DigitsOnly = InputTransformation {
    if (!asCharSequence().all { it.isDigit() }) {
        revertAllChanges()
    }
}

/**
 * The pairing code's field: one text field of up to four digits, drawn as
 * four cells with the next one outlined. Being one field, it is one focus
 * stop and one TalkBack item ("Pairing code, 2 of 4 digits"). With
 * [acceptsTyping] off, as when the on-screen keypad types into [state], it
 * takes no focus and never raises the keyboard.
 */
@Composable
fun CodeField(state: TextFieldState, acceptsTyping: Boolean, modifier: Modifier = Modifier) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val spoken = pluralStringResource(R.plurals.pair_code_spoken, CODE_LENGTH, state.text.length, CODE_LENGTH)
    BasicTextField(
        state = state,
        modifier = modifier
            .focusProperties { canFocus = acceptsTyping }
            .semantics { contentDescription = spoken },
        readOnly = !acceptsTyping,
        inputTransformation = InputTransformation.maxLength(CODE_LENGTH).then(DigitsOnly),
        textStyle = TextStyle(color = Color.Transparent),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
        lineLimits = TextFieldLineLimits.SingleLine,
        interactionSource = interaction,
        cursorBrush = SolidColor(Color.Transparent),
        decorator = { innerTextField ->
            Box {
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    for (index in 0 until CODE_LENGTH) {
                        CodeCell(
                            digit = state.text.getOrNull(index),
                            next = index == state.text.length && (focused || !acceptsTyping),
                        )
                    }
                }
                // The real field lies invisibly over the cells, taking taps and typing.
                Box(modifier = Modifier.matchParentSize().alpha(0f)) { innerTextField() }
            }
        },
    )
}

/** One cell of the code: its [digit] once entered, outlined in the primary color if it is [next]. */
@Composable
private fun CodeCell(digit: Char?, next: Boolean) {
    val colors = MaterialTheme.colorScheme
    val shape = MaterialTheme.shapes.medium
    Box(
        modifier = Modifier
            .defaultMinSize(CELL_MIN_WIDTH, CELL_MIN_HEIGHT)
            .clip(shape)
            .background(colors.surfaceContainerHighest)
            .border(
                width = if (next) ACTIVE_OUTLINE else CELL_OUTLINE,
                color = if (next) colors.primary else colors.outlineVariant,
                shape = shape,
            ),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = digit?.toString().orEmpty(),
            style = MaterialTheme.typography.displayMedium.merge(TextStyle(fontFeatureSettings = TABULAR_FIGURES)),
            color = colors.onSurface,
        )
    }
}
