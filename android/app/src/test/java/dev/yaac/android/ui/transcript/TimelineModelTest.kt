package dev.yaac.android.ui.transcript

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TimelineModelTest {
    @Test fun a_hold_only_starts_in_the_edge_zone() {
        assertTrue(TimelineModel.inEdge(0f, 50f))
        assertTrue(TimelineModel.inEdge(50f, 50f))
        assertFalse(TimelineModel.inEdge(51f, 50f))
    }

    @Test fun the_finger_maps_to_prompts_spread_over_the_track_and_clamps_at_the_ends() {
        // Five prompts on a 400 px track starting at 100 px: one every 100 px.
        assertEquals(0, TimelineModel.indexAt(100f, 100f, 400f, 5))
        assertEquals(2, TimelineModel.indexAt(300f, 100f, 400f, 5))
        assertEquals(4, TimelineModel.indexAt(500f, 100f, 400f, 5))
        assertEquals(0, TimelineModel.indexAt(-40f, 100f, 400f, 5))
        assertEquals(4, TimelineModel.indexAt(9_000f, 100f, 400f, 5))
        assertEquals(-1, TimelineModel.indexAt(200f, 100f, 400f, 0))
    }

    @Test fun a_dash_sits_where_its_prompt_is_picked() {
        for (i in 0 until 7) {
            val y = TimelineModel.yOf(i, 24f, 600f, 7)
            assertEquals(i, TimelineModel.indexAt(y, 24f, 600f, 7))
        }
    }

    @Test fun dashes_swell_around_the_finger_and_rest_elsewhere() {
        assertEquals(1f, TimelineModel.swell(5, 5), 0f)
        assertTrue(TimelineModel.swell(4, 5) > TimelineModel.swell(3, 5))
        assertEquals(0f, TimelineModel.swell(1, 5), 0f)
        assertEquals(0f, TimelineModel.swell(5, -1), 0f)
        // Between two prompts the ripple is centred between them.
        assertEquals(TimelineModel.swellAt(4, 4.5f), TimelineModel.swellAt(5, 4.5f), 1e-6f)
    }

    @Test fun many_prompts_are_sampled_but_keep_the_ends_and_the_active_one() {
        assertEquals((0 until 5).toList(), TimelineModel.visibleDashes(5, 600f, 6f, 2))
        val many = TimelineModel.visibleDashes(500, 600f, 6f, 333)
        assertTrue(many.size <= 103)
        assertTrue(0 in many && 499 in many && 333 in many)
        assertEquals(many.sorted(), many)
    }
}
