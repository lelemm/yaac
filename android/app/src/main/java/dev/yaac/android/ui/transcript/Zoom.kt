package dev.yaac.android.ui.transcript

/** A point or a size in the viewer, in pixels. Plain floats so the math tests on the JVM. */
data class Vec(val x: Float, val y: Float) {
    operator fun plus(o: Vec) = Vec(x + o.x, y + o.y)
    operator fun minus(o: Vec) = Vec(x - o.x, y - o.y)
    operator fun times(s: Float) = Vec(x * s, y * s)
}

/**
 * The image viewer's zoom (T14): [scale] 1 is the picture fitted to the screen, [pan] the
 * translation of the layer (scaled about the viewer's centre). Zooming keeps the point under
 * the fingers (or the tap) where it is; the picture cannot be dragged past its edges.
 */
data class Zoom(val scale: Float = 1f, val pan: Vec = Vec(0f, 0f)) {
    val fitted: Boolean get() = scale == MIN && pan == Vec(0f, 0f)

    /** Zoom by [factor] about [focus] (viewer coordinates), then move by [move], in a [size] viewer. */
    fun transform(size: Vec, focus: Vec, factor: Float, move: Vec = Vec(0f, 0f)): Zoom {
        val next = (scale * factor).coerceIn(MIN, MAX)
        val centre = size * 0.5f
        val from = focus - centre
        // The content point under the focus stays under it: from = q·s + t before and after.
        val pan = from - (from - this.pan) * (next / scale) + move
        return Zoom(next, clamp(pan, next, size))
    }

    /** Double tap: zoom in about the tap, or back to fit when already zoomed. */
    fun toggle(size: Vec, focus: Vec): Zoom = if (scale > MIN) Zoom() else transform(size, focus, STEP_DOUBLE)

    /** The + and − controls zoom about the centre. */
    fun step(size: Vec, zoomIn: Boolean): Zoom = transform(size, size * 0.5f, if (zoomIn) STEP else 1f / STEP)

    val percent: Int get() = Math.round(scale * 100)

    companion object {
        const val MIN = 1f
        const val MAX = 8f
        const val STEP = 1.5f
        const val STEP_DOUBLE = 2.5f

        fun clamp(pan: Vec, scale: Float, size: Vec): Vec {
            val limit = size * ((scale - 1f) / 2f)
            return Vec(pan.x.coerceIn(-limit.x, limit.x), pan.y.coerceIn(-limit.y, limit.y))
        }
    }
}
