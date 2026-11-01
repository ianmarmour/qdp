package io.github.ianmarmour.qdp

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.view.View

/**
 * Draws the host's cursor over the video, at the pointer's position in
 * video pixels, scaled as the video is.
 */
class CursorView(context: Context) : View(context) {
    private var image: Bitmap? = null
    private var hotspotX = 0f
    private var hotspotY = 0f
    private var pointerX = Float.NaN
    private var pointerY = Float.NaN
    private var videoWidth = 1
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG)

    /** Whether the cursor is drawn: not while the host's pointer is captured. */
    var shown = true
        set(value) {
            field = value
            invalidate()
        }

    init {
        // Only draws; touches go to the video beneath.
        isClickable = false
        isFocusable = false
    }

    /** Sets the image from [Native.cursorImage]'s values, or hides it for `null`. */
    fun setImage(values: IntArray?) {
        image = values?.takeIf { it.size >= HEADER }?.let {
            val width = it[0]
            val height = it[1]
            hotspotX = it[2].toFloat()
            hotspotY = it[3].toFloat()
            if (width <= 0 || height <= 0 || it.size < HEADER + width * height) {
                null
            } else {
                Bitmap.createBitmap(it, HEADER, width, width, height, Bitmap.Config.ARGB_8888)
            }
        }
        invalidate()
    }

    /** Moves the cursor to [x], [y] in video pixels. */
    fun moveTo(x: Float, y: Float) {
        if (x != pointerX || y != pointerY) {
            pointerX = x
            pointerY = y
            invalidate()
        }
    }

    /** Sets the width of the video the positions are in. */
    fun setVideoWidth(width: Int) {
        videoWidth = width.coerceAtLeast(1)
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val image = image ?: return
        if (!shown || pointerX.isNaN()) {
            return
        }
        val scale = width.toFloat() / videoWidth
        canvas.save()
        canvas.translate(pointerX * scale, pointerY * scale)
        canvas.scale(scale, scale)
        canvas.drawBitmap(image, -hotspotX, -hotspotY, paint)
        canvas.restore()
    }

    private companion object {
        /** The values before the pixels: width, height and hotspot. */
        const val HEADER = 4
    }
}
