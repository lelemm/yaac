package dev.yaac.android.core

import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.yaac.android.notify.CallService

/**
 * The process's one voice call (docs/07-ui.md#voice-calls-on-android). It belongs to the process, not to
 * the activity, so a call goes on with the screen off or another app in front; the foreground
 * service [CallService] (type microphone) holds it, with End in its notification.
 *
 * [CallMachine] decides; this carries its effects out: the peer ([CallPeer]), the audio
 * ([CallAudio]), `call.start` / `call.end` to the controller, the service. Main thread only;
 * the peer's callbacks are posted here.
 */
object CallManager {
    var phase by mutableStateOf(CallPhase.Idle); private set
    var error by mutableStateOf<String?>(null); private set
    /** [SystemClock.elapsedRealtime] when the call became active. */
    var startedAt by mutableStateOf<Long?>(null); private set
    var muted by mutableStateOf(false); private set
    var speaker by mutableStateOf(true); private set
    var output by mutableStateOf<CallOutput?>(null); private set
    var lines by mutableStateOf<List<CallLine>>(emptyList()); private set
    var key by mutableStateOf<String?>(null); private set
    var title by mutableStateOf(""); private set
    /**
     * The hub the call was started on (P404): the call stays with it when another hub is
     * selected, and the call bar then names it.
     */
    var hubKey by mutableStateOf<String?>(null); private set
    /** A failure stays on screen until dismissed. */
    var dismissed by mutableStateOf(true); private set

    private val main = Handler(Looper.getMainLooper())
    private val machine = CallMachine()
    private var app: Context? = null
    private var controller: Controller? = null
    private var peer: CallPeer? = null
    private var audio: CallAudio? = null
    private var fed: Pair<String?, String?>? = null
    private val tick = Runnable { apply(machine.tick(now())) }
    private val frames: (FrameUpdate) -> Unit = { update -> onFrame(update) }

    private fun now() = SystemClock.elapsedRealtime()

    /** Starts a call with the thread whose composer is [key]. RECORD_AUDIO must be granted. */
    fun start(context: Context, key: String, title: String) {
        if (phase.live) return
        val rt = YaacRuntime.selected ?: return
        val ctl = rt.controller
        app = context.applicationContext
        controller = ctl
        hubKey = rt.key
        this.key = key
        this.title = title
        lines = emptyList()
        muted = false
        dismissed = false
        val f = CallFrame.of(CallWire.callFor(key, ctl.store::view))
        fed = f.state to f.error
        ctl.store.listeners += frames
        apply(machine.start(key, f.state, now(), f.error, f.callId))
    }

    fun end() = apply(machine.end())

    fun dismiss() { dismissed = true }

    fun mute(on: Boolean) {
        muted = on
        peer?.setMuted(on)
        app?.let { CallService.update(it) }
    }

    fun useSpeaker(on: Boolean) {
        speaker = on
        output = audio?.route(on)
    }

    /** A controller is going away (Quit, Remove a hub, Retry): the call on it goes with it. */
    fun controllerStopping(ctl: Controller) { if (ctl === controller && phase.live) end() }

    /** The call's hub's name, when the phone has more than one hub. */
    val hubName: String? get() = hubKey?.takeIf { YaacRuntime.running.size > 1 }?.let { YaacRuntime.running[it]?.name }

    /** Each published update: this call's `call` object, when its thread's composer is shown. */
    @Suppress("UNUSED_PARAMETER")
    private fun onFrame(update: FrameUpdate) {
        val k = key ?: return
        val ctl = controller ?: return
        val call = CallWire.callFor(k, ctl.store::view) ?: return
        val f = CallFrame.of(call)
        lines = f.lines
        f.answer?.let { apply(machine.answer(f.callId, it, now())) }
        val state = f.state to f.error
        if (state != fed) { fed = state; apply(machine.controller(f.state, f.error)) }
    }

    private fun apply(effects: List<CallEffect>) {
        for (fx in effects) when (fx) {
            CallEffect.CreateOffer -> open()
            is CallEffect.SendStart -> controller?.send(CallWire.START, CallWire.startPayload(fx.key, fx.sdp, app?.let(VoiceLanguage::current)))
            is CallEffect.ApplyAnswer -> peer?.applyAnswer(fx.sdp)
            is CallEffect.SendEnd -> controller?.send(CallWire.END, CallWire.endPayload(fx.key))
            CallEffect.ClosePeer -> close()
        }
        sync()
    }

    private fun open() {
        val context = app ?: return
        val a = CallAudio(context).also { it.acquire() }
        audio = a
        speaker = CallRoute.defaultSpeaker(a.available())
        output = a.route(speaker)
        peer = CallPeer(context, object : CallPeer.Listener {
            override fun onOffer(sdp: String) { main.post { apply(machine.offer(sdp, now())) } }
            override fun onConnected() { main.post { apply(machine.connected(now())) } }
            override fun onLost() { main.post { apply(machine.lost(now())) } }
            override fun onFailed(why: String) { main.post { apply(machine.peerFailed(why)) } }
        }).also { it.start() }
        runCatching { context.startForegroundService(Intent(context, CallService::class.java)) }
    }

    private fun close() {
        peer?.close(); peer = null
        audio?.release(); audio = null
        output = null
        controller?.store?.listeners?.remove(frames)
        app?.let { it.stopService(Intent(it, CallService::class.java)) }
    }

    private fun sync() {
        phase = machine.phase
        error = machine.error
        startedAt = machine.startedAt
        main.removeCallbacks(tick)
        machine.deadline?.let { main.postDelayed(tick, (it - now()).coerceAtLeast(0)) }
        if (phase.live) app?.let { CallService.update(it) }
    }
}
