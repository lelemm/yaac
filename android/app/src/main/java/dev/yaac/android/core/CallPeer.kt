package dev.yaac.android.core

import android.content.Context
import org.webrtc.AudioTrack
import org.webrtc.DataChannel
import org.webrtc.IceCandidate
import org.webrtc.MediaConstraints
import org.webrtc.MediaStream
import org.webrtc.PeerConnection
import org.webrtc.PeerConnectionFactory
import org.webrtc.RtpTransceiver
import org.webrtc.SdpObserver
import org.webrtc.SessionDescription
import org.webrtc.audio.JavaAudioDeviceModule
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/**
 * The phone's WebRTC peer for a voice call (docs/plan/voice-call.md), shaped as Codex's own
 * voice host shapes it (openai/codex `voice-host/src/transport.rs`, rust-v0.172.0): one Opus
 * audio track sent and received, the `oai-events` data channel with default options, no ICE
 * servers, and a non-trickle offer taken once ICE gathering is complete. The call is ready when
 * the data channel opens.
 *
 * Every libwebrtc call runs on this peer's own thread, never the UI thread (invariant 7);
 * [Listener] methods are called on that thread too.
 */
class CallPeer(context: Context, private val listener: Listener) {
    interface Listener {
        fun onOffer(sdp: String)
        /** The event channel opened, or the network path came back. */
        fun onConnected()
        /** The network path is lost for now (ICE disconnected). */
        fun onLost()
        fun onFailed(why: String)
    }

    private val app = context.applicationContext
    private val thread = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "yaac-call").apply { isDaemon = true } }
    private var adm: JavaAudioDeviceModule? = null
    private var factory: PeerConnectionFactory? = null
    private var pc: PeerConnection? = null
    private var channel: DataChannel? = null
    private var track: AudioTrack? = null
    private var source: org.webrtc.AudioSource? = null
    private var gatherTimeout: ScheduledFuture<*>? = null
    @Volatile private var closed = false
    private var offered = false

    /** Makes the peer, creates the offer and reports it through [Listener.onOffer] once gathered. */
    fun start() = post { runCatching { open() }.onFailure { fail("The call could not start on this phone: ${it.message}") } }

    fun applyAnswer(sdp: String) = post {
        val p = pc ?: return@post
        p.setRemoteDescription(object : Sdp() {
            override fun onSetFailure(error: String?) = fail("The call's answer was refused: $error")
        }, SessionDescription(SessionDescription.Type.ANSWER, sdp))
    }

    /** Mute sends silence: the track stays, so the call does not renegotiate. */
    fun setMuted(muted: Boolean) = post { track?.setEnabled(!muted) }

    fun close() {
        if (closed) return
        closed = true
        post {
            gatherTimeout?.cancel(false)
            runCatching { channel?.unregisterObserver(); channel?.close(); channel?.dispose() }
            runCatching { pc?.close(); pc?.dispose() }
            runCatching { track?.dispose(); source?.dispose() }
            runCatching { factory?.dispose() }
            runCatching { adm?.release() }
            channel = null; pc = null; track = null; source = null; factory = null; adm = null
        }
        thread.shutdown()
    }

    /** Runs [block] on the peer's thread; nothing runs once the peer is closed and its thread gone. */
    private fun post(block: () -> Unit) {
        runCatching { thread.execute(block) }
    }

    private fun open() {
        initialize(app)
        val audioError = { what: String -> fail("The call's audio stopped: $what") }
        val module = JavaAudioDeviceModule.builder(app)
            // The phone's own echo canceller and noise suppressor where it has them; WebRTC's
            // audio processing (echo cancellation, noise suppression, gain) runs as well.
            .setUseHardwareAcousticEchoCanceler(true)
            .setUseHardwareNoiseSuppressor(true)
            .setAudioRecordErrorCallback(object : JavaAudioDeviceModule.AudioRecordErrorCallback {
                override fun onWebRtcAudioRecordInitError(errorMessage: String?) = audioError("the microphone could not open ($errorMessage)")
                override fun onWebRtcAudioRecordStartError(errorCode: JavaAudioDeviceModule.AudioRecordStartErrorCode?, errorMessage: String?) = audioError("the microphone could not start ($errorMessage)")
                override fun onWebRtcAudioRecordError(errorMessage: String?) = audioError("the microphone failed ($errorMessage)")
            })
            .setAudioTrackErrorCallback(object : JavaAudioDeviceModule.AudioTrackErrorCallback {
                override fun onWebRtcAudioTrackInitError(errorMessage: String?) = audioError("the speaker could not open ($errorMessage)")
                override fun onWebRtcAudioTrackStartError(errorCode: JavaAudioDeviceModule.AudioTrackStartErrorCode?, errorMessage: String?) = audioError("the speaker could not start ($errorMessage)")
                override fun onWebRtcAudioTrackError(errorMessage: String?) = audioError("the speaker failed ($errorMessage)")
            })
            .createAudioDeviceModule()
        adm = module
        val f = PeerConnectionFactory.builder().setAudioDeviceModule(module).createPeerConnectionFactory()
        factory = f
        val config = PeerConnection.RTCConfiguration(emptyList()).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
            rtcpMuxPolicy = PeerConnection.RtcpMuxPolicy.REQUIRE
            continualGatheringPolicy = PeerConnection.ContinualGatheringPolicy.GATHER_ONCE
            tcpCandidatePolicy = PeerConnection.TcpCandidatePolicy.ENABLED
        }
        val p = f.createPeerConnection(config, observer) ?: error("no peer connection")
        pc = p
        val constraints = MediaConstraints().apply {
            for (k in listOf("googEchoCancellation", "googAutoGainControl", "googNoiseSuppression", "googHighpassFilter"))
                mandatory += MediaConstraints.KeyValuePair(k, "true")
        }
        val s = f.createAudioSource(constraints)
        source = s
        val t = f.createAudioTrack("yaac-mic", s)
        track = t
        p.addTransceiver(t, RtpTransceiver.RtpTransceiverInit(RtpTransceiver.RtpTransceiverDirection.SEND_RECV, listOf("yaac")))
        val dc = p.createDataChannel("oai-events", DataChannel.Init()) ?: error("no event channel")
        channel = dc
        dc.registerObserver(object : DataChannel.Observer {
            override fun onBufferedAmountChange(previousAmount: Long) {}
            // The sideband (Codex's websocket) carries the events; nothing is read here.
            override fun onMessage(buffer: DataChannel.Buffer?) {}
            override fun onStateChange() {
                when (dc.state()) {
                    DataChannel.State.OPEN -> if (!closed) listener.onConnected()
                    DataChannel.State.CLOSED -> fail("The call's event channel closed.")
                    else -> {}
                }
            }
        })
        p.createOffer(object : Sdp() {
            override fun onCreateSuccess(desc: SessionDescription?) {
                val d = desc ?: return fail("The phone made no call offer.")
                post {
                    pc?.setLocalDescription(object : Sdp() {
                        override fun onSetFailure(error: String?) = fail("The call offer was refused: $error")
                    }, d)
                }
            }
            override fun onCreateFailure(error: String?) = fail("The phone made no call offer: $error")
        }, MediaConstraints())
        // Non-trickle: the offer goes once gathering completes. If it never says so, what was
        // gathered by then is used (the offer check refuses one with no candidate).
        gatherTimeout = runCatching { thread.schedule({ sendOffer() }, GATHER_MS, TimeUnit.MILLISECONDS) }.getOrNull()
    }

    private fun sendOffer() {
        if (offered || closed) return
        val sdp = pc?.localDescription?.description ?: return fail("The phone could not gather the call's addresses.")
        offered = true
        gatherTimeout?.cancel(false)
        listener.onOffer(sdp)
    }

    private fun fail(why: String) {
        if (!closed) listener.onFailed(why)
    }

    private val observer = object : PeerConnection.Observer {
        override fun onSignalingChange(state: PeerConnection.SignalingState?) {}
        override fun onIceConnectionChange(state: PeerConnection.IceConnectionState?) {
            when (state) {
                PeerConnection.IceConnectionState.DISCONNECTED -> if (!closed) listener.onLost()
                PeerConnection.IceConnectionState.CONNECTED, PeerConnection.IceConnectionState.COMPLETED ->
                    if (!closed && channel?.state() == DataChannel.State.OPEN) listener.onConnected()
                PeerConnection.IceConnectionState.FAILED -> fail("No network path to the call.")
                else -> {}
            }
        }
        override fun onIceConnectionReceivingChange(receiving: Boolean) {}
        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState?) {
            if (state == PeerConnection.IceGatheringState.COMPLETE) post { sendOffer() }
        }
        override fun onIceCandidate(candidate: IceCandidate?) {}
        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>?) {}
        override fun onAddStream(stream: MediaStream?) {}
        override fun onRemoveStream(stream: MediaStream?) {}
        override fun onDataChannel(channel: DataChannel?) {}
        override fun onRenegotiationNeeded() {}
    }

    private abstract class Sdp : SdpObserver {
        override fun onCreateSuccess(desc: SessionDescription?) {}
        override fun onSetSuccess() {}
        override fun onCreateFailure(error: String?) {}
        override fun onSetFailure(error: String?) {}
    }

    companion object {
        /** Codex's voice host waits 15 s for the gathering. */
        const val GATHER_MS = 15_000L
        @Volatile private var initialized = false

        @Synchronized private fun initialize(context: Context) {
            if (initialized) return
            PeerConnectionFactory.initialize(PeerConnectionFactory.InitializationOptions.builder(context).createInitializationOptions())
            initialized = true
        }
    }
}
