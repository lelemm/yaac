package dev.yaac.android.core

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/*
 * A voice call with a thread (docs/plan/voice-call.md, docs/07-ui.md#voice-calls-on-android): this phone is
 * the WebRTC peer, the controller relays the offer and the answer. Everything here is plain
 * Kotlin, unit-tested in CallModelTest; `CallPeer`, `CallAudio` and `CallManager` do the Android
 * side.
 */

/**
 * Every name the call uses on the controller's wire (docs/plan/voice-call.md § Window
 * controller), in one place. Nothing here names a provider: the call button follows the
 * capability, which renderers see as the presence of `composer.call`.
 */
object CallWire {
    /** The composer frame's call object; present iff the thread offers a call. */
    const val VIEW = "call"
    const val START = "composer.callStart"
    const val END = "composer.callEnd"

    fun offered(composer: JsonElement?): Boolean = composer[VIEW] is JsonObject

    /**
     * The call object of the thread whose composer is [key], or null when the open composer is
     * another thread's: then this call's state is unknown, not idle.
     */
    fun callFor(key: String, view: (String) -> JsonElement?): JsonObject? {
        val composer = view("composer")
        return if (composer["key"].text == key) composer[VIEW].obj else null
    }

    fun startPayload(key: String, sdp: String) = mapOf("key" to key, "sdpOffer" to sdp)
    fun endPayload(key: String) = mapOf("key" to key)
}

/** One utterance of the call: the user's or the agent's; [final] once it is complete. */
data class CallLine(val user: Boolean, val text: String, val final: Boolean = true)

/**
 * The composer's `call` object: state (idle, starting, connecting, active, ending, error), the
 * call's id, the SDP answer once it arrived, the last error, the transcript, a status line and
 * the call button's enabled state and reason.
 */
data class CallFrame(
    val state: String,
    val callId: String?,
    val answer: String?,
    val error: String?,
    val lines: List<CallLine>,
    val status: String? = null,
    val enabled: Boolean = true,
    val detail: String? = null,
) {
    companion object {
        const val MAX_LINES = 8

        fun of(o: JsonObject?): CallFrame {
            val lines = o?.get("transcript").objects.mapNotNull { l ->
                val text = l["text"].text?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
                val final = l["final"].flag(true)
                when (l["role"].str) {
                    "user" -> CallLine(true, text, final)
                    "assistant" -> CallLine(false, text, final)
                    else -> null
                }
            }
            val button = o?.get("button").obj
            return CallFrame(
                o?.get("state").text ?: "idle", o?.get("callId").text, o?.get("answer").text, o?.get("error").text,
                lines.takeLast(MAX_LINES), o?.get("status").text, button?.get("enabled").flag(true) ?: true, button?.get("detail").text,
            )
        }
    }
}

enum class CallPhase {
    Idle,
    /** The peer is being made and its ICE candidates gathered. */
    Preparing,
    /** `call.start` sent; waiting for the answer. */
    Offering,
    /** The answer is applied; waiting for the event channel to open. */
    Connecting,
    Active,
    Ended,
    Failed;

    val live: Boolean get() = this == Preparing || this == Offering || this == Connecting || this == Active
}

/** What the Android side must do after an event. */
sealed interface CallEffect {
    data object CreateOffer : CallEffect
    data class SendStart(val key: String, val sdp: String) : CallEffect
    data class ApplyAnswer(val sdp: String) : CallEffect
    data class SendEnd(val key: String) : CallEffect
    data object ClosePeer : CallEffect
}

/**
 * The phone's side of one call. Events come from the user, the peer and the controller's
 * frame; each returns the effects to carry out. Times are milliseconds on any monotonic clock.
 * `call.end` goes only once `call.start` has gone; the peer is closed exactly once per call.
 */
class CallMachine {
    var key: String? = null; private set
    var phase = CallPhase.Idle; private set
    var error: String? = null; private set
    /** When the call became active (the clock in the call bar). */
    var startedAt: Long? = null; private set
    /** When the current stage gives up, or null. */
    var deadline: Long? = null; private set

    private var sent = false
    private var lostAt: Long? = null
    /** The controller's state when this call started: a terminal state equal to it is stale. */
    private var baseline: Pair<String?, String?>? = null
    private var staleCallId: String? = null

    /** [lastCallId] is the frame's `call.callId` at the start: its answer, if still shown, is not this call's. */
    fun start(key: String, controllerState: String?, now: Long, controllerError: String? = null, lastCallId: String? = null): List<CallEffect> {
        if (phase.live) return emptyList()
        staleCallId = lastCallId
        this.key = key
        phase = CallPhase.Preparing
        error = null; startedAt = null; lostAt = null; sent = false
        baseline = controllerState to controllerError
        deadline = now + PREPARE_MS
        return listOf(CallEffect.CreateOffer)
    }

    fun offer(sdp: String, now: Long): List<CallEffect> {
        if (phase != CallPhase.Preparing) return emptyList()
        CallSdp.checkOffer(sdp)?.let { return fail(it) }
        phase = CallPhase.Offering
        sent = true
        deadline = now + ANSWER_MS
        return listOf(CallEffect.SendStart(key!!, sdp))
    }

    /** The frame's `call.answer`: applied once per call (the frame keeps it while the call lives). */
    fun answer(callId: String?, sdp: String, now: Long): List<CallEffect> {
        if (phase != CallPhase.Offering) return emptyList()
        if (callId != null && callId == staleCallId) return emptyList()
        CallSdp.checkAnswer(sdp)?.let { return fail(it) }
        phase = CallPhase.Connecting
        deadline = now + CONNECT_MS
        return listOf(CallEffect.ApplyAnswer(sdp))
    }

    /** The event channel is open (first time), or the network came back. */
    fun connected(now: Long): List<CallEffect> {
        if (phase != CallPhase.Connecting && phase != CallPhase.Active) return emptyList()
        if (phase == CallPhase.Connecting) startedAt = now
        phase = CallPhase.Active
        lostAt = null
        deadline = null
        return emptyList()
    }

    /** The peer lost its path (ICE disconnected): survived for [LOST_MS]. */
    fun lost(now: Long): List<CallEffect> {
        if (phase != CallPhase.Active || lostAt != null) return emptyList()
        lostAt = now
        deadline = now + LOST_MS
        return emptyList()
    }

    fun peerFailed(why: String): List<CallEffect> = if (phase.live) fail(why) else emptyList()

    /**
     * The controller's `call.state` changed. The peer closes when the state leaves starting,
     * connecting and active; a state equal to the one the call started from is the last
     * call's, republished, until the controller has moved.
     */
    fun controller(state: String?, error: String?): List<CallEffect> {
        if (!phase.live || !sent) return emptyList()
        if (state in LIVE_STATES) { baseline = null; return emptyList() }
        if (baseline == (state to error)) return emptyList()
        return if (state == "error") close(CallPhase.Failed, error ?: "The call failed.") else close(CallPhase.Ended, null)
    }

    /** The user hung up (the call bar, the notification). */
    fun end(): List<CallEffect> = if (phase.live) hangUp(CallPhase.Ended, null) else emptyList()

    fun tick(now: Long): List<CallEffect> {
        val d = deadline ?: return emptyList()
        if (!phase.live || now < d) return emptyList()
        return fail(when (phase) {
            CallPhase.Preparing -> "The phone could not prepare the call."
            CallPhase.Offering -> "The call did not get an answer."
            CallPhase.Connecting -> "The call could not connect."
            else -> "The call lost its connection."
        })
    }

    private fun fail(why: String) = hangUp(CallPhase.Failed, why)

    /** Ends from this side: the controller hears of it only if it was told of the call. */
    private fun hangUp(to: CallPhase, why: String?): List<CallEffect> {
        val tell = sent
        val k = key
        close(to, why)
        return if (tell && k != null) listOf(CallEffect.SendEnd(k), CallEffect.ClosePeer) else listOf(CallEffect.ClosePeer)
    }

    private fun close(to: CallPhase, why: String?): List<CallEffect> {
        phase = to
        error = why
        deadline = null
        lostAt = null
        sent = false
        return listOf(CallEffect.ClosePeer)
    }

    companion object {
        private val LIVE_STATES = setOf("starting", "connecting", "active")
        /** Making the peer and gathering candidates (Codex's own host allows 15 s for the gathering). */
        const val PREPARE_MS = 20_000L
        /** From `call.start` to the answer: the hub, yaacd and the provider each take a hop. */
        const val ANSWER_MS = 30_000L
        /** From the answer to the open event channel (Codex's host: 15 s). */
        const val CONNECT_MS = 15_000L
        /** How long an active call survives a lost network path. */
        const val LOST_MS = 10_000L
    }
}

/** Checks on the SDP that crosses the controller (bounded as Codex bounds it: 64 KiB). */
object CallSdp {
    const val MAX_BYTES = 64 * 1024

    fun candidates(sdp: String): Int = sdp.lineSequence().count { it.trimEnd().startsWith("a=candidate:") }

    /** Why the offer cannot be sent, or null: audio, the event channel, and gathered candidates. */
    fun checkOffer(sdp: String): String? = when {
        sdp.isBlank() -> "The phone made no call offer."
        sdp.toByteArray().size > MAX_BYTES -> "The call offer is too large."
        sdp.lineSequence().none { it.startsWith("m=audio ") } -> "The call offer has no audio."
        sdp.lineSequence().none { it.startsWith("m=application ") } -> "The call offer has no event channel."
        candidates(sdp) == 0 -> "The phone found no network address for the call."
        else -> null
    }

    fun checkAnswer(sdp: String): String? = when {
        !sdp.startsWith("v=0") -> "The call's answer is not a session description."
        sdp.toByteArray().size > MAX_BYTES -> "The call's answer is too large."
        else -> null
    }
}

/** Where the call's sound goes. */
enum class CallOutput { Earpiece, Speaker, Wired, Usb, Bluetooth }

object CallRoute {
    private val headsets = listOf(CallOutput.Wired, CallOutput.Usb, CallOutput.Bluetooth)

    /** The speaker, unless a headset is connected: the phone is usually held in view, not to the ear. */
    fun defaultSpeaker(available: List<CallOutput>) = available.none { it in headsets }

    /** The device for [speaker] on or off: off prefers a headset, then the earpiece. */
    fun pick(available: List<CallOutput>, speaker: Boolean): CallOutput? {
        if (speaker && CallOutput.Speaker in available) return CallOutput.Speaker
        return headsets.firstOrNull { it in available }
            ?: CallOutput.Earpiece.takeIf { it in available }
            ?: available.firstOrNull()
    }
}

object CallTime {
    fun elapsed(ms: Long): String {
        val s = (ms.coerceAtLeast(0) / 1000)
        val h = s / 3600; val m = (s % 3600) / 60; val sec = s % 60
        return if (h > 0) String.format(java.util.Locale.ROOT, "%d:%02d:%02d", h, m, sec) else String.format(java.util.Locale.ROOT, "%d:%02d", m, sec)
    }
}

/** The one-time consent to calls ("the agent works while you talk; it uses your plan"). */
class CallConsent(private val read: (String) -> Boolean, private val write: (String, Boolean) -> Unit) {
    fun needed() = !read(KEY)
    fun give() = write(KEY, true)

    companion object { const val KEY = "voiceCall.consented" }
}
