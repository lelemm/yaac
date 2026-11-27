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
