package io.github.ianmarmour.qdp.ui.components

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.IndicationNodeFactory
import androidx.compose.foundation.indication
import androidx.compose.foundation.interaction.FocusInteraction
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.node.CompositionLocalConsumerModifierNode
import androidx.compose.ui.node.DelegatableNode
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.node.currentValueOf
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.ui.theme.LocalFocusRingColor
import kotlinx.coroutines.launch

/** The focus ring's width. */
private val RING_WIDTH = 3.dp

/** The space between an item and its focus ring. */
private val RING_GAP = 2.dp

/** How long the ring takes to appear or go. */
private const val RING_ANIMATION_MILLIS = 120

/** How much larger a focused item draws. */
enum class FocusGrowth(val scale: Float) {
    /** A little larger, within the 1.03–1.05 TV focus guidance: cards, tiles and buttons. */
    GROW(1.04f),

    /** The same size: full-width rows, which would spill off the screen. */
    NONE(1f),
}

/**
 * Marks the item that has focus with a 3dp outline in the theme's primary
 * color around [shape], and draws the item as [growth] says, so that a
 * gamepad, D-pad or keyboard user sees where they are: Material's own focus
 * state layer is too faint on a handheld or a TV.
 */
class FocusRingIndication(private val shape: Shape, private val growth: FocusGrowth) : IndicationNodeFactory {
    override fun create(interactionSource: InteractionSource): DelegatableNode =
        FocusRingNode(interactionSource, shape, growth)

    override fun equals(other: Any?): Boolean =
        other is FocusRingIndication && other.shape == shape && other.growth == growth

    override fun hashCode(): Int = 31 * shape.hashCode() + growth.hashCode()
}

/** Draws [FocusRingIndication] while [interactionSource] reports focus. */
private class FocusRingNode(
    private val interactionSource: InteractionSource,
    private val shape: Shape,
    private val growth: FocusGrowth,
) : Modifier.Node(), DrawModifierNode, CompositionLocalConsumerModifierNode {
    /** How far the ring has appeared, from 0 to 1. */
    private val shown = Animatable(0f)

    override fun onAttach() {
        coroutineScope.launch {
            var focused = 0
            interactionSource.interactions.collect { interaction ->
                when (interaction) {
                    is FocusInteraction.Focus -> focused++
                    is FocusInteraction.Unfocus -> focused--
                    else -> return@collect
                }
                launch { shown.animateTo(if (focused > 0) 1f else 0f, tween(RING_ANIMATION_MILLIS)) }
            }
        }
    }

    override fun ContentDrawScope.draw() {
        val amount = shown.value
        val color = currentValueOf(LocalFocusRingColor)
        if (amount == 0f || color == Color.Unspecified) {
            drawContent()
            return
        }
        scale(1f + (growth.scale - 1f) * amount) {
            this@draw.drawContent()
            val stroke = RING_WIDTH.toPx()
            val inset = RING_GAP.toPx() + stroke / 2
            val ring = Size(size.width + 2 * inset, size.height + 2 * inset)
            translate(-inset, -inset) {
                drawOutline(
                    shape.createOutline(ring, layoutDirection, this),
                    color.copy(alpha = amount),
                    style = Stroke(stroke),
                )
            }
        }
    }
}

/**
 * Draws [FocusRingIndication] around [shape], growing as [growth] says,
 * while the item that [interactionSource] belongs to has focus. Give the
 * item the same source, and put this before its own modifiers so that the
 * ring is not clipped.
 */
fun Modifier.focusRing(
    interactionSource: InteractionSource,
    shape: Shape,
    growth: FocusGrowth = FocusGrowth.GROW,
): Modifier = indication(interactionSource, FocusRingIndication(shape, growth))
