package dev.yaac.android.core

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.sin

class VoiceAudioTest {
    @Test fun the_wanted_rate_is_used_when_the_device_records_it() {
        assertEquals(16_000, VoiceAudio.recordRate(16_000) { true })
        assertEquals(24_000, VoiceAudio.recordRate(24_000) { true })
    }

    @Test fun otherwise_the_first_rate_the_device_opens_and_null_when_none() {
        assertEquals(48_000, VoiceAudio.recordRate(16_000) { it == 48_000 || it == 44_100 })
        assertEquals(44_100, VoiceAudio.recordRate(24_000) { it == 44_100 })
        assertNull(VoiceAudio.recordRate(16_000) { false })
    }

    @Test fun samples_are_little_endian_pcm16() {
        val bytes = VoiceAudio.toBytes(shortArrayOf(1, -1, 0x1234, Short.MIN_VALUE))
        assertArrayEquals(byteArrayOf(1, 0, -1, -1, 0x34, 0x12, 0, -128), bytes)
    }

    @Test fun base64_is_padded_and_has_no_line_breaks() {
        assertEquals("AQD//w==", VoiceAudio.base64(VoiceAudio.toBytes(shortArrayOf(1, -1))))
        val long = VoiceAudio.base64(ByteArray(3200))
        assertTrue('\n' !in long)
    }

    @Test fun a_chunk_is_a_tenth_of_a_second() {
        assertEquals(1600, VoiceAudio.chunkSamples(16_000))
        assertEquals(2400, VoiceAudio.chunkSamples(24_000))
    }

    @Test fun the_chunker_hands_out_whole_chunks_and_flushes_the_rest() {
        val chunker = Chunker(4)
        assertEquals(0, chunker.push(shortArrayOf(1, 2, 3)).size)
        val out = chunker.push(shortArrayOf(4, 5, 6, 7, 8, 9, 10))
        assertEquals(2, out.size)
        assertArrayEquals(VoiceAudio.toBytes(shortArrayOf(1, 2, 3, 4)), out[0])
        assertArrayEquals(VoiceAudio.toBytes(shortArrayOf(5, 6, 7, 8)), out[1])
        assertArrayEquals(VoiceAudio.toBytes(shortArrayOf(9, 10)), chunker.flush())
        assertNull(chunker.flush())
    }

    @Test fun equal_rates_pass_through() {
        val r = Resampler(16_000, 16_000)
        assertArrayEquals(shortArrayOf(1, 2, 3), r.push(shortArrayOf(1, 2, 3, 4), 3))
    }

    @Test fun resampling_keeps_the_length_ratio_across_chunk_boundaries() {
        val r = Resampler(48_000, 16_000)
        var total = 0
        repeat(100) { total += r.push(ShortArray(480) { 100 }).size }
        // 48 000 samples in → 16 000 out, give or take the one still waiting.
        assertTrue("got $total", abs(total - 16_000) <= 1)
        val up = Resampler(16_000, 44_100)
        var n = 0
        repeat(10) { n += up.push(ShortArray(1600)).size }
        // Up to one input sample (~2.8 output samples) waits for the next chunk.
        assertTrue("got $n", n in 44_097..44_100)
    }

    @Test fun a_tone_survives_resampling_in_chunks_as_in_one_piece() {
        val tone = ShortArray(4800) { (8000 * sin(2 * PI * 440 * it / 48_000)).toInt().toShort() }
        val whole = Resampler(48_000, 16_000).push(tone)
        val split = Resampler(48_000, 16_000)
        val pieces = ArrayList<Short>()
        for (start in 0 until 4800 step 333) {
            val end = minOf(4800, start + 333)
            pieces += split.push(tone.copyOfRange(start, end)).toList()
        }
        assertEquals(whole.size, pieces.size)
        for (i in whole.indices) assertTrue("sample $i", abs(whole[i] - pieces[i]) <= 1)
    }

    @Test fun the_pipeline_sends_tenth_second_chunks_at_the_harness_rate() {
        val p = VoicePipeline(48_000, 16_000)
        val (first, level) = p.push(ShortArray(4800) { 16_000 }, 4800)
        assertEquals(1, first.size)
        assertEquals(3200, java.util.Base64.getDecoder().decode(first[0]).size)
        assertTrue(level > 0.5f)
        assertNull(p.flush())
        assertEquals(0f, VoiceAudio.level(ShortArray(10)))
    }
}
