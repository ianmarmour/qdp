package io.github.ianmarmour.qdp.ui.stream

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.VirtualControl

/** A control's fill at rest: faint, so that the picture shows through. */
private val FILL = Color.White.copy(alpha = 0.14f)

/** A control's fill while held. */
private val FILL_HELD = Color.White.copy(alpha = 0.42f)

/** A control's outline. */
private val OUTLINE = Color.White.copy(alpha = 0.55f)

/** Labels, arrows and glyphs. */
private val INK = Color.White.copy(alpha = 0.9f)

/** A stick's knob at rest. */
private val KNOB = Color.White.copy(alpha = 0.35f)

/** A stick's knob while held. */
private val KNOB_HELD = Color.White.copy(alpha = 0.7f)

/** The face buttons' letters, in the colors controllers print them in. */
private val FACE_COLORS = mapOf(
    VirtualControl.A to Color(0xFF7BD35F),
    VirtualControl.B to Color(0xFFFF6B61),
    VirtualControl.X to Color(0xFF5EA8FF),
    VirtualControl.Y to Color(0xFFFFD25E),
)

/** The labels of the buttons drawn with text. */
private val LABELS = mapOf(
    VirtualControl.A to "A",
    VirtualControl.B to "B",
    VirtualControl.X to "X",
    VirtualControl.Y to "Y",
    VirtualControl.LEFT_SHOULDER to "LB",
    VirtualControl.RIGHT_SHOULDER to "RB",
    VirtualControl.LEFT_TRIGGER to "LT",
    VirtualControl.RIGHT_TRIGGER to "RT",
    VirtualControl.LEFT_STICK_BUTTON to "L3",
    VirtualControl.RIGHT_STICK_BUTTON to "R3",
)

/** A stick's knob's radius, as a share of its base's. */
private const val KNOB_SHARE = 0.45f

/** A label's size, as a share of its button's height. */
private const val LABEL_SHARE = 0.42f

/** A glyph's size, as a share of its button's radius. */
private const val GLYPH_SHARE = 0.5f

/** A shoulder button's corners, as a share of its height. */
private const val SHOULDER_CORNER_SHARE = 0.35f

/** A directional pad arrow's size, as a share of an arm's width. */
private const val ARROW_SHARE = 0.3f

/**
 * The on-screen gamepad, drawn over the stream as the core lays it out:
 * faint controls the picture shows through, brighter where a thumb holds
 * them, the sticks' knobs following their tilt.
 */
@Composable
fun OnScreenController(controls: List<VirtualControl>) {
    val measurer = rememberTextMeasurer()
    val description = stringResource(R.string.controller_description)
    Canvas(modifier = Modifier.fillMaxSize().semantics { contentDescription = description }) {
        val outline = Stroke(width = 1.5.dp.toPx())
        for (control in controls) {
            val frame = Rect(control.left, control.top, control.left + control.width, control.top + control.height)
            when (control.code) {
                VirtualControl.LEFT_STICK, VirtualControl.RIGHT_STICK -> drawStick(frame, control, outline)
                VirtualControl.DPAD -> drawDPad(frame, control.directions, outline)
                VirtualControl.LEFT_SHOULDER,
                VirtualControl.RIGHT_SHOULDER,
                VirtualControl.LEFT_TRIGGER,
                VirtualControl.RIGHT_TRIGGER,
                -> drawShoulder(frame, control, outline, measurer)
                else -> drawRoundButton(frame, control, outline, measurer)
            }
        }
    }
}

/** Draws a stick: its base, and its knob where its tilt has it. */
private fun DrawScope.drawStick(frame: Rect, control: VirtualControl, outline: Stroke) {
    val radius = frame.width / 2
    drawCircle(FILL, radius, frame.center)
    drawCircle(OUTLINE, radius, frame.center, style = outline)
    val knob = radius * KNOB_SHARE
    val travel = radius - knob
    val at = frame.center + Offset(control.tiltX * travel, control.tiltY * travel)
    drawCircle(if (control.held) KNOB_HELD else KNOB, knob, at)
}

/** Draws the directional pad: a cross whose arms light as their directions are held. */
private fun DrawScope.drawDPad(frame: Rect, directions: Int, outline: Stroke) {
    val arm = frame.width / 3
    val center = frame.center
    val corner = CornerRadius(arm / 4)
    drawRect(FILL, Offset(center.x - arm / 2, center.y - arm / 2), Size(arm, arm))
    val arms = listOf(
        VirtualControl.UP to Rect(center.x - arm / 2, frame.top, center.x + arm / 2, center.y - arm / 2),
        VirtualControl.DOWN to Rect(center.x - arm / 2, center.y + arm / 2, center.x + arm / 2, frame.bottom),
        VirtualControl.LEFT to Rect(frame.left, center.y - arm / 2, center.x - arm / 2, center.y + arm / 2),
        VirtualControl.RIGHT to Rect(center.x + arm / 2, center.y - arm / 2, frame.right, center.y + arm / 2),
    )
    for ((direction, rect) in arms) {
        val fill = if (directions and direction != 0) FILL_HELD else FILL
        drawRoundRect(fill, rect.topLeft, rect.size, corner)
        drawRoundRect(OUTLINE, rect.topLeft, rect.size, corner, style = outline)
        drawArrow(rect.center, arm * ARROW_SHARE, direction)
    }
}

/** Draws a triangle of [size] at [center], pointing in [direction]. */
private fun DrawScope.drawArrow(center: Offset, size: Float, direction: Int) {
    val half = size / 2
    val (tip, base1, base2) = when (direction) {
        VirtualControl.UP -> Triple(Offset(0f, -half), Offset(-half, half), Offset(half, half))
        VirtualControl.DOWN -> Triple(Offset(0f, half), Offset(-half, -half), Offset(half, -half))
        VirtualControl.LEFT -> Triple(Offset(-half, 0f), Offset(half, -half), Offset(half, half))
        else -> Triple(Offset(half, 0f), Offset(-half, -half), Offset(-half, half))
    }
    val path = Path().apply {
        moveTo(center.x + tip.x, center.y + tip.y)
        lineTo(center.x + base1.x, center.y + base1.y)
        lineTo(center.x + base2.x, center.y + base2.y)
        close()
    }
    drawPath(path, INK)
}

/** Draws a shoulder button or trigger: a rounded rectangle and its label. */
private fun DrawScope.drawShoulder(frame: Rect, control: VirtualControl, outline: Stroke, measurer: TextMeasurer) {
    val corner = CornerRadius(frame.height * SHOULDER_CORNER_SHARE)
    drawRoundRect(if (control.held) FILL_HELD else FILL, frame.topLeft, frame.size, corner)
    drawRoundRect(OUTLINE, frame.topLeft, frame.size, corner, style = outline)
    drawLabel(measurer, LABELS[control.code].orEmpty(), frame, INK)
}

/** Draws a round button: the face buttons, the stick buttons, Select and Start. */
private fun DrawScope.drawRoundButton(frame: Rect, control: VirtualControl, outline: Stroke, measurer: TextMeasurer) {
    val radius = frame.width / 2
    drawCircle(if (control.held) FILL_HELD else FILL, radius, frame.center)
    drawCircle(OUTLINE, radius, frame.center, style = outline)
    when (control.code) {
        VirtualControl.SELECT -> drawViewGlyph(frame.center, radius * GLYPH_SHARE, outline)
        VirtualControl.START -> drawMenuGlyph(frame.center, radius * GLYPH_SHARE, outline)
        else -> drawLabel(measurer, LABELS[control.code].orEmpty(), frame, FACE_COLORS[control.code] ?: INK)
    }
}

/** Draws Select's glyph, two overlapping windows, [size] across at [center]. */
private fun DrawScope.drawViewGlyph(center: Offset, size: Float, outline: Stroke) {
    val window = Size(size * 0.7f, size * 0.55f)
    val corner = CornerRadius(size * 0.1f)
    drawRoundRect(INK, center - Offset(size / 2, size * 0.45f), window, corner, style = outline)
    drawRoundRect(INK, center - Offset(size * 0.2f, size * 0.1f), window, corner, style = outline)
}

/** Draws Start's glyph, three lines, [size] across at [center]. */
private fun DrawScope.drawMenuGlyph(center: Offset, size: Float, outline: Stroke) {
    for (row in -1..1) {
        val y = center.y + row * size * 0.35f
        drawLine(INK, Offset(center.x - size / 2, y), Offset(center.x + size / 2, y), outline.width)
    }
}

/** Draws [text] in [color], centred in [frame], sized to it. */
private fun DrawScope.drawLabel(measurer: TextMeasurer, text: String, frame: Rect, color: Color) {
    val style = TextStyle(color = color, fontSize = (frame.height * LABEL_SHARE).toSp(), fontWeight = FontWeight.SemiBold)
    val layout = measurer.measure(text, style)
    drawText(layout, topLeft = frame.center - Offset(layout.size.width / 2f, layout.size.height / 2f))
}
