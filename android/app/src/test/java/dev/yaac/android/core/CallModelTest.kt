package dev.yaac.android.core

import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CallModelTest {
    private fun obj(json: String) = FrameJson.parseToJsonElement(json) as JsonObject

    private val offer = """
        v=0
        o=- 1 2 IN IP4 127.0.0.1
        s=-
        m=audio 9 UDP/TLS/RTP/SAVPF 111
        a=rtpmap:111 opus/48000/2
        a=candidate:1 1 udp 2122260223 192.168.1.5 50000 typ host
        m=application 9 UDP/DTLS/SCTP webrtc-datachannel
        a=sctp-port:5000
    """.trimIndent().replace("\n", "\r\n")

    // ---- the machine ---------------------------------------------------------------------

    private fun offering(m: CallMachine = CallMachine()): CallMachine {
        m.start("k1", "idle", 0)
        m.offer(offer, 100)
        return m
    }

    @Test fun a_call_creates_the_offer_then_sends_it_with_the_key() {
        val m = CallMachine()
        assertEquals(listOf(CallEffect.CreateOffer), m.start("k1", "idle", 0))
        assertEquals(CallPhase.Preparing, m.phase)
        assertEquals(listOf(CallEffect.SendStart("k1", offer)), m.offer(offer, 100))
        assertEquals(CallPhase.Offering, m.phase)
    }

    @Test fun a_second_start_while_live_does_nothing() {
        val m = offering()
        assertTrue(m.start("k1", "connecting", 200).isEmpty())
        assertTrue(m.start("k2", "idle", 200).isEmpty())
        assertEquals("k1", m.key)
    }

    @Test fun an_offer_without_candidates_fails_and_never_reaches_the_controller() {
        val m = CallMachine()
        m.start("k1", "idle", 0)
        val fx = m.offer(offer.lines().filterNot { it.startsWith("a=candidate") }.joinToString("\r\n"), 100)
        assertEquals(listOf(CallEffect.ClosePeer), fx)
        assertEquals(CallPhase.Failed, m.phase)
        assertNotNull(m.error)
    }

    @Test fun the_answer_is_applied_once() {
        val m = offering()
        assertEquals(listOf(CallEffect.ApplyAnswer("v=0\r\n")), m.answer("c1", "v=0\r\n", 150))
        assertEquals(CallPhase.Connecting, m.phase)
        // The frame keeps the answer while the call lives: later frames do not apply it again.
        assertTrue(m.answer("c1", "v=0\r\n", 160).isEmpty())
    }

    @Test fun the_last_calls_answer_still_in_the_frame_is_not_taken_for_this_one() {
        val m = CallMachine()
        m.start("k1", "idle", 0, null, lastCallId = "c0")
        m.offer(offer, 1)
        assertTrue(m.answer("c0", "v=0\r\n", 2).isEmpty())
        assertEquals(listOf(CallEffect.ApplyAnswer("v=0\r\n")), m.answer("c1", "v=0\r\n", 3))
    }

    @Test fun an_answer_without_a_call_id_is_taken_for_the_live_call() {
        val m = offering()
        assertEquals(listOf(CallEffect.ApplyAnswer("v=0\r\n")), m.answer(null, "v=0\r\n", 150))
    }

    @Test fun a_malformed_answer_ends_the_call() {
        val m = offering()
        assertEquals(listOf(CallEffect.SendEnd("k1"), CallEffect.ClosePeer), m.answer("c1", "<html>", 150))
        assertEquals(CallPhase.Failed, m.phase)
    }

    @Test fun the_open_event_channel_makes_the_call_active_and_starts_the_clock() {
        val m = offering()
        m.answer("c1", "v=0\r\n", 150)
        m.connected(1_000)
        assertEquals(CallPhase.Active, m.phase)
        assertEquals(1_000L, m.startedAt)
        assertNull(m.deadline)
    }

    @Test fun end_hangs_up_once() {
        val m = offering()
        assertEquals(listOf(CallEffect.SendEnd("k1"), CallEffect.ClosePeer), m.end())
        assertEquals(CallPhase.Ended, m.phase)
        assertTrue(m.end().isEmpty())
    }

    @Test fun end_before_the_offer_was_sent_tells_the_controller_nothing() {
        val m = CallMachine()
        m.start("k1", "idle", 0)
        assertEquals(listOf(CallEffect.ClosePeer), m.end())
    }

    @Test fun the_controller_ending_or_failing_the_call_closes_the_peer() {
        val m = offering()
        m.controller("connecting", null)
        assertEquals(listOf(CallEffect.ClosePeer), m.controller("error", "The call could not start"))
        assertEquals(CallPhase.Failed, m.phase)
        assertEquals("The call could not start", m.error)

        val n = offering()
        n.answer("c1", "v=0\r\n", 1); n.connected(2)
        assertEquals(listOf(CallEffect.ClosePeer), n.controller("ended", null))
        assertEquals(CallPhase.Ended, n.phase)
    }

    @Test fun a_stale_end_from_the_last_call_is_not_taken_for_this_one() {
        val m = CallMachine()
        m.start("k1", "idle", 0)
        m.offer(offer, 1)
        // Republished unchanged: still before this call.
        assertTrue(m.controller("idle", null).isEmpty())
        assertEquals(CallPhase.Offering, m.phase)
        m.controller("starting", null)
        assertEquals(listOf(CallEffect.ClosePeer), m.controller("idle", null))
        assertEquals(CallPhase.Ended, m.phase)
    }

    @Test fun an_error_that_differs_from_the_one_at_start_is_fresh() {
        val m = CallMachine()
        m.start("k1", "error", 0, "old")
        m.offer(offer, 1)
        assertTrue(m.controller("error", "old").isEmpty())
        assertEquals(listOf(CallEffect.ClosePeer), m.controller("error", "new"))
    }

    @Test fun each_stage_has_a_deadline() {
        val m = CallMachine()
        m.start("k1", "idle", 0)
        assertEquals(CallMachine.PREPARE_MS, m.deadline)
        assertEquals(listOf(CallEffect.ClosePeer), m.tick(CallMachine.PREPARE_MS))
        assertEquals(CallPhase.Failed, m.phase)

        val n = offering()
        assertTrue(n.tick(100 + CallMachine.ANSWER_MS - 1).isEmpty())
        assertEquals(listOf(CallEffect.SendEnd("k1"), CallEffect.ClosePeer), n.tick(100 + CallMachine.ANSWER_MS))

        val o = offering()
        o.answer("c1", "v=0\r\n", 200)
        assertEquals(200 + CallMachine.CONNECT_MS, o.deadline)
    }

    @Test fun a_short_loss_of_the_network_is_survived_a_long_one_ends_the_call() {
        val m = offering()
        m.answer("c1", "v=0\r\n", 1); m.connected(10)
        m.lost(1_000)
        assertEquals(CallPhase.Active, m.phase)
        assertTrue(m.tick(1_000 + CallMachine.LOST_MS - 1).isEmpty())
        m.connected(5_000)
        assertEquals(10L, m.startedAt)
        assertNull(m.deadline)
        m.lost(6_000)
        assertEquals(listOf(CallEffect.SendEnd("k1"), CallEffect.ClosePeer), m.tick(6_000 + CallMachine.LOST_MS))
        assertEquals(CallPhase.Failed, m.phase)
    }

    @Test fun a_peer_failure_ends_the_call_with_its_reason() {
        val m = offering()
        assertEquals(listOf(CallEffect.SendEnd("k1"), CallEffect.ClosePeer), m.peerFailed("No network path to the call"))
        assertEquals("No network path to the call", m.error)
    }

    @Test fun a_new_call_after_one_ended_starts_clean() {
        val m = offering()
        m.end()
        assertEquals(listOf(CallEffect.CreateOffer), m.start("k2", "idle", 500))
        assertEquals("k2", m.key)
        assertNull(m.error)
        assertNull(m.startedAt)
    }

    // ---- SDP -----------------------------------------------------------------------------

    @Test fun an_offer_needs_audio_the_event_channel_and_a_candidate() {
        assertNull(CallSdp.checkOffer(offer))
        assertNotNull(CallSdp.checkOffer(""))
        assertNotNull(CallSdp.checkOffer(offer.replace("m=application", "m=video")))
        assertNotNull(CallSdp.checkOffer(offer.replace("m=audio", "m=video")))
        assertNotNull(CallSdp.checkOffer(offer + "x".repeat(CallSdp.MAX_BYTES)))
        assertEquals(1, CallSdp.candidates(offer))
    }

    @Test fun an_answer_is_bounded_sdp() {
        assertNull(CallSdp.checkAnswer("v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\n"))
        assertNotNull(CallSdp.checkAnswer(""))
        assertNotNull(CallSdp.checkAnswer("{\"error\":1}"))
        assertNotNull(CallSdp.checkAnswer("v=0\r\n" + "x".repeat(CallSdp.MAX_BYTES)))
    }

    // ---- the frame -----------------------------------------------------------------------

    @Test fun the_call_frame_is_read_leniently() {
        val f = CallFrame.of(obj("""{"state":"active","callId":"c1","answer":"v=0","error":null,"status":"In a call",
            "button":{"id":"composer.callEnd","label":"End","enabled":true},
            "transcript":[{"role":"user","text":"hi","final":true},{"role":"assistant","text":"hel","final":false},{"role":"x","text":"?"},{"role":"user","text":" "}]}"""))
        assertEquals("active", f.state)
        assertEquals("c1", f.callId)
        assertEquals("v=0", f.answer)
        assertNull(f.error)
        assertEquals("In a call", f.status)
        assertTrue(f.enabled)
        assertEquals(listOf(CallLine(true, "hi", true), CallLine(false, "hel", false)), f.lines)
        val idle = CallFrame.of(null)
        assertEquals("idle", idle.state)
        assertNull(idle.answer)
        val refused = CallFrame.of(obj("""{"state":"idle","button":{"enabled":false,"detail":"Another call is running"}}"""))
        assertFalse(refused.enabled)
        assertEquals("Another call is running", refused.detail)
    }

    @Test fun the_transcript_keeps_its_last_lines() {
        val many = (1..20).joinToString(",") { """{"role":"user","text":"l$it"}""" }
        val f = CallFrame.of(obj("""{"state":"active","transcript":[$many]}"""))
        assertEquals(CallFrame.MAX_LINES, f.lines.size)
        assertEquals("l20", f.lines.last().text)
    }

    @Test fun the_capability_is_the_presence_of_the_composers_call_never_the_provider() {
        assertTrue(CallWire.offered(obj("""{"call":{"state":"idle"}}""")))
        assertFalse(CallWire.offered(obj("""{"call":null}""")))
        assertFalse(CallWire.offered(obj("""{"provider":"codex"}""")))
        assertFalse(CallWire.offered(null))
    }

    @Test fun the_call_of_a_key_is_never_another_threads() {
        val composer = obj("""{"key":"k1","call":{"state":"active"}}""")
        assertEquals("active", CallWire.callFor("k1") { if (it == "composer") composer else null }!!["state"].str)
        // Another thread is open: its composer says nothing about this call.
        assertNull(CallWire.callFor("k2") { if (it == "composer") composer else null })
        assertNull(CallWire.callFor("k1") { null })
    }

    @Test fun commands_carry_the_key_and_the_offer() {
        assertEquals("composer.callStart", CallWire.START)
        assertEquals("composer.callEnd", CallWire.END)
        assertEquals(mapOf("key" to "k", "sdpOffer" to "v=0"), CallWire.startPayload("k", "v=0"))
        assertEquals(mapOf("key" to "k"), CallWire.endPayload("k"))
    }

    // ---- routing, time, consent ----------------------------------------------------------

    @Test fun the_speaker_is_the_default_unless_a_headset_is_connected() {
        assertTrue(CallRoute.defaultSpeaker(listOf(CallOutput.Earpiece, CallOutput.Speaker)))
        assertFalse(CallRoute.defaultSpeaker(listOf(CallOutput.Earpiece, CallOutput.Speaker, CallOutput.Bluetooth)))
    }

    @Test fun off_the_speaker_a_headset_wins_over_the_earpiece() {
        val all = listOf(CallOutput.Earpiece, CallOutput.Speaker, CallOutput.Bluetooth, CallOutput.Wired)
        assertEquals(CallOutput.Speaker, CallRoute.pick(all, speaker = true))
        assertEquals(CallOutput.Wired, CallRoute.pick(all, speaker = false))
        assertEquals(CallOutput.Bluetooth, CallRoute.pick(listOf(CallOutput.Earpiece, CallOutput.Bluetooth), speaker = false))
        assertEquals(CallOutput.Earpiece, CallRoute.pick(listOf(CallOutput.Earpiece, CallOutput.Speaker), speaker = false))
        // A tablet without an earpiece stays on the speaker.
        assertEquals(CallOutput.Speaker, CallRoute.pick(listOf(CallOutput.Speaker), speaker = false))
        assertNull(CallRoute.pick(emptyList(), speaker = true))
    }

    @Test fun elapsed_time_reads_like_a_phone_call() {
        assertEquals("0:00", CallTime.elapsed(0))
        assertEquals("0:42", CallTime.elapsed(42_900))
        assertEquals("12:05", CallTime.elapsed(725_000))
        assertEquals("1:02:03", CallTime.elapsed(3_723_000))
        assertEquals("0:00", CallTime.elapsed(-5))
    }

    @Test fun consent_is_asked_once() {
        val saved = HashMap<String, Boolean>()
        val consent = CallConsent({ saved[it] ?: false }, { k, v -> saved[k] = v })
        assertTrue(consent.needed())
        consent.give()
        assertFalse(consent.needed())
        assertFalse(CallConsent({ saved[it] ?: false }, { _, _ -> }).needed())
    }
}
