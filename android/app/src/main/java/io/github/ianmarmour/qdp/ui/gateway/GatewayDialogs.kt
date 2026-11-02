package io.github.ianmarmour.qdp.ui.gateway

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.Native
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.HideSystemBarsInDialog
import io.github.ianmarmour.qdp.ui.components.QdpButton

/** The digits of a gateway's TOTP code (Section 4.4.10 of the specification). */
private const val CODE_DIGITS = 6

/** The light modules around a QR code that readers need to find it. */
private const val QUIET_ZONE = 4

/** The QR code's side on the screen, large enough for another phone's camera. */
private val QR_SIDE = 220.dp

/**
 * Asks for the user's name, filled in with [user], and password for the
 * gateway named [gateway], saying that the last attempt failed if [retry]:
 * Sign in answers [onSubmit], Cancel [onCancel], which ends the session.
 */
@Composable
fun GatewaySignInDialog(
    gateway: String,
    retry: Boolean,
    user: String,
    onSubmit: (user: String, password: String) -> Unit,
    onCancel: () -> Unit,
) {
    var name by rememberSaveable { mutableStateOf(user) }
    var password by rememberSaveable { mutableStateOf("") }
    val passwordFocus = remember { FocusRequester() }
    val nameFocus = remember { FocusRequester() }
    val ready = name.isNotBlank() && password.isNotEmpty()

    fun submit() {
        if (ready) {
            onSubmit(name.trim(), password)
        }
    }

    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text(stringResource(R.string.gateway_sign_in_title, gateway)) },
        text = {
            HideSystemBarsInDialog()
            LaunchedEffect(Unit) { if (user.isBlank()) nameFocus.requestFocus() else passwordFocus.requestFocus() }
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(stringResource(if (retry) R.string.gateway_sign_in_retry else R.string.gateway_sign_in_body))
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    modifier = Modifier.fillMaxWidth().focusRequester(nameFocus),
                    label = { Text(stringResource(R.string.gateway_user_label)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email, imeAction = ImeAction.Next),
                )
                OutlinedTextField(
                    value = password,
                    onValueChange = { password = it },
                    modifier = Modifier.fillMaxWidth().focusRequester(passwordFocus),
                    label = { Text(stringResource(R.string.gateway_password_label)) },
                    visualTransformation = PasswordVisualTransformation(),
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { submit() }),
                )
            }
        },
        confirmButton = {
            QdpButton(onClick = ::submit, emphasis = Emphasis.HIGH, enabled = ready) {
                Text(stringResource(R.string.action_sign_in))
            }
        },
        dismissButton = {
            QdpButton(onClick = onCancel, emphasis = Emphasis.LOWEST) { Text(stringResource(R.string.action_cancel)) }
        },
    )
}

/**
 * Asks for a code from the user's authenticator app for the gateway named
 * [gateway], saying that the last code did not work if [retry]. With an
 * [enrollment], an `otpauth://` URI, it first offers to add the gateway to
 * an authenticator app on this device, and shows the URI's QR code for one
 * on another, with a button that copies it. Sign in answers [onSubmit],
 * Cancel [onCancel], which ends the session.
 */
@Composable
fun GatewayCodeDialog(
    gateway: String,
    retry: Boolean,
    enrollment: String?,
    onSubmit: (code: String) -> Unit,
    onCancel: () -> Unit,
) {
    val context = LocalContext.current
    var code by rememberSaveable { mutableStateOf("") }
    val codeFocus = remember { FocusRequester() }
    val ready = code.length == CODE_DIGITS && code.all(Char::isDigit)

    fun submit() {
        if (ready) {
            onSubmit(code)
        }
    }

    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text(stringResource(R.string.gateway_sign_in_title, gateway)) },
        text = {
            HideSystemBarsInDialog()
            LaunchedEffect(Unit) { codeFocus.requestFocus() }
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                val body = when {
                    enrollment != null && retry -> R.string.gateway_enroll_retry
                    enrollment != null -> R.string.gateway_enroll_body
                    retry -> R.string.gateway_code_retry
                    else -> R.string.gateway_code_body
                }
                Text(stringResource(body))
                if (enrollment != null) {
                    QdpButton(onClick = { openAuthenticator(context, enrollment) }, emphasis = Emphasis.MEDIUM) {
                        Text(stringResource(R.string.gateway_enroll_open))
                    }
                    QrCode(enrollment, stringResource(R.string.gateway_enroll_qr_description))
                    QdpButton(onClick = { copyLink(context, enrollment) }, emphasis = Emphasis.LOWEST) {
                        Text(stringResource(R.string.gateway_enroll_copy))
                    }
                }
                OutlinedTextField(
                    value = code,
                    onValueChange = { typed -> code = typed.filter(Char::isDigit).take(CODE_DIGITS) },
                    modifier = Modifier.fillMaxWidth().focusRequester(codeFocus),
                    label = { Text(stringResource(R.string.gateway_code_label)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { submit() }),
                )
            }
        },
        confirmButton = {
            QdpButton(onClick = ::submit, emphasis = Emphasis.HIGH, enabled = ready) {
                Text(stringResource(R.string.action_sign_in))
            }
        },
        dismissButton = {
            QdpButton(onClick = onCancel, emphasis = Emphasis.LOWEST) { Text(stringResource(R.string.action_cancel)) }
        },
    )
}

/** Draws the QR code of [text], dark modules on white with its quiet zone, described as [description]. */
@Composable
private fun QrCode(text: String, description: String) {
    val modules = remember(text) { Native.qrModules(text) } ?: return
    val side = modules[0]
    val total = side + 2 * QUIET_ZONE
    Canvas(Modifier.size(QR_SIDE).semantics { contentDescription = description }) {
        drawRect(Color.White)
        val cell = size.width / total
        for (y in 0 until side) {
            for (x in 0 until side) {
                if (modules[1 + y * side + x] == 1) {
                    drawRect(
                        Color.Black,
                        topLeft = Offset((x + QUIET_ZONE) * cell, (y + QUIET_ZONE) * cell),
                        size = Size(cell, cell),
                    )
                }
            }
        }
    }
}

/** Hands the `otpauth://` [uri] to the authenticator app on this device, or says there is none. */
private fun openAuthenticator(context: Context, uri: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(uri)))
    } catch (_: ActivityNotFoundException) {
        Toast.makeText(context, R.string.gateway_enroll_no_app, Toast.LENGTH_LONG).show()
    }
}

/** Copies the setup link [uri], for an authenticator that takes one pasted. */
private fun copyLink(context: Context, uri: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    clipboard.setPrimaryClip(ClipData.newPlainText(context.getString(R.string.gateway_enroll_copy), uri))
    Toast.makeText(context, R.string.gateway_enroll_copied, Toast.LENGTH_SHORT).show()
}
