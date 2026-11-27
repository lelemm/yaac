package dev.yaac.android.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.unit.dp
import dev.yaac.android.core.VoiceRecorder
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonObject

/**
 * Voice input (docs/plan/voice.md) for one composer. The controller owns the dictation (its
 * `voice` object: state, partial words, error, audio format); this phone records the audio
 * itself and streams it with `composer.voiceAudio`. The words land in the field through the
 * controller's `insert`, at the caret; nothing is ever sent by itself.
 */
class VoiceUi(private val key: String, private val send: (String, Any?) -> Unit) {
    var recorder: VoiceRecorder? = null
    var level by mutableFloatStateOf(0f)
    /** A failure on this phone (permission, microphone) in words; the controller has its own. */
    var localError by mutableStateOf<String?>(null)

    fun record(rate: Int) {
        localError = null
        send("composer.voiceStart", mapOf("key" to key))
        val handler = android.os.Handler(android.os.Looper.getMainLooper())
        recorder = VoiceRecorder(
            rate,
            onChunk = { pcm -> send("composer.voiceAudio", mapOf("key" to key, "pcm" to pcm)) },
            onLevel = { v -> handler.post { level = v } },
            onError = { why ->
                send("composer.voiceCancel", mapOf("key" to key))
                handler.post { localError = why; recorder = null; level = 0f }
            },
            onStopped = { send("composer.voiceStop", mapOf("key" to key)) },
        ).also { it.start() }
    }

    /** Stop recording; the last audio goes, then `composer.voiceStop`. */
    fun stop() {
        val r = recorder
        if (r == null) send("composer.voiceStop", mapOf("key" to key)) else r.stop()
        recorder = null
        level = 0f
    }

    /** The controller ended the dictation (stopped, failed, cancelled elsewhere): stop recording. */
    fun drop() {
        recorder?.cancel()
        recorder = null
        level = 0f
    }

    /** Drop the dictation and its words (the cancel mark, or the composer going away). */
    fun cancel() {
        recorder?.cancel()
        recorder = null
        level = 0f
        send("composer.voiceCancel", mapOf("key" to key))
    }
}

/**
 * The words heard so far, shown in place at the caret until the final text replaces them
 * (docs/07-ui.md#voice-input). [text] is what to show there, with the space the controller will
 * put before the final words; the offsets map the field's text to what is drawn and back.
 */
data class Provisional(val caret: Int, val text: String) {
    fun toShown(offset: Int): Int = if (offset < caret) offset else offset + text.length
    fun toOriginal(offset: Int): Int = when {
        offset <= caret -> offset
        offset <= caret + text.length -> caret
        else -> offset - text.length
    }

    companion object {
        /** For [partial] words at [caret] of [field], or null when nothing is heard yet. */
        fun of(field: String, caret: Int, partial: String?): Provisional? {
            val words = partial?.trim()?.takeIf { it.isNotEmpty() } ?: return null
            val at = caret.coerceIn(0, field.length)
            val spaced = if (at > 0 && !field[at - 1].isWhitespace()) " $words" else words
            return Provisional(at, spaced)
        }
    }
}

/** What a tap on the mic does, by the controller's `voice.state` and whether we record. */
enum class VoiceTap { Start, Stop, None }

fun voiceTap(state: String, recording: Boolean): VoiceTap = when (state) {
    "starting", "listening" -> VoiceTap.Stop
    "stopping" -> VoiceTap.None
    else -> if (recording) VoiceTap.Stop else VoiceTap.Start
}

/** The mic in the composer's button row, left of × and history. Shown only with `voice`. */
@Composable
fun VoiceButton(voice: JsonObject, ui: VoiceUi, enabled: Boolean) {
    val c = LocalYaac.current
    val context = LocalContext.current
    val state = voice["state"].str
    val rate = voice["format"]["sampleRate"].int ?: 16_000
    val active = state == "starting" || state == "listening"
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) ui.record(rate) else ui.localError = "YAAC needs the microphone to take dictation. Allow it in the app's settings."
    }
    // The dictation is over on the controller's side (ended, failed, cancelled): stop recording.
    LaunchedEffect(state) { if (!active && state != "stopping") ui.drop() }
    // Leaving the thread drops the dictation (docs/07-ui.md#voice-input).
    DisposableEffect(ui) { onDispose { if (ui.recorder != null) ui.cancel() } }
    val tap = {
        when (voiceTap(state, ui.recorder != null)) {
            VoiceTap.Start -> {
                val granted = context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
                if (granted) ui.record(rate) else permission.launch(Manifest.permission.RECORD_AUDIO)
            }
            VoiceTap.Stop -> ui.stop()
            VoiceTap.None -> {}
        }
    }
    val description = when (state) {
        "listening" -> "Stop dictation"
        "starting" -> "Starting dictation, tap to stop"
        "stopping" -> "Finishing dictation"
        else -> "Dictate"
    }
    Box(
        Modifier.size(44.dp).clip(RoundedCornerShape(6.dp))
            .clickable(enabled = enabled && state != "stopping", onClickLabel = description) { tap() }
            .semantics { contentDescription = description }
            .alpha(if (enabled) 1f else 0.4f),
        contentAlignment = Alignment.Center,
    ) {
        if (active) {
            // Listening: a pulse behind the mark, swelling with the voice's loudness.
            val pulse = rememberInfiniteTransition(label = "voice")
            val beat by pulse.animateFloat(0.85f, 1f, infiniteRepeatable(tween(700), RepeatMode.Reverse), label = "beat")
            val swell = if (state == "listening") 0.75f + 0.5f * ui.level else 0.75f
            Box(Modifier.size(34.dp).scale(beat * swell).clip(CircleShape).background(c.failure.copy(alpha = 0.28f)))
            Icon(YIcon.StopFilled, c.failure, size = 16.dp)
        } else {
            Icon(YIcon.Mic, if (state == "stopping") c.muted else c.text, size = 21.dp, strokeWidth = 1.8f)
        }
    }
}

/** Under the button row while dictating: the elapsed time (the words are in the field), or why it failed. */
@Composable
fun VoiceStatus(voice: JsonObject, ui: VoiceUi) {
    val c = LocalYaac.current
    val type = LocalType.current
    val error = ui.localError ?: voice["error"].text
    val state = voice["state"].str
    val line = when (state) {
        "starting" -> "Starting dictation…"
        "listening" -> listOfNotNull("Listening…", voice["time"].text).joinToString(" ")
        "stopping" -> "Finishing…"
        else -> null
    }
    when {
        error != null -> T(error, type.small, c.failure, modifier = Modifier.padding(top = 2.dp, start = 4.dp))
        line != null -> androidx.compose.foundation.layout.Row(Modifier.padding(top = 2.dp, start = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            T(line, type.small.copy(fontStyle = FontStyle.Italic), c.muted, modifier = Modifier.weight(1f, fill = false))
            // The cancel mark: drop the dictation and its words.
            if (state == "starting" || state == "listening") TextAction("Cancel", true, style = type.small) { ui.cancel() }
        }
    }
}

/** Draws [p]'s words into the field at its caret, in [color] and italic; the text itself is untouched. */
class ProvisionalWords(private val p: Provisional, private val color: androidx.compose.ui.graphics.Color) : androidx.compose.ui.text.input.VisualTransformation {
    override fun filter(text: androidx.compose.ui.text.AnnotatedString): androidx.compose.ui.text.input.TransformedText {
        val at = p.caret.coerceIn(0, text.length)
        val shown = androidx.compose.ui.text.buildAnnotatedString {
            append(text.subSequence(0, at))
            pushStyle(androidx.compose.ui.text.SpanStyle(color = color, fontStyle = FontStyle.Italic))
            append(p.text)
            pop()
            append(text.subSequence(at, text.length))
        }
        val mapping = object : androidx.compose.ui.text.input.OffsetMapping {
            override fun originalToTransformed(offset: Int) = p.copy(caret = at).toShown(offset)
            override fun transformedToOriginal(offset: Int) = p.copy(caret = at).toOriginal(offset)
        }
        return androidx.compose.ui.text.input.TransformedText(shown, mapping)
    }

    override fun equals(other: Any?) = other is ProvisionalWords && other.p == p && other.color == color
    override fun hashCode() = p.hashCode() * 31 + color.hashCode()
}
