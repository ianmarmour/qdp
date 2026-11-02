package io.github.ianmarmour.qdp.ui.stream

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.ianmarmour.qdp.SessionEvent
import io.github.ianmarmour.qdp.StreamError
import io.github.ianmarmour.qdp.TouchMode
import io.github.ianmarmour.qdp.VirtualControl

/** Where the stream is. */
enum class StreamPhase {
    /** Connecting for the first time, before any video: the screen says so, with Cancel. */
    CONNECTING,

    /** Showing the host's video. */
    LIVE,

    /** The connection dropped and is being made again, over the last frame. */
    RECONNECTING,
}

/** What the connecting screen says is happening. */
enum class ConnectingStep {
    /** Reaching the host. */
    REACHING,

    /** Connected; waiting for the first frame. */
    STARTING,
}

/** How much of the statistics the stream shows. */
enum class StatsLevel {
    /** None. */
    HIDDEN,

    /** Frame rate and latency, in a corner pill. */
    SUMMARY,

    /** The pill, with where the second's slowest frame spent its time. */
    DETAILED;

    /** Returns the level after this one, as the menu's Stats tile steps through them. */
    fun next(): StatsLevel = entries[(ordinal + 1) % entries.size]
}

/** Whether a part of the overlay shows. */
enum class Visibility {
    /** Not shown. */
    HIDDEN,

    /** Shown. */
    SHOWN,
}

/** Where pairing stands while its dialog shows. */
enum class PairingStep {
    /** Waiting for the user to enter the code the host shows. */
    ENTERING,

    /** The code was sent; waiting for the host's answer. */
    CHECKING,

    /** Pairing failed and the session ended. */
    FAILED,
}

/**
 * The pairing dialog's state: its [step], how many codes did not match (so
 * that the field shakes and clears once for each), and why pairing failed.
 */
data class PairingUi(val step: PairingStep, val wrongCodes: Int = 0, val failure: StreamError? = null)

/**
 * What the gateway named [gateway] asks while the user signs in: their name
 * and password, or, with [code], a code from their authenticator app, which
 * [enrollment], an `otpauth://` URI, adds to it first if given; [retry]
 * after an answer that did not work.
 */
data class GatewaySignInUi(
    val gateway: String,
    val retry: Boolean,
    val code: Boolean,
    val enrollment: String? = null,
)

/** A short message the stream shows at its bottom. */
enum class StreamNotice {
    /** Pairing succeeded. */
    PAIRED,
}

/**
 * What the stream's overlay shows, which [io.github.ianmarmour.qdp.StreamActivity]
 * changes as the session goes and [StreamOverlay] draws over the video.
 */
class StreamUiState(val hostName: String, val deviceName: String, touchMode: TouchMode, statsLevel: StatsLevel) {
    /** Where the stream is. */
    var phase by mutableStateOf(StreamPhase.CONNECTING)

    /** What the connecting screen says. */
    var connectingStep by mutableStateOf(ConnectingStep.REACHING)

    /** When, in [android.os.SystemClock.uptimeMillis], the next reconnection attempt starts. */
    var reconnectAt by mutableLongStateOf(0L)

    /** Whether the menu shows. */
    var menu by mutableStateOf(Visibility.HIDDEN)

    /** How far a predictive back gesture has gone, from 0 to 1, while it closes the menu. */
    var backProgress by mutableFloatStateOf(0f)

    /** The last second's statistics, once there are any. */
    var stats by mutableStateOf<SessionEvent.Stats?>(null)

    /** How much of the statistics shows. */
    var statsLevel by mutableStateOf(statsLevel)

    /** What touching the screen does. */
    var touchMode by mutableStateOf(touchMode)

    /**
     * Whether the on-screen gamepad shows, taking the screen's touches,
     * which the menu turns on and off: off as each stream starts.
     */
    var controller by mutableStateOf(Visibility.HIDDEN)

    /** The on-screen gamepad's controls, as it is drawn. */
    var controllerControls by mutableStateOf<List<VirtualControl>>(emptyList())

    /** The pairing dialog, while pairing. */
    var pairing by mutableStateOf<PairingUi?>(null)

    /** The gateway's sign-in dialog, while it asks. */
    var gatewaySignIn by mutableStateOf<GatewaySignInUi?>(null)

    /** Why the stream failed, while that shows. */
    var error by mutableStateOf<StreamError?>(null)

    /** A message to show at the bottom, once. */
    var notice by mutableStateOf<StreamNotice?>(null)

    /** Whether the tip teaching how to open the menu shows. */
    var coachmark by mutableStateOf(Visibility.HIDDEN)

    /**
     * Whether touches and the mouse go to the overlay rather than the host:
     * while the menu is open and whenever the video is not live, so that its
     * buttons can be tapped.
     */
    val capturesTouch: Boolean
        get() = menu == Visibility.SHOWN || phase != StreamPhase.LIVE

    /**
     * Whether keys and gamepads go to the overlay rather than the host: while
     * the menu is open and while connecting. Not while reconnecting, so that
     * a gamepad press during a short drop cannot press Disconnect.
     */
    val capturesKeys: Boolean
        get() = menu == Visibility.SHOWN || phase == StreamPhase.CONNECTING
}
