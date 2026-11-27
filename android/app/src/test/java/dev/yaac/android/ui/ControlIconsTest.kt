package dev.yaac.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class ControlIconsTest {
    @Test fun five_levels_take_the_scale_in_order() {
        assertEquals((0..4).toList(), (0..4).map { ControlIcons.effortStep(it, 5) })
    }

    @Test fun fewer_levels_start_from_the_calm_end() {
        assertEquals(listOf(0, 1, 2), (0..2).map { ControlIcons.effortStep(it, 3) })
    }

    @Test fun more_levels_spread_and_end_on_the_strongest() {
        val steps = (0..5).map { ControlIcons.effortStep(it, 6) }
        assertEquals(0, steps.first()); assertEquals(4, steps.last()); assertEquals(steps.sorted(), steps)
    }

    @Test fun initials_and_model_names() {
        assertEquals("X", ControlIcons.initial("xhigh"))
        assertEquals("M", ControlIcons.initial(" medium"))
        assertEquals("Opus 5.5", ControlIcons.modelName("Claude · Opus 5.5"))
        assertEquals("gpt-6.2-sol", ControlIcons.modelName("Codex · work · gpt-6.2-sol"))
    }

    @Test fun badge_text_contrasts_with_its_fill() {
        assertEquals(0xFF16181D, ControlIcons.inkOn(0xFFE6E8EC))
        assertEquals(0xFFFFFFFF, ControlIcons.inkOn(0xFFA371F7))
    }
}
