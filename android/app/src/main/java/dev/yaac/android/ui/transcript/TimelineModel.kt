package dev.yaac.android.ui.transcript

import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The scrub timeline (A11f, the phone's form of the desktop's prompt dashes): hidden until a
 * finger is held still at the transcript's left edge, then scrubbed up and down.
 * Pure: the composable feeds it positions in pixels.
 */
object TimelineModel {
    /** How long a finger must stay still at the edge before the timeline opens. */
    const val HOLD_MS = 2_000L

    /** Whether a touch that went down at [x] (px) starts in the edge zone of [edgePx]. */
    fun inEdge(x: Float, edgePx: Float): Boolean = x in 0f..edgePx

    /**
     * The prompt under a finger at [y] (px) on a track running from [top] to [top] + [height].
     * Prompts are spread evenly, oldest at the top; outside the track, the nearest end.
     */
    fun indexAt(y: Float, top: Float, height: Float, count: Int): Int {
        if (count <= 0) return -1
        if (count == 1 || height <= 0f) return 0
        val t = ((y - top) / height).coerceIn(0f, 1f)
        return (t * (count - 1)).roundToInt()
    }

    /** The centre (px) of prompt [index]'s dash on the same track. */
    fun yOf(index: Int, top: Float, height: Float, count: Int): Float =
        if (count <= 1) top + height / 2 else top + height * index / (count - 1)

    /**
     * How much a dash grows (0 = resting, 1 = the one under the finger): a smooth falloff
     * over [reach] neighbours, so the dashes swell around the finger as it moves.
     */
    fun swell(index: Int, active: Int, reach: Int = 3): Float {
        if (active < 0) return 0f
        val d = abs(index - active)
        if (d > reach) return 0f
        val x = 1f - d.toFloat() / (reach + 1)
        return x * x
    }

    /** [swell] around a fractional position, for the animated ripple. */
    fun swellAt(index: Int, position: Float, reach: Int = 3): Float {
        if (position < 0f) return 0f
        val d = abs(index - position)
        if (d > reach + 1) return 0f
        val x = (1f - d / (reach + 1)).coerceAtLeast(0f)
        return x * x
    }

    /**
     * Which dashes to draw when there are more prompts than fit at [minGapPx]: every prompt's
     * index when they fit, otherwise an even sample that always keeps the first, the last and
     * the active one.
     */
    fun visibleDashes(count: Int, height: Float, minGapPx: Float, active: Int): List<Int> {
        if (count <= 0) return emptyList()
        val fit = if (minGapPx <= 0f) count else (height / minGapPx).toInt().coerceAtLeast(2)
        if (count <= fit) return (0 until count).toList()
        val picked = sortedSetOf(0, count - 1)
        for (k in 0 until fit) picked += ((count - 1).toFloat() * k / (fit - 1)).roundToInt()
        if (active in 0 until count) picked += active
        return picked.toList()
    }
}
