package io.github.ianmarmour.qdp.ui.home

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.ColorSource
import io.github.ianmarmour.qdp.HostCard
import io.github.ianmarmour.qdp.HostStatus
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.focusRing
import io.github.ianmarmour.qdp.ui.theme.QdpTheme

/** The size of a card's computer icon and its tonal circle. */
private val AVATAR_SIZE = 48.dp

/** The size of the icon on a status chip. */
private val CHIP_ICON_SIZE = 18.dp

/** The padding inside a card. */
private val CARD_PADDING = 16.dp

/** How a status shows: its chip's words and icon, what tapping does, and how it is read aloud. */
private data class StatusLook(
    @param:StringRes val label: Int,
    @param:StringRes val spoken: Int,
    @param:StringRes val action: Int,
    @param:DrawableRes val icon: Int,
)

/** Returns how [status] shows. */
private fun lookOf(status: HostStatus): StatusLook = when (status) {
    HostStatus.READY -> StatusLook(
        R.string.status_ready,
        R.string.status_ready_spoken,
        R.string.action_connect,
        R.drawable.ic_circle_fill1,
    )
    HostStatus.GATEWAY -> StatusLook(
        R.string.status_gateway,
        R.string.status_gateway_spoken,
        R.string.action_connect,
        R.drawable.ic_circle_fill1,
    )
    HostStatus.NEW -> StatusLook(R.string.status_new, R.string.status_new_spoken, R.string.action_pair, R.drawable.ic_key)
    HostStatus.KEY_MISMATCH -> StatusLook(
        R.string.status_key_mismatch,
        R.string.status_key_mismatch_spoken,
        R.string.action_learn_more,
        R.drawable.ic_warning,
    )
    HostStatus.PAIRING_OFF -> StatusLook(
        R.string.status_pairing_off,
        R.string.status_pairing_off_spoken,
        R.string.action_how_to_pair,
        R.drawable.ic_lock,
    )
    HostStatus.NOT_SEEN -> StatusLook(
        R.string.status_not_seen,
        R.string.status_not_seen_spoken,
        R.string.action_try_anyway,
        R.drawable.ic_radio_button_unchecked,
    )
}

/** Returns the words for what tapping a computer of [status] does, as the gamepad hint shows them. */
@StringRes
fun actionLabelOf(status: HostStatus): Int = lookOf(status).action

/** Returns the words naming [status], as its chip shows them. */
@StringRes
fun statusLabelOf(status: HostStatus): Int = lookOf(status).label

/** Returns the card's background: computers ready to use stand out, those not found recede. */
@Composable
private fun containerColor(status: HostStatus): Color = when (status) {
    HostStatus.READY -> MaterialTheme.colorScheme.surfaceContainerHigh
    HostStatus.NOT_SEEN -> MaterialTheme.colorScheme.surfaceContainerLow
    else -> MaterialTheme.colorScheme.surfaceContainer
}

/**
 * A computer on the hosts screen, as one click target: its icon, name,
 * status chip and address, and an overflow menu of [onForget] (for paired
 * computers) and [onDetails], which a long press or the gamepad's Y opens
 * too. TalkBack reads it as one item ("Desk-PC, ready, paired. Double-tap
 * to connect.") with the menu's entries as custom actions.
 */
@Composable
fun HostCardView(
    card: HostCard,
    menuExpanded: Boolean,
    onMenuExpandedChange: (Boolean) -> Unit,
    onOpen: () -> Unit,
    onForget: () -> Unit,
    onDetails: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val look = lookOf(card.status)
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.large
    val spoken = stringResource(R.string.card_spoken, card.name, stringResource(look.spoken))
    val actionLabel = stringResource(look.action)
    val forgetLabel = stringResource(R.string.action_forget)
    val detailsLabel = stringResource(R.string.action_details)
    val optionsLabel = stringResource(R.string.action_options)
    val canForget = card.paired != null || card.gateway != null
    Box(
        modifier = modifier
            .focusRing(interaction, shape)
            .semantics(mergeDescendants = true) {
                contentDescription = spoken
                customActions = listOfNotNull(
                    CustomAccessibilityAction(forgetLabel) { onForget(); true }.takeIf { canForget },
                    CustomAccessibilityAction(detailsLabel) { onDetails(); true },
                )
            }
            .clip(shape)
            .background(containerColor(card.status))
            .combinedClickable(
                interactionSource = interaction,
                indication = ripple(),
                onClickLabel = actionLabel,
                onLongClickLabel = optionsLabel,
                onLongClick = { onMenuExpandedChange(true) },
                onClick = onOpen,
            ),
    ) {
        CompositionLocalProvider(LocalContentColor provides MaterialTheme.colorScheme.onSurface) {
            Row(
                modifier = Modifier
                    .clearAndSetSemantics {}
                    .padding(start = CARD_PADDING, top = CARD_PADDING, bottom = CARD_PADDING),
                horizontalArrangement = Arrangement.spacedBy(CARD_PADDING),
            ) {
                HostAvatar(card.status)
                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(
                        text = card.name,
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    AnimatedContent(targetState = card.status, label = "status") { status -> StatusChip(status) }
                    Text(
                        text = card.address,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (card.status == HostStatus.NEW) {
                        PairPill()
                    }
                }
                Box {
                    // The gamepad's Y opens the menu, so the button is not a
                    // focus stop of its own.
                    IconButton(
                        onClick = { onMenuExpandedChange(true) },
                        modifier = Modifier.focusProperties { canFocus = false },
                    ) {
                        Icon(painterResource(R.drawable.ic_more_vert), contentDescription = null)
                    }
                    DropdownMenu(expanded = menuExpanded, onDismissRequest = { onMenuExpandedChange(false) }) {
                        if (canForget) {
                            MenuEntry(R.string.action_forget, R.drawable.ic_delete) {
                                onMenuExpandedChange(false)
                                onForget()
                            }
                        }
                        MenuEntry(R.string.action_details, R.drawable.ic_info) {
                            onMenuExpandedChange(false)
                            onDetails()
                        }
                    }
                }
            }
        }
    }
}

/** An entry of a card's overflow menu, labelled [label] with the drawable [icon]. */
@Composable
private fun MenuEntry(@StringRes label: Int, @DrawableRes icon: Int, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(stringResource(label)) },
        leadingIcon = { Icon(painterResource(icon), contentDescription = null) },
        onClick = onClick,
    )
}

/** A computer icon in a tonal circle: filled and in the primary container while the computer is ready. */
@Composable
private fun HostAvatar(status: HostStatus) {
    val colors = MaterialTheme.colorScheme
    val (background, foreground) = when (status) {
        HostStatus.READY -> colors.primaryContainer to colors.onPrimaryContainer
        HostStatus.NEW -> colors.tertiaryContainer to colors.onTertiaryContainer
        HostStatus.KEY_MISMATCH -> colors.errorContainer to colors.onErrorContainer
        HostStatus.PAIRING_OFF, HostStatus.NOT_SEEN -> colors.surfaceContainerHighest to colors.onSurfaceVariant
    }
    Box(
        modifier = Modifier.size(AVATAR_SIZE).clip(CircleShape).background(background),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            painter = painterResource(
                if (status == HostStatus.READY) R.drawable.ic_computer_fill1 else R.drawable.ic_computer,
            ),
            contentDescription = null,
            tint = foreground,
        )
    }
}

/** A pill naming [status] with an icon, so that status never rests on color alone. */
@Composable
private fun StatusChip(status: HostStatus) {
    val look = lookOf(status)
    val colors = MaterialTheme.colorScheme
    val (background, foreground) = when (status) {
        HostStatus.READY -> colors.secondaryContainer to colors.onSecondaryContainer
        HostStatus.NEW -> colors.tertiaryContainer to colors.onTertiaryContainer
        HostStatus.KEY_MISMATCH -> colors.errorContainer to colors.onErrorContainer
        HostStatus.PAIRING_OFF, HostStatus.NOT_SEEN -> colors.surfaceContainerHighest to colors.onSurfaceVariant
    }
    Row(
        modifier = Modifier
            .clip(CircleShape)
            .background(background)
            .padding(start = 8.dp, end = 12.dp, top = 4.dp, bottom = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            painter = painterResource(look.icon),
            contentDescription = null,
            tint = if (status == HostStatus.READY) colors.primary else foreground,
            modifier = Modifier.size(CHIP_ICON_SIZE),
        )
        Text(stringResource(look.label), style = MaterialTheme.typography.labelLarge, color = foreground)
    }
}

/** The tonal "Pair" pill on a new computer's card, which tapping anywhere on the card answers. */
@Composable
private fun PairPill() {
    Text(
        text = stringResource(R.string.action_pair),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSecondaryContainer,
        modifier = Modifier
            .padding(top = 4.dp)
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.secondaryContainer)
            .padding(horizontal = 16.dp, vertical = 8.dp),
    )
}

/** The cards in each state, in Ember's light theme. */
@Preview(widthDp = 280)
@Composable
private fun HostCardPreview() {
    QdpTheme(ColorSource.EMBER, dark = false) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            for (status in HostStatus.entries) {
                HostCardView(
                    card = HostCard("$status", "Desk-PC", "192.168.1.20:4433", status, null, null, null),
                    menuExpanded = false,
                    onMenuExpandedChange = {},
                    onOpen = {},
                    onForget = {},
                    onDetails = {},
                )
            }
        }
    }
}
