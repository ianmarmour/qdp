package io.github.ianmarmour.qdp.ui.home

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.HostCard
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.QdpButton
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import io.github.ianmarmour.qdp.ui.components.rememberInitialFocus

/** The size of a setup step's number circle. */
private val STEP_NUMBER_SIZE = 28.dp

/** Setup help's steps, in order. */
private val SETUP_STEPS = listOf(
    R.string.setup_help_step_install,
    R.string.setup_help_step_start,
    R.string.setup_help_step_wifi,
    R.string.setup_help_step_pair,
)

/** Shows the hosts screen's [dialog], answering its buttons through the callbacks. */
@Composable
fun HomeDialogs(
    dialog: HomeDialog,
    onDismiss: () -> Unit,
    onForget: (HostCard) -> Unit,
    onPair: (HostCard) -> Unit,
    onAddByAddressPair: (String) -> Unit,
    onConnectWithToken: (String, String, String) -> Unit,
) {
    when (dialog) {
        // Each opening starts from the settings kept, as a link fills them in.
        is HomeDialog.AddHost -> key(dialog.request) {
            AddHostDialog(dialog.advanced, onDismiss, onAddByAddressPair, onConnectWithToken)
        }
        is HomeDialog.Forget -> ForgetDialog(dialog.card, onDismiss) { onForget(dialog.card) }
        is HomeDialog.Details -> DetailsDialog(dialog.card, onDismiss)
        is HomeDialog.PairingOff -> InfoDialog(
            icon = R.drawable.ic_lock,
            title = stringResource(R.string.pairing_off_title, dialog.card.name),
            onDismiss = onDismiss,
        ) {
            Text(stringResource(R.string.pairing_off_body, dialog.card.name))
            Text(
                text = stringResource(R.string.pairing_off_advanced),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        is HomeDialog.KeyMismatch -> KeyMismatchDialog(dialog.card, onDismiss) { onPair(dialog.card) }
        HomeDialog.SetupHelp -> InfoDialog(
            icon = R.drawable.ic_help,
            title = stringResource(R.string.setup_help_title),
            onDismiss = onDismiss,
        ) {
            SETUP_STEPS.forEachIndexed { index, step -> SetupStep(index + 1, step) }
        }
    }
}

/** Asks whether to forget [card]'s computer, opening on Cancel, the safe action. */
@Composable
private fun ForgetDialog(card: HostCard, onDismiss: () -> Unit, onForget: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.gamepadButtons(),
        icon = { Icon(painterResource(R.drawable.ic_delete), contentDescription = null) },
        title = { Text(stringResource(R.string.forget_title, card.name)) },
        text = { Text(stringResource(R.string.forget_body)) },
        confirmButton = {
            QdpButton(
                onClick = onForget,
                emphasis = Emphasis.LOWEST,
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
            ) {
                Text(stringResource(R.string.action_forget))
            }
        },
        dismissButton = {
            val cancel = rememberInitialFocus()
            QdpButton(onClick = onDismiss, emphasis = Emphasis.LOWEST, modifier = Modifier.focusRequester(cancel)) {
                Text(stringResource(R.string.action_cancel))
            }
        },
    )
}

/** Shows [card]'s status, address and key pin, which can be selected and copied. */
@Composable
private fun DetailsDialog(card: HostCard, onDismiss: () -> Unit) {
    val pin = card.advertisedPin ?: card.paired?.pin
    InfoDialog(icon = R.drawable.ic_info, title = card.name, onDismiss = onDismiss) {
        SelectionContainer {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Detail(R.string.details_status, stringResource(statusLabelOf(card.status)))
                Detail(R.string.details_address, card.address)
                Detail(R.string.details_key_pin, pin ?: stringResource(R.string.details_none), monospace = pin != null)
            }
        }
    }
}

/** One labelled value of a computer's details. */
@Composable
private fun Detail(@StringRes label: Int, value: String, monospace: Boolean = false) {
    Column {
        Text(
            text = stringResource(label),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = value,
            style = MaterialTheme.typography.bodyLarge,
            fontFamily = if (monospace) FontFamily.Monospace else null,
        )
    }
}

/**
 * Explains why [card]'s computer, named like one paired with but holding
 * another key, is not connected to automatically, and offers to pair with
 * it as a new computer when it is pairing. It opens on Close, the safe action.
 */
@Composable
private fun KeyMismatchDialog(card: HostCard, onDismiss: () -> Unit, onPair: () -> Unit) {
    val canPair = card.target != null
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.gamepadButtons(),
        icon = { Icon(painterResource(R.drawable.ic_warning), contentDescription = null) },
        title = { Text(stringResource(R.string.mismatch_title, card.name)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.mismatch_body, card.name))
                if (canPair) {
                    Text(stringResource(R.string.mismatch_pair_hint))
                }
            }
        },
        confirmButton = {
            if (canPair) {
                QdpButton(onClick = onPair, emphasis = Emphasis.LOWEST) {
                    Text(stringResource(R.string.action_pair_as_new))
                }
            }
        },
        dismissButton = {
            val close = rememberInitialFocus()
            QdpButton(onClick = onDismiss, emphasis = Emphasis.LOWEST, modifier = Modifier.focusRequester(close)) {
                Text(stringResource(R.string.action_close))
            }
        },
    )
}

/** A dialog that only informs: the drawable [icon], [title], [content], and Close. */
@Composable
private fun InfoDialog(
    @DrawableRes icon: Int,
    title: String,
    onDismiss: () -> Unit,
    content: @Composable () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.gamepadButtons(),
        icon = { Icon(painterResource(icon), contentDescription = null) },
        title = { Text(title) },
        text = {
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                content()
            }
        },
        confirmButton = {
            val close = rememberInitialFocus()
            QdpButton(onClick = onDismiss, emphasis = Emphasis.LOWEST, modifier = Modifier.focusRequester(close)) {
                Text(stringResource(R.string.action_close))
            }
        },
    )
}

/** Setup help's step [number], which [text] describes. */
@Composable
private fun SetupStep(number: Int, @StringRes text: Int) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(
            modifier = Modifier
                .size(STEP_NUMBER_SIZE)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.primaryContainer),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = number.toString(),
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onPrimaryContainer,
            )
        }
        Text(stringResource(text), style = MaterialTheme.typography.bodyMedium)
    }
}
