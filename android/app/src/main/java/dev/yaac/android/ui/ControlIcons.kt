package dev.yaac.android.ui

import kotlin.math.roundToInt

/**
 * The compact control row's pure rules. Nothing here knows a model or an effort by name
 * (invariant 3): an effort's colour comes from its rank in the levels the model reports, so a
 * model with more or fewer levels still spreads them from the calmest to the strongest.
 */
object ControlIcons {
    /** The effort scale, calmest first: grey, white, orange, red, purple. */
    val EffortScale: List<Long> = listOf(0xFF8A9099, 0xFFE6E8EC, 0xFFF0883E, 0xFFE5534B, 0xFFA371F7)

    /**
     * The scale position (0 until [EffortScale].size) of level [index] among [count] levels:
     * up to five levels take the scale in order from the calm end (low, medium, high → grey,
     * white, orange); more levels are spread over it so the strongest is always purple.
     */
    fun effortStep(index: Int, count: Int): Int {
        if (index < 0 || count <= 0) return 0
        val last = EffortScale.size - 1
        if (count <= EffortScale.size) return index.coerceAtMost(last)
        return (index.toFloat() * last / (count - 1)).roundToInt().coerceIn(0, last)
    }

    /** The letter shown on an effort badge: the level label's first letter or digit, upper case. */
    fun initial(label: String): String =
        label.trim().firstOrNull { it.isLetterOrDigit() }?.uppercaseChar()?.toString() ?: "?"

    /**
     * Text that is legible on [argb]: dark on light fills, light on dark ones
     * (relative luminance against the WCAG midpoint).
     */
    fun inkOn(argb: Long): Long {
        fun lin(c: Int): Double { val v = c / 255.0; return if (v <= 0.03928) v / 12.92 else Math.pow((v + 0.055) / 1.055, 2.4) }
        val r = ((argb shr 16) and 0xFF).toInt(); val g = ((argb shr 8) and 0xFF).toInt(); val b = (argb and 0xFF).toInt()
        val l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
        return if (l > 0.35) 0xFF16181D else 0xFFFFFFFF
    }

    /** The model's own name from the control's "instance · model" label (the logo stands for the instance). */
    fun modelName(label: String): String = label.substringAfterLast(" · ").ifBlank { label }
}
