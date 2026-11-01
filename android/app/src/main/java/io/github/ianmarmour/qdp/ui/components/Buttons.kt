package io.github.ianmarmour.qdp.ui.components

import androidx.annotation.DrawableRes
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonColors
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource

/** How much a button stands out, from Material's button styles. */
enum class Emphasis {
    /** A filled button, for a screen's main action. */
    HIGH,

    /** A tonal button, for an important but secondary action. */
    MEDIUM,

    /** An outlined button. */
    LOW,

    /** A text button, for dismissive actions. */
    LOWEST,
}

/**
 * An icon button showing the drawable [icon], described to accessibility
 * services as [description], that also shows the focus ring.
 */
@Composable
fun QdpIconButton(
    onClick: () -> Unit,
    @DrawableRes icon: Int,
    description: String,
    modifier: Modifier = Modifier,
) {
    val interaction = remember { MutableInteractionSource() }
    IconButton(
        onClick = onClick,
        modifier = modifier.focusRing(interaction, CircleShape),
        interactionSource = interaction,
    ) {
        Icon(painterResource(icon), contentDescription = description)
    }
}

/**
 * A Material button of [emphasis] that also shows the focus ring, which a
 * gamepad or D-pad user needs to see where they are. [colors] overrides the
 * style's colors, as for a destructive action.
 */
@Composable
fun QdpButton(
    onClick: () -> Unit,
    emphasis: Emphasis,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    colors: ButtonColors? = null,
    content: @Composable RowScope.() -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val shape = ButtonDefaults.shape
    val ringed = modifier.focusRing(interaction, shape)
    when (emphasis) {
        Emphasis.HIGH -> Button(
            onClick = onClick,
            modifier = ringed,
            enabled = enabled,
            colors = colors ?: ButtonDefaults.buttonColors(),
            interactionSource = interaction,
            content = content,
        )
        Emphasis.MEDIUM -> FilledTonalButton(
            onClick = onClick,
            modifier = ringed,
            enabled = enabled,
            colors = colors ?: ButtonDefaults.filledTonalButtonColors(),
            interactionSource = interaction,
            content = content,
        )
        Emphasis.LOW -> OutlinedButton(
            onClick = onClick,
            modifier = ringed,
            enabled = enabled,
            colors = colors ?: ButtonDefaults.outlinedButtonColors(),
            interactionSource = interaction,
            content = content,
        )
        Emphasis.LOWEST -> TextButton(
            onClick = onClick,
            modifier = ringed,
            enabled = enabled,
            colors = colors ?: ButtonDefaults.textButtonColors(),
            interactionSource = interaction,
            content = content,
        )
    }
}
