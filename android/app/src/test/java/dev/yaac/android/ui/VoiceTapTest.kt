package dev.yaac.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class VoiceTapTest {
    @Test fun a_tap_starts_when_idle_or_after_an_error() {
        assertEquals(VoiceTap.Start, voiceTap("idle", recording = false))
        assertEquals(VoiceTap.Start, voiceTap("error", recording = false))
    }

    @Test fun a_tap_stops_while_starting_or_listening_or_before_the_controller_caught_up() {
        assertEquals(VoiceTap.Stop, voiceTap("listening", recording = true))
        assertEquals(VoiceTap.Stop, voiceTap("starting", recording = true))
        // Tapped twice before the first frame with `starting`: the second tap stops.
        assertEquals(VoiceTap.Stop, voiceTap("idle", recording = true))
    }

    @Test fun a_tap_while_finishing_does_nothing() {
        assertEquals(VoiceTap.None, voiceTap("stopping", recording = false))
    }
}

class ProvisionalTest {
    @Test fun the_words_heard_show_at_the_caret_after_a_space_when_they_follow_a_word() {
        assertEquals(Provisional(3, " the parser"), Provisional.of("fix it", 3, "the parser"))
        assertEquals(Provisional(0, "hello"), Provisional.of("", 0, " hello "))
        assertEquals(Provisional(5, "next"), Provisional.of("line\n", 5, "next"))
        assertEquals(null, Provisional.of("x", 1, "  "))
        assertEquals(Provisional(1, " y"), Provisional.of("x", 9, "y"))
    }

    @Test fun offsets_map_around_the_words_both_ways() {
        val p = Provisional(3, " ab")
        assertEquals(listOf(0, 1, 2, 6, 7), (0..4).map(p::toShown))
        assertEquals(listOf(0, 1, 2, 3, 3, 3, 3, 4), (0..7).map(p::toOriginal))
        // Round trip for every original offset.
        for (o in 0..10) assertEquals(o, p.toOriginal(p.toShown(o)))
    }
}
