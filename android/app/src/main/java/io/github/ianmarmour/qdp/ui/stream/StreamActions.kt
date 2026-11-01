package io.github.ianmarmour.qdp.ui.stream

/** What the stream's overlay asks its activity to do. */
class StreamActions(
    /** Closes the menu and goes back to the video. */
    val resume: () -> Unit,
    /** Closes the menu and shows the on-screen keyboard. */
    val showKeyboard: () -> Unit,
    /** Switches between trackpad and direct touch. */
    val toggleTouchMode: () -> Unit,
    /** Shows more, less or no statistics. */
    val cycleStats: () -> Unit,
    /** Ends the session and leaves the stream. */
    val disconnect: () -> Unit,
    /** Answers the host's request for a pairing code with the one entered. */
    val submitCode: (String) -> Unit,
    /** Declines to pair, which ends the session. */
    val cancelPairing: () -> Unit,
    /** Connects again after a failure. */
    val tryAgain: () -> Unit,
    /** Leaves the stream after a failure. */
    val leave: () -> Unit,
)
