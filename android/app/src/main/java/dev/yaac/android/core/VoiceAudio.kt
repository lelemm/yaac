package dev.yaac.android.core

import kotlin.math.floor
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sqrt

/**
 * Voice input's audio path (docs/plan/voice.md), as plain values: no Android types, so it is
 * tested on the JVM. The microphone gives mono 16-bit samples at the rate the device could open;
 * the thread's harness wants the rate in the composer's `voice.format`. These turn one into the
 * other and cut it into ~100 ms base64 chunks for `composer.voiceAudio`.
 */
object VoiceAudio {
    /** Length of one `composer.voiceAudio` chunk. */
    const val CHUNK_MS = 100

    /** Recording rates to try after the harness's own, most to least common on phones. */
    private val FALLBACK_RATES = intArrayOf(48_000, 44_100, 32_000, 16_000, 22_050, 11_025, 8_000)

    /**
     * The rate to open the microphone at: the [wanted] one when the device can record it,
     * otherwise the first fallback it can (then resampled), or null when none opens.
     */
    fun recordRate(wanted: Int, canRecord: (Int) -> Boolean): Int? =
        sequenceOf(wanted).plus(FALLBACK_RATES.asSequence()).filter { it > 0 }.distinct().firstOrNull(canRecord)

    /** Samples in one chunk at [rate]. */
    fun chunkSamples(rate: Int): Int = maxOf(1, rate * CHUNK_MS / 1000)

    /** Little-endian bytes of [samples] (`pcm16le`). */
    fun toBytes(samples: ShortArray, count: Int = samples.size): ByteArray {
        val out = ByteArray(count * 2)
        for (i in 0 until count) {
            val s = samples[i].toInt()
            out[2 * i] = (s and 0xff).toByte()
            out[2 * i + 1] = ((s shr 8) and 0xff).toByte()
        }
        return out
    }

    /** Base64 (standard alphabet, padded, no line breaks) of [bytes]: `composer.voiceAudio`'s `pcm`. */
    fun base64(bytes: ByteArray): String = java.util.Base64.getEncoder().encodeToString(bytes)

    /** Loudness of [samples] in 0..1 (RMS, scaled so ordinary speech is near the middle). */
    fun level(samples: ShortArray, count: Int = samples.size): Float {
        if (count <= 0) return 0f
        var sum = 0.0
        for (i in 0 until count) { val v = samples[i] / 32768.0; sum += v * v }
        val rms = sqrt(sum / count)
        return min(1.0, rms * 4).toFloat()
    }
}

/**
 * Streaming linear resampler, mono 16-bit, from [from] Hz to [to] Hz. State carries across
 * [push] calls, so chunk boundaries do not click or drop samples. Equal rates pass through.
 */
class Resampler(private val from: Int, private val to: Int) {
    init { require(from > 0 && to > 0) { "rates must be positive" } }

    private val step = from.toDouble() / to
    /** Where the next output sample falls, in input samples from the next input's first (> -1). */
    private var t = 0.0
    /** The previous input's last sample: index -1 of the next input. */
    private var prev: Short = 0

    fun push(input: ShortArray, count: Int = input.size): ShortArray {
        if (from == to) return input.copyOf(count)
        if (count == 0) return ShortArray(0)
        fun at(i: Int): Int = if (i < 0) prev.toInt() else input[minOf(i, count - 1)].toInt()
        val out = ShortArray(((count - t) / step).toInt() + 2)
        var n = 0
        while (t <= count - 1) {
            val i = floor(t).toInt()
            val frac = t - i
            val a = at(i)
            val b = at(i + 1)
            out[n++] = (a + (b - a) * frac).roundToInt().coerceIn(-32768, 32767).toShort()
            t += step
        }
        t -= count
        prev = input[count - 1]
        return out.copyOf(n)
    }
}

/**
 * Collects samples and hands out whole chunks of [chunk] samples as `pcm16le` bytes; [flush]
 * gives the rest when recording stops, so the last words are not cut.
 */
class Chunker(private val chunk: Int) {
    init { require(chunk > 0) }
    private val buf = ShortArray(chunk)
    private var fill = 0

    fun push(samples: ShortArray, count: Int = samples.size): List<ByteArray> {
        val out = ArrayList<ByteArray>()
        var i = 0
        while (i < count) {
            val n = min(chunk - fill, count - i)
            System.arraycopy(samples, i, buf, fill, n)
            fill += n; i += n
            if (fill == chunk) { out += VoiceAudio.toBytes(buf, chunk); fill = 0 }
        }
        return out
    }

    fun flush(): ByteArray? = if (fill == 0) null else VoiceAudio.toBytes(buf, fill).also { fill = 0 }
}

/** One recording's pipeline: device samples in, base64 chunks at the harness's rate out. */
class VoicePipeline(deviceRate: Int, targetRate: Int) {
    private val resampler = Resampler(deviceRate, targetRate)
    private val chunker = Chunker(VoiceAudio.chunkSamples(targetRate))

    /** Chunks ready to send, as `pcm` strings, and the input's loudness. */
    fun push(samples: ShortArray, count: Int): Pair<List<String>, Float> =
        chunker.push(resampler.push(samples, count)).map(VoiceAudio::base64) to VoiceAudio.level(samples, count)

    fun flush(): String? = chunker.flush()?.let(VoiceAudio::base64)
}
