package io.github.ianmarmour.qdp.ui.stream

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.adaptive.currentWindowAdaptiveInfo
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.window.core.layout.WindowSizeClass
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.ui.components.rememberGamepadConnected
import io.github.ianmarmour.qdp.ui.gateway.GatewayCodeDialog
import io.github.ianmarmour.qdp.ui.gateway.GatewaySignInDialog
import io.github.ianmarmour.qdp.ui.pairing.PairingDialog
import io.github.ianmarmour.qdp.ui.pairing.PairingLayout
import kotlinx.coroutines.delay

/** How long the tip teaching the menu shows. */
private const val COACHMARK_MILLIS = 8_000L

/**
 * Everything drawn over the stream's video, always in dark theme: the
 * connecting screen, the reconnecting banner, the statistics pill inside
 * the display cutout, the first stream's menu tip, the menu, messages, and
 * the pairing, gateway sign-in and error dialogs.
 */
@Composable
fun StreamOverlay(ui: StreamUiState, actions: StreamActions, gatewayUser: String = "") {
    val gamepad = rememberGamepadConnected()
    val snackbar = remember { SnackbarHostState() }
    val compactHeight = !currentWindowAdaptiveInfo().windowSizeClass
        .isHeightAtLeastBreakpoint(WindowSizeClass.HEIGHT_DP_MEDIUM_LOWER_BOUND)
    val pairedMessage = stringResource(R.string.stream_paired, ui.hostName)
    val notice = ui.notice
    LaunchedEffect(notice) {
        if (notice == StreamNotice.PAIRED) {
            snackbar.showSnackbar(pairedMessage)
            ui.notice = null
        }
    }
    LaunchedEffect(ui.coachmark) {
        if (ui.coachmark == Visibility.SHOWN) {
            delay(COACHMARK_MILLIS)
            ui.coachmark = Visibility.HIDDEN
        }
    }

    Box(modifier = Modifier.fillMaxSize()) {
        val stats = ui.stats
        if (stats != null && ui.statsLevel != StatsLevel.HIDDEN && ui.phase != StreamPhase.CONNECTING) {
            StatsPill(
                stats = stats,
                level = ui.statsLevel,
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .windowInsetsPadding(WindowInsets.displayCutout)
                    .padding(12.dp),
            )
        }
        AnimatedVisibility(
            visible = ui.phase == StreamPhase.RECONNECTING,
            modifier = Modifier.align(Alignment.TopCenter),
            enter = slideInVertically { -it } + fadeIn(),
            exit = slideOutVertically { -it } + fadeOut(),
        ) {
            ReconnectingBanner(ui.reconnectAt, actions.disconnect)
        }
        // At the top, where the reconnecting banner shows only while the tip
        // does not, so that the tip clears the "Paired" message below.
        AnimatedVisibility(
            visible = ui.coachmark == Visibility.SHOWN && ui.phase == StreamPhase.LIVE,
            modifier = Modifier.align(Alignment.TopCenter),
            enter = fadeIn(),
            exit = fadeOut(),
        ) {
            MenuCoachmark(gamepad)
        }
        AnimatedVisibility(visible = ui.phase == StreamPhase.CONNECTING, enter = fadeIn(), exit = fadeOut()) {
            ConnectingScreen(ui.hostName, ui.connectingStep, actions.disconnect)
        }
        StreamMenu(ui, actions)
        SnackbarHost(
            hostState = snackbar,
            modifier = Modifier.align(Alignment.BottomCenter).windowInsetsPadding(WindowInsets.safeDrawing),
        )
    }

    ui.pairing?.let { pairing ->
        PairingDialog(
            hostName = ui.hostName,
            deviceName = ui.deviceName,
            pairing = pairing,
            layout = if (compactHeight) PairingLayout.SIDE_BY_SIDE else PairingLayout.STACKED,
            onSubmit = actions.submitCode,
            onCancel = actions.cancelPairing,
            onTryAgain = actions.tryAgain,
            onClose = actions.leave,
        )
    }
    ui.gatewaySignIn?.let { signIn ->
        if (signIn.code) {
            GatewayCodeDialog(
                gateway = signIn.gateway,
                retry = signIn.retry,
                enrollment = signIn.enrollment,
                onSubmit = actions.submitSignInCode,
                onCancel = actions.cancelSignIn,
            )
        } else {
            GatewaySignInDialog(
                gateway = signIn.gateway,
                retry = signIn.retry,
                user = gatewayUser,
                onSubmit = actions.submitSignIn,
                onCancel = actions.cancelSignIn,
            )
        }
    }
    ui.error?.let { error -> ErrorDialog(error, ui.hostName, actions.tryAgain, actions.leave) }
}
