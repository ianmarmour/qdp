package io.github.ianmarmour.qdp.ui.stream

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.StreamError
import io.github.ianmarmour.qdp.ui.components.Emphasis
import io.github.ianmarmour.qdp.ui.components.HideSystemBarsInDialog
import io.github.ianmarmour.qdp.ui.components.QdpButton
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import io.github.ianmarmour.qdp.ui.components.rememberInitialFocus

/**
 * Explains why the stream from [hostName] failed: a title naming the
 * problem and the computer, a sentence of cause and fix, and the core's own
 * words under "Show details"; then [onTryAgain] or [onBack]. It opens on
 * "Try again", and the warning icon says it is an error without relying on
 * color.
 */
@Composable
fun ErrorDialog(error: StreamError, hostName: String, onTryAgain: () -> Unit, onBack: () -> Unit) {
    var detailsShown by rememberSaveable { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onBack,
        modifier = Modifier.gamepadButtons(),
        // A stray tap beside it does not leave the stream; Back still does.
        properties = DialogProperties(dismissOnClickOutside = false),
        icon = { Icon(painterResource(R.drawable.ic_warning), contentDescription = null) },
        title = { Text(stringResource(error.kind.title, hostName)) },
        text = {
            HideSystemBarsInDialog()
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(stringResource(error.kind.body))
                QdpButton(onClick = { detailsShown = !detailsShown }, emphasis = Emphasis.LOWEST) {
                    Text(stringResource(if (detailsShown) R.string.error_hide_details else R.string.error_show_details))
                }
                AnimatedVisibility(visible = detailsShown) {
                    SelectionContainer {
                        Text(
                            text = error.details,
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = FontFamily.Monospace,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        },
        confirmButton = {
            val tryAgain = rememberInitialFocus()
            QdpButton(onClick = onTryAgain, emphasis = Emphasis.HIGH, modifier = Modifier.focusRequester(tryAgain)) {
                Text(stringResource(R.string.action_try_again))
            }
        },
        dismissButton = {
            QdpButton(onClick = onBack, emphasis = Emphasis.LOWEST) {
                Text(stringResource(R.string.action_back_to_computers))
            }
        },
    )
}
