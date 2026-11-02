package io.github.ianmarmour.qdp.ui.home

import androidx.annotation.StringRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.collapse
import androidx.compose.ui.semantics.expand
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.Gateway
import io.github.ianmarmour.qdp.Native
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.Settings
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.QdpButton
import io.github.ianmarmour.qdp.ui.components.focusRing
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import io.github.ianmarmour.qdp.ui.settings.SwitchRow

/** The turn of the Advanced section's chevron while it is open. */
private const val CHEVRON_OPEN_DEGREES = 180f

/** Which of the dialog's fields shows an error. */
private enum class FieldError {
    /** Every field is fine, or not yet checked. */
    NONE,

    /** The address is empty or has spaces. */
    ADDRESS,

    /** The key pin is not one. */
    PIN,

    /** The token is empty. */
    TOKEN,
}

/**
 * Adds a computer by its address, filled in from the last one used or a
 * connection link: Pair answers [onPair] with the address; the collapsed
 * Advanced section connects with the host's key pin and a token through
 * [onConnectWithToken], and starts open when [advancedOpen]. With the
 * organization's gateway set, the computer is reached through it by
 * default, by its ID there or its name inside the network, answering
 * [onAddThroughGateway].
 */
@Composable
fun AddHostDialog(
    advancedOpen: Boolean,
    onDismiss: () -> Unit,
    onPair: (address: String) -> Unit,
    onConnectWithToken: (address: String, pin: String, token: String) -> Unit,
    onAddThroughGateway: (target: String) -> Unit,
) {
    val context = LocalContext.current
    val stored = remember { Settings.load(context) }
    val gateway = remember { Gateway.load(context) }
    var throughGateway by rememberSaveable { mutableStateOf(gateway != null) }
    var address by rememberSaveable { mutableStateOf(stored.host) }
    var pin by rememberSaveable { mutableStateOf(stored.pin) }
    var token by rememberSaveable { mutableStateOf(stored.token) }
    var advanced by rememberSaveable { mutableStateOf(advancedOpen) }
    var error by rememberSaveable { mutableStateOf(FieldError.NONE) }
    val addressFocus = remember { FocusRequester() }

    fun pair() {
        val trimmed = address.trim()
        if (!isAddress(trimmed)) {
            error = FieldError.ADDRESS
            return
        }
        if (throughGateway) onAddThroughGateway(trimmed) else onPair(trimmed)
    }

    fun connectWithToken() {
        val trimmed = address.trim()
        val trimmedPin = pin.trim().lowercase()
        error = when {
            !isAddress(trimmed) -> FieldError.ADDRESS
            !Native.isPin(trimmedPin) -> FieldError.PIN
            token.isEmpty() -> FieldError.TOKEN
            else -> FieldError.NONE
        }
        if (error == FieldError.NONE) {
            onConnectWithToken(trimmed, trimmedPin, token)
        }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        modifier = Modifier.gamepadButtons(),
        title = { Text(stringResource(R.string.action_add_by_address)) },
        text = {
            // Inside the dialog's window, where the field is.
            LaunchedEffect(addressFocus) { addressFocus.requestFocus() }
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(stringResource(if (throughGateway) R.string.add_gateway_body else R.string.add_body))
                OutlinedTextField(
                    value = address,
                    onValueChange = {
                        address = it
                        if (error == FieldError.ADDRESS) error = FieldError.NONE
                    },
                    modifier = Modifier.fillMaxWidth().focusRequester(addressFocus),
                    label = { Text(stringResource(R.string.add_address_label)) },
                    placeholder = { Text(stringResource(R.string.add_address_hint)) },
                    isError = error == FieldError.ADDRESS,
                    supportingText = errorText(error == FieldError.ADDRESS, R.string.add_address_error),
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { pair() }),
                )
                if (gateway != null) {
                    SwitchRow(
                        title = stringResource(R.string.add_through_gateway, gateway.address),
                        supporting = stringResource(R.string.add_through_gateway_supporting),
                        checked = throughGateway,
                        onCheckedChange = { throughGateway = it },
                    )
                }
                if (!throughGateway) {
                    AdvancedHeader(advanced) { advanced = !advanced }
                }
                AnimatedVisibility(visible = advanced && !throughGateway) {
                    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(
                            text = stringResource(R.string.add_advanced_body),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        OutlinedTextField(
                            value = pin,
                            onValueChange = {
                                pin = it
                                if (error == FieldError.PIN) error = FieldError.NONE
                            },
                            modifier = Modifier.fillMaxWidth(),
                            label = { Text(stringResource(R.string.add_pin_label)) },
                            isError = error == FieldError.PIN,
                            supportingText = errorText(error == FieldError.PIN, R.string.add_pin_error),
                            textStyle = TextStyle(fontFamily = FontFamily.Monospace),
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Next),
                        )
                        OutlinedTextField(
                            value = token,
                            onValueChange = {
                                token = it
                                if (error == FieldError.TOKEN) error = FieldError.NONE
                            },
                            modifier = Modifier.fillMaxWidth(),
                            label = { Text(stringResource(R.string.add_token_label)) },
                            isError = error == FieldError.TOKEN,
                            supportingText = errorText(error == FieldError.TOKEN, R.string.add_token_error),
                            visualTransformation = PasswordVisualTransformation(),
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(
                                keyboardType = KeyboardType.Password,
                                imeAction = ImeAction.Done,
                            ),
                            keyboardActions = KeyboardActions(onDone = { connectWithToken() }),
                        )
                        QdpButton(onClick = ::connectWithToken, emphasis = Emphasis.MEDIUM) {
                            Text(stringResource(R.string.action_connect_with_token))
                        }
                    }
                }
            }
        },
        confirmButton = {
            QdpButton(onClick = ::pair, emphasis = Emphasis.HIGH) {
                Text(stringResource(if (throughGateway) R.string.action_connect else R.string.action_pair))
            }
        },
        dismissButton = {
            QdpButton(onClick = onDismiss, emphasis = Emphasis.LOWEST) { Text(stringResource(R.string.action_cancel)) }
        },
    )
}

/** The Advanced section's header, which opens or closes it; [open] turns its chevron. */
@Composable
private fun AdvancedHeader(open: Boolean, onToggle: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val shape = MaterialTheme.shapes.small
    val turn by animateFloatAsState(if (open) CHEVRON_OPEN_DEGREES else 0f, label = "chevron")
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .focusRing(interaction, shape)
            .clickable(interactionSource = interaction, indication = ripple(), role = Role.Button, onClick = onToggle)
            .semantics {
                heading()
                // TalkBack reads these as the section's expanded state.
                if (open) {
                    collapse { onToggle(); true }
                } else {
                    expand { onToggle(); true }
                }
            }
            .padding(vertical = 12.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(stringResource(R.string.add_advanced), style = MaterialTheme.typography.titleSmall)
        Icon(painterResource(R.drawable.ic_expand_more), contentDescription = null, modifier = Modifier.rotate(turn))
    }
}

/** Returns a field's supporting text: [message] while [shown], nothing otherwise. */
private fun errorText(shown: Boolean, @StringRes message: Int): (@Composable () -> Unit)? =
    if (shown) {
        { Text(stringResource(message)) }
    } else {
        null
    }

/** Returns whether [text] could be an address: a name or IP address, maybe with a port, and no spaces. */
private fun isAddress(text: String): Boolean = text.isNotEmpty() && text.none { it.isWhitespace() }
