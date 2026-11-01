package io.github.ianmarmour.qdp.ui.components

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.input.InputMode
import androidx.compose.ui.platform.LocalInputModeManager

/**
 * Returns whether keys drive the screen: a keyboard or D-pad was last used,
 * or a gamepad is connected. Only then does focus start anywhere, so that
 * touch users see no stray focus ring.
 */
@Composable
fun rememberKeyDriven(): Boolean {
    val keyboard = LocalInputModeManager.current.inputMode == InputMode.Keyboard
    val gamepad = rememberGamepadConnected()
    return keyboard || gamepad
}

/**
 * Returns a requester that focuses what it is attached to once that
 * appears, when [rememberKeyDriven] says keys drive the screen: dialogs and
 * menus open on their safe action this way. Call it where the item is
 * composed, inside a dialog's own content, so that the request comes once
 * the item exists.
 */
@Composable
fun rememberInitialFocus(): FocusRequester {
    val requester = remember { FocusRequester() }
    val keyDriven = rememberKeyDriven()
    LaunchedEffect(requester) {
        if (keyDriven) {
            requester.requestFocus()
        }
    }
    return requester
}
