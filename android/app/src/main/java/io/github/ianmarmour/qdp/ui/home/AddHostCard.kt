package io.github.ianmarmour.qdp.ui.home

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.focusRing

/** The size of the plus sign's circle. */
private val ICON_CIRCLE_SIZE = 48.dp

/** The width of the card's outline. */
private val OUTLINE_WIDTH = 1.dp

/**
 * The last card of the grid: an outlined "Add by address" for a computer
 * that is not found on the network. Adding is rare, so it is a card rather
 * than a floating button, which would cover cards on short screens and which
 * D-pad navigation reaches unreliably.
 */
@Composable
fun AddHostCard(onClick: () -> Unit, modifier: Modifier = Modifier) {
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.large
    Row(
        modifier = modifier
            .focusRing(interaction, shape)
            .clip(shape)
            .border(BorderStroke(OUTLINE_WIDTH, MaterialTheme.colorScheme.outlineVariant), shape)
            .clickable(interactionSource = interaction, indication = ripple(), role = Role.Button, onClick = onClick)
            .padding(16.dp),
        horizontalArrangement = Arrangement.spacedBy(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            modifier = Modifier
                .size(ICON_CIRCLE_SIZE)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.secondaryContainer),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painter = painterResource(R.drawable.ic_add),
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSecondaryContainer,
            )
        }
        Text(
            text = stringResource(R.string.action_add_by_address),
            style = MaterialTheme.typography.titleMedium,
            color = MaterialTheme.colorScheme.onSurface,
        )
    }
}
