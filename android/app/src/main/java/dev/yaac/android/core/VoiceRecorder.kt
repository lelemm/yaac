package dev.yaac.android.core

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder

/**
 * The microphone for voice input (docs/plan/voice.md): mono 16-bit `AudioRecord` on its own
 * thread, never the UI thread. Samples go through [VoicePipeline] to the harness's rate and
 * leave as ~100 ms base64 chunks through [onChunk] (called on the recording thread; the
 * controller's `send` is thread-safe). [onLevel] reports loudness for the indicator, [onError]
 * a failure in words. [stop] sends what is left, then calls [onStopped], so the stop never
 * overtakes the last audio; [cancel] drops it.
 */
class VoiceRecorder(
    private val targetRate: Int,
    private val onChunk: (String) -> Unit,
    private val onLevel: (Float) -> Unit,
    private val onError: (String) -> Unit,
    /** After [stop], once the last chunk has gone: the moment to send `composer.voiceStop`. */
    private val onStopped: () -> Unit,
) {
    @Volatile private var running = false
    @Volatile private var flushOnEnd = true
    private var thread: Thread? = null

    /** Call only once RECORD_AUDIO is granted. */
    @SuppressLint("MissingPermission")
    fun start() {
        if (running) return
        running = true
        thread = Thread({ record() }, "yaac-voice").apply { isDaemon = true; start() }
    }

    fun stop() { flushOnEnd = true; running = false }

    fun cancel() { flushOnEnd = false; running = false }

    @SuppressLint("MissingPermission")
    private fun record() {
        val encoding = AudioFormat.ENCODING_PCM_16BIT
        val channel = AudioFormat.CHANNEL_IN_MONO
        val rate = VoiceAudio.recordRate(targetRate) { AudioRecord.getMinBufferSize(it, channel, encoding) > 0 }
        if (rate == null) { running = false; onError("This phone cannot record at a rate the agent accepts."); return }
        val min = AudioRecord.getMinBufferSize(rate, channel, encoding)
        val frame = VoiceAudio.chunkSamples(rate)
        val record = try {
            AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, rate, channel, encoding, maxOf(min, frame * 2 * 2))
        } catch (e: Exception) {
            running = false; onError("The microphone could not open: ${e.message}"); return
        }
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release(); running = false; onError("The microphone could not open."); return
        }
        val pipeline = VoicePipeline(rate, targetRate)
        val buffer = ShortArray(frame)
        var failed = false
        try {
            record.startRecording()
            while (running) {
                val n = record.read(buffer, 0, buffer.size)
                if (n < 0) { failed = true; onError("The microphone stopped (error $n)."); break }
                if (n == 0) continue
                val (chunks, level) = pipeline.push(buffer, n)
                onLevel(level)
                chunks.forEach(onChunk)
            }
            if (flushOnEnd && !failed) { pipeline.flush()?.let(onChunk); onStopped() }
        } catch (e: Exception) {
            onError("The microphone stopped: ${e.message}")
        } finally {
            running = false
            runCatching { record.stop() }
            record.release()
        }
    }
}
