package io.github.ianmarmour.qdp.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.Gateway
import io.github.ianmarmour.qdp.Native
import io.github.ianmarmour.qdp.R

/**
 * The Gateway section: the organization's gateway, by its address, the
 * user's name and, if its administrator gave one, its key pin, each kept
 * as it is typed. With an address set, computers added are reached through
 * it by default; clearing the address forgets the gateway.
 */
@Composable
fun GatewaySection() {
    val context = LocalContext.current
    val stored = remember { Gateway.load(context) }
    var address by rememberSaveable { mutableStateOf(stored?.address.orEmpty()) }
    var user by rememberSaveable { mutableStateOf(stored?.user.orEmpty()) }
    var pin by rememberSaveable { mutableStateOf(stored?.pin.orEmpty()) }
    val pinValid = pin.isBlank() || Native.isPin(pin.trim().lowercase())

    fun keep() {
        // A pin being typed is kept once it is one.
        val keptPin = if (pinValid) pin.trim().lowercase() else Gateway.load(context)?.pin.orEmpty()
        Gateway.save(context, Gateway(address, keptPin, user).takeIf { address.isNotBlank() })
    }

    SectionHeader(R.string.settings_gateway)
    Column(
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            text = stringResource(R.string.settings_gateway_body),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        OutlinedTextField(
            value = address,
            onValueChange = {
                address = it
                keep()
            },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.settings_gateway_address)) },
            placeholder = { Text(stringResource(R.string.settings_gateway_address_hint)) },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next),
        )
        OutlinedTextField(
            value = user,
            onValueChange = {
                user = it
                keep()
            },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.gateway_user_label)) },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email, imeAction = ImeAction.Next),
        )
        OutlinedTextField(
            value = pin,
            onValueChange = {
                pin = it
                keep()
            },
            modifier = Modifier.fillMaxWidth(),
            label = { Text(stringResource(R.string.settings_gateway_pin)) },
            isError = !pinValid,
            supportingText = pinError(!pinValid),
            textStyle = TextStyle(fontFamily = FontFamily.Monospace),
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Done),
        )
    }
}

/** Returns the pin field's supporting text: why it is not a pin while [shown], nothing otherwise. */
private fun pinError(shown: Boolean): (@Composable () -> Unit)? =
    if (shown) {
        { Text(stringResource(R.string.add_pin_error)) }
    } else {
        null
    }
