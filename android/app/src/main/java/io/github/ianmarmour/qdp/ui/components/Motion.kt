package io.github.ianmarmour.qdp.ui.components

import android.animation.ValueAnimator
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.keyframes
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp

/** How long a shake takes. */
private const val SHAKE_MILLIS = 400

/** How far a shake moves, at its widest. */
private val SHAKE_DISTANCE = 12.dp

/** The share of the widest shake reached at each of its swings, in turn. */
private val SHAKE_SWINGS = listOf(-1f, 1f, -0.6f, 0.6f, -0.3f, 0f)

/**
 * Returns whether decorative animations may run: not when the user turned
 * animations off, which sets the animator duration scale to 0.
 */
fun animationsEnabled(): Boolean = ValueAnimator.areAnimatorsEnabled()

/** A single sideways shake, as when a code did not match; [shake] draws it. */
class Shake {
    /** The shake's offset, as a share of its widest. */
    internal val offset = Animatable(0f)

    /** Shakes once, unless animations are off. */
    internal suspend fun play() {
        if (!animationsEnabled()) {
            return
        }
        offset.animateTo(
            0f,
            keyframes {
                durationMillis = SHAKE_MILLIS
                SHAKE_SWINGS.forEachIndexed { index, swing ->
                    swing at SHAKE_MILLIS * (index + 1) / SHAKE_SWINGS.size
                }
            },
        )
    }
}

/** Returns a [Shake] that plays each time [trigger] changes, though not for its first value. */
@Composable
fun rememberShake(trigger: Int): Shake {
    val shake = remember { Shake() }
    val first = remember { trigger }
    LaunchedEffect(trigger) {
        if (trigger != first) {
            shake.play()
        }
    }
    return shake
}

/** Moves the content sideways as [shake] plays. */
fun Modifier.shake(shake: Shake): Modifier =
    graphicsLayer { translationX = shake.offset.value * SHAKE_DISTANCE.toPx() }
