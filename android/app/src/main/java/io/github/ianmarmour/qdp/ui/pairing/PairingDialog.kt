package io.github.ianmarmour.qdp.ui.pairing

import android.content.pm.PackageManager
import android.view.KeyEvent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.clearText
import androidx.compose.foundation.text.input.delete
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.GamepadButton
import io.github.ianmarmour.qdp.ui.components.HideSystemBarsInDialog
import io.github.ianmarmour.qdp.ui.components.QdpButton
import io.github.ianmarmour.qdp.ui.components.Shake
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import io.github.ianmarmour.qdp.ui.components.rememberGamepadConnected
import io.github.ianmarmour.qdp.ui.components.rememberInitialFocus
import io.github.ianmarmour.qdp.ui.components.rememberShake
import io.github.ianmarmour.qdp.ui.components.shake
import io.github.ianmarmour.qdp.ui.stream.PairingStep
import io.github.ianmarmour.qdp.ui.stream.PairingUi

/** The widest the dialog grows when the keypad sits beside the code. */
private val SIDE_BY_SIDE_MAX_WIDTH = 640.dp

/** The widest the dialog grows otherwise. */
private val STACKED_MAX_WIDTH = 420.dp

/** The space kept between the dialog and the screen's edges. */
private val SCREEN_MARGIN = 24.dp

/** The size of the "Checking…" spinner. */
private val CHECKING_INDICATOR_SIZE = 16.dp

/** The width of the "Checking…" spinner's stroke. */
private val CHECKING_INDICATOR_STROKE = 2.dp

/** The height kept for the status line, so that the dialog does not jump as it changes. */
private val STATUS_MIN_HEIGHT = 24.dp

/** How the dialog lays out its code and keypad. */
enum class PairingLayout {
    /** One above the other. */
    STACKED,

    /** Side by side, on landscape screens too short for both stacked. */
    SIDE_BY_SIDE,
}

/** How the code is entered. */
private enum class CodeEntry {
    /** With the keyboard, which the focused field raises. */
    KEYBOARD,

    /** With the on-screen keypad; the field takes no focus. */
    KEYPAD,
}

/**
 * Asks for the code that [hostName] shows to pair with it: "Pair with
 * Desk-PC", a four-cell code field that submits itself through [onSubmit]
 * on the fourth digit and then says "Checking…", and the name the computer
 * will know this device by. A wrong code shakes and clears the field.
 * Failing, it shows why with Try again and Close.
 *
 * With a gamepad, or without a touchscreen, a keypad replaces the keyboard:
 * A types the focused key, X deletes and B cancels. It is a dialog rather
 * than a sheet, so that focus stays inside and it fits a short landscape
 * screen with the keyboard up.
 */
@Composable
fun PairingDialog(
    hostName: String,
    deviceName: String,
    pairing: PairingUi,
    layout: PairingLayout,
    onSubmit: (String) -> Unit,
    onCancel: () -> Unit,
    onTryAgain: () -> Unit,
    onClose: () -> Unit,
) {
    val failure = pairing.failure
    val sideBySide = layout == PairingLayout.SIDE_BY_SIDE
    Dialog(
        onDismissRequest = if (pairing.step == PairingStep.FAILED) onClose else onCancel,
        properties = DialogProperties(dismissOnClickOutside = false, usePlatformDefaultWidth = false),
    ) {
        HideSystemBarsInDialog()
        Surface(
            shape = MaterialTheme.shapes.extraLarge,
            color = MaterialTheme.colorScheme.surfaceContainerHigh,
            modifier = Modifier
                .padding(SCREEN_MARGIN)
                .widthIn(max = if (sideBySide) SIDE_BY_SIDE_MAX_WIDTH else STACKED_MAX_WIDTH),
        ) {
            if (pairing.step == PairingStep.FAILED && failure != null) {
                Failed(
                    title = stringResource(failure.kind.title, hostName),
                    body = stringResource(failure.kind.body),
                    onTryAgain = onTryAgain,
                    onClose = onClose,
                )
            } else {
                Entry(hostName, deviceName, pairing, sideBySide, onSubmit, onCancel)
            }
        }
    }
}

/** The dialog while the code is entered and checked. */
@Composable
private fun Entry(
    hostName: String,
    deviceName: String,
    pairing: PairingUi,
    sideBySide: Boolean,
    onSubmit: (String) -> Unit,
    onCancel: () -> Unit,
) {
    val code = rememberTextFieldState()
    val touchscreen = LocalContext.current.packageManager.hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN)
    val entry = if (rememberGamepadConnected() || !touchscreen) CodeEntry.KEYPAD else CodeEntry.KEYBOARD
    val entering = pairing.step == PairingStep.ENTERING
    val field = remember { FocusRequester() }
    val firstKey = remember { FocusRequester() }
    val shake = rememberShake(pairing.wrongCodes)
    val submit by rememberUpdatedState(onSubmit)

    // Each request for a code starts empty, and the fourth digit sends it.
    LaunchedEffect(pairing.step, pairing.wrongCodes) {
        if (!entering) {
            return@LaunchedEffect
        }
        code.clearText()
        snapshotFlow { code.text.toString() }.collect { typed ->
            if (typed.length == CODE_LENGTH) {
                submit(typed)
            }
        }
    }
    LaunchedEffect(entry) {
        withFrameNanos {}
        if (entry == CodeEntry.KEYPAD) firstKey.requestFocus() else field.requestFocus()
    }

    fun typeDigit(digit: Char) {
        if (entering && code.text.length < CODE_LENGTH) {
            code.edit { append(digit) }
        }
    }

    fun deleteDigit() {
        if (entering && code.text.isNotEmpty()) {
            code.edit { delete(length - 1, length) }
        }
    }

    Column(
        modifier = Modifier
            .gamepadButtons { button -> if (button == GamepadButton.X) deleteDigit() }
            .onPreviewKeyEvent { event ->
                entry == CodeEntry.KEYPAD && keypadKey(event.nativeKeyEvent, ::typeDigit, ::deleteDigit)
            }
            .verticalScroll(rememberScrollState())
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        val keypad: @Composable () -> Unit = {
            Keypad(onDigit = ::typeDigit, onDelete = ::deleteDigit, firstKey = firstKey)
        }
        if (entry == CodeEntry.KEYPAD && sideBySide) {
            Row(horizontalArrangement = Arrangement.spacedBy(24.dp), verticalAlignment = Alignment.CenterVertically) {
                Prompt(hostName, deviceName, pairing, code, entry, field, shake, Modifier.weight(1f))
                keypad()
            }
        } else {
            Prompt(hostName, deviceName, pairing, code, entry, field, shake, Modifier)
            if (entry == CodeEntry.KEYPAD) {
                Row(modifier = Modifier.align(Alignment.CenterHorizontally)) { keypad() }
            }
        }
        Row(modifier = Modifier.align(Alignment.End)) {
            QdpButton(onClick = onCancel, emphasis = Emphasis.LOWEST) { Text(stringResource(R.string.action_cancel)) }
        }
    }
}

/** The title, instructions, code field, status line and footnote. */
@Composable
private fun Prompt(
    hostName: String,
    deviceName: String,
    pairing: PairingUi,
    code: TextFieldState,
    entry: CodeEntry,
    field: FocusRequester,
    shake: Shake,
    modifier: Modifier,
) {
    Column(modifier = modifier, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(
            text = stringResource(R.string.pair_title, hostName),
            style = MaterialTheme.typography.headlineSmall,
            modifier = Modifier.semantics { heading() },
        )
        Text(stringResource(R.string.pair_body, hostName), style = MaterialTheme.typography.bodyMedium)
        CodeField(
            state = code,
            acceptsTyping = entry == CodeEntry.KEYBOARD,
            modifier = Modifier.shake(shake).focusRequester(field),
        )
        Status(hostName, pairing)
        Text(
            text = stringResource(R.string.pair_footnote, hostName, deviceName),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** "Checking…" while the code is checked, or why the last code failed, announced as it changes. */
@Composable
private fun Status(hostName: String, pairing: PairingUi) {
    Row(
        modifier = Modifier.heightIn(min = STATUS_MIN_HEIGHT).semantics { liveRegion = LiveRegionMode.Polite },
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        when {
            pairing.step == PairingStep.CHECKING -> {
                CircularProgressIndicator(
                    modifier = Modifier.size(CHECKING_INDICATOR_SIZE),
                    strokeWidth = CHECKING_INDICATOR_STROKE,
                )
                Text(stringResource(R.string.pair_checking), style = MaterialTheme.typography.labelLarge)
            }
            pairing.wrongCodes > 0 -> {
                Icon(
                    painter = painterResource(R.drawable.ic_warning),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.error,
                )
                Text(
                    text = stringResource(R.string.pair_wrong_code, hostName),
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.error,
                )
            }
        }
    }
}

/** The dialog after pairing failed: why, with Try again (focused first) and Close. */
@Composable
private fun Failed(title: String, body: String, onTryAgain: () -> Unit, onClose: () -> Unit) {
    val tryAgain = rememberInitialFocus()
    Column(
        modifier = Modifier.gamepadButtons().verticalScroll(rememberScrollState()).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Icon(
            painter = painterResource(R.drawable.ic_warning),
            contentDescription = null,
            tint = MaterialTheme.colorScheme.error,
            modifier = Modifier.align(Alignment.CenterHorizontally),
        )
        Text(title, style = MaterialTheme.typography.headlineSmall, modifier = Modifier.semantics { heading() })
        Text(body, style = MaterialTheme.typography.bodyMedium)
        Row(modifier = Modifier.align(Alignment.End), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            QdpButton(onClick = onClose, emphasis = Emphasis.LOWEST) { Text(stringResource(R.string.action_close)) }
            QdpButton(onClick = onTryAgain, emphasis = Emphasis.HIGH, modifier = Modifier.focusRequester(tryAgain)) {
                Text(stringResource(R.string.action_try_again))
            }
        }
    }
}

/**
 * Types a digit key or deletes with Backspace from a keyboard while the
 * keypad, rather than the field, has focus, returning whether [key] was
 * one of those.
 */
private fun keypadKey(key: KeyEvent, type: (Char) -> Unit, delete: () -> Unit): Boolean {
    val digit = when (key.keyCode) {
        in KeyEvent.KEYCODE_0..KeyEvent.KEYCODE_9 -> '0' + (key.keyCode - KeyEvent.KEYCODE_0)
        in KeyEvent.KEYCODE_NUMPAD_0..KeyEvent.KEYCODE_NUMPAD_9 -> '0' + (key.keyCode - KeyEvent.KEYCODE_NUMPAD_0)
        KeyEvent.KEYCODE_DEL -> null
        else -> return false
    }
    if (key.action == KeyEvent.ACTION_DOWN) {
        if (digit != null) type(digit) else delete()
    }
    return true
}
