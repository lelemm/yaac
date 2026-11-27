package dev.yaac.android.ui

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.SystemClock
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import dev.yaac.android.core.CallFollow
import dev.yaac.android.core.CallLine
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.unit.dp
import dev.yaac.android.core.CallConsent
import dev.yaac.android.core.CallFrame
import dev.yaac.android.core.CallManager
import dev.yaac.android.core.CallOutput
import dev.yaac.android.core.CallPhase
import dev.yaac.android.core.CallTime
import dev.yaac.android.core.CallWire
import kotlinx.coroutines.delay
import kotlinx.serialization.json.JsonObject

/** The open thread's title, for the call's notification and bar. */
val LocalThreadTitle = compositionLocalOf { "" }

private fun consent(context: Context): CallConsent {
    val prefs = context.getSharedPreferences("yaac.call", Context.MODE_PRIVATE)
    return CallConsent({ prefs.getBoolean(it, false) }, { k, v -> prefs.edit().putBoolean(k, v).apply() })
}

/**
 * The call button in the composer's button row, beside the mic (docs/07-ui.md#voice-calls-on-android):
 * shown only when the composer has `call` (the thread's `voiceCall` capability), never by
 * provider. A tap asks for consent the first time, then the microphone, then calls; during this
 * thread's call it hangs up.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CallButton(composer: JsonObject, key: String, editable: Boolean) {
    if (!CallWire.offered(composer)) return
    val c = LocalYaac.current
    val context = LocalContext.current
    val title = LocalThreadTitle.current
    val frame = CallFrame.of(composer[CallWire.VIEW] as? JsonObject)
    val ours = CallManager.phase.live && CallManager.key == key && CallManager.hubKey == dev.yaac.android.core.YaacRuntime.selectedKey
    val elsewhere = CallManager.phase.live && !ours
    var asking by remember { mutableStateOf(false) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) CallManager.start(context, key, title)
        else android.widget.Toast.makeText(context, "YAAC needs the microphone for a call. Allow it in the app's settings.", android.widget.Toast.LENGTH_LONG).show()
    }
    fun call() {
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) CallManager.start(context, key, title)
        else permission.launch(Manifest.permission.RECORD_AUDIO)
    }
    val enabled = ours || (editable && frame.enabled && !elsewhere)
    val description = when {
        ours -> "End the call"
        elsewhere -> "Another call is running"
        !frame.enabled -> frame.detail ?: "Call this thread"
        else -> "Call this thread: talk, and the agent works here"
    }
    Box(
        Modifier.size(44.dp).clip(RoundedCornerShape(6.dp))
            .clickable(enabled = enabled, onClickLabel = description) {
                when {
                    ours -> CallManager.end()
                    consent(context).needed() -> asking = true
                    else -> call()
                }
            }
            .semantics { contentDescription = description }
            .alpha(if (enabled) 1f else 0.4f),
        contentAlignment = Alignment.Center,
    ) {
        Icon(YIcon.Phone, if (ours) c.failure else c.text, size = 19.dp, strokeWidth = 1.8f)
    }
    if (asking) {
        val type = LocalType.current
        val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(onDismissRequest = { asking = false }, sheetState = state, containerColor = c.raised, contentColor = c.text,
            tonalElevation = 0.dp, scrimColor = Color.Black.copy(alpha = 0.45f)) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 24.dp)) {
                T("Talk with this thread", type.title)
                Spacer(Modifier.height(10.dp))
                T("The agent works in this thread while you talk: what you ask for runs here as ordinary turns, with this thread's approvals.", type.ui)
                Spacer(Modifier.height(6.dp))
                T("The call's voice uses your plan's quota.", type.ui, c.muted)
                Spacer(Modifier.height(18.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Spacer(Modifier.weight(1f))
                    TextAction("Not now") { asking = false }
                    Spacer(Modifier.width(8.dp))
                    PrimaryButton("Call") { consent(context).give(); asking = false; call() }
                }
            }
        }
    }
}

/**
 * The call bar above the composer while a call lives, in whichever thread is open: the state,
 * the clock, the transcript (each line in full, scrolling), Mute, the speaker and End. A failure stays until
 * dismissed.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun CallBar(openKey: String?) {
    val phase = CallManager.phase
    val failed = phase == CallPhase.Failed && !CallManager.dismissed
    if (!phase.live && !failed) return
    val c = LocalYaac.current
    val type = LocalType.current
    var overflowing by remember { mutableStateOf(false) }
    var expanded by remember { mutableStateOf(false) }
    if (expanded && !failed) TranscriptSheet { expanded = false }
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp).clip(RoundedCornerShape(8.dp)).background(c.input).padding(horizontal = 12.dp, vertical = 8.dp)) {
        if (failed) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(YIcon.Phone, c.failure, size = 16.dp, strokeWidth = 1.8f)
                Spacer(Modifier.width(8.dp))
                T(CallManager.error ?: "The call failed.", type.ui, c.failure, modifier = Modifier.weight(1f))
                TextAction("Dismiss") { CallManager.dismiss() }
            }
            return@Column
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(YIcon.Phone, if (phase == CallPhase.Active) c.success else c.muted, size = 16.dp, strokeWidth = 1.8f)
            Spacer(Modifier.width(8.dp))
            val label = when (phase) {
                CallPhase.Active -> if (CallManager.muted) "In a call · muted" else "In a call"
                else -> "Connecting…"
            }
            // The call's thread, when another one is open.
            // Its hub too when another hub is selected (P404).
            val elsewhere = CallManager.hubKey != dev.yaac.android.core.YaacRuntime.selectedKey
            val where = CallManager.title.takeIf { (elsewhere || CallManager.key != openKey) && it.isNotBlank() }
            val hub = CallManager.hubName?.takeIf { elsewhere }
            T(listOfNotNull(label, hub, where).joinToString(" · "), type.ui, maxLines = 1, modifier = Modifier.weight(1f))
            CallManager.startedAt?.let { Clock(it) }
            if (overflowing || expanded) IconButton(YIcon.ArrowUp, "Show the whole call transcript", tint = c.muted, size = 18.dp, strokeWidth = 1.8f) { expanded = true }
        }
        // Every line in full, never cut: a bounded area with its own scroll that keeps the newest
        // line in view unless the user scrolled up, and a sheet for the whole transcript.
        val lines = CallManager.lines
        if (lines.isNotEmpty()) {
            val share = if (WindowInsets.isImeVisible) 0.2f else 0.4f
            val most = (LocalConfiguration.current.screenHeightDp * share).dp
            CallLines(lines, Modifier.fillMaxWidth().padding(top = 4.dp).heightIn(max = most)) { overflowing = it }
        }
        Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            val muted = CallManager.muted
            IconButton(if (muted) YIcon.MicOff else YIcon.Mic, if (muted) "Unmute" else "Mute", tint = if (muted) c.failure else c.text, size = 20.dp, strokeWidth = 1.8f) {
                CallManager.mute(!muted)
            }
            val output = CallManager.output
            val (icon, what) = when (output) {
                CallOutput.Speaker -> YIcon.Speaker to "Speaker on: tap for the earpiece or headset"
                CallOutput.Earpiece -> YIcon.Ear to "Earpiece: tap for the speaker"
                null -> YIcon.Speaker to "Speaker"
                else -> YIcon.Headphones to "Headset: tap for the speaker"
            }
            IconButton(icon, what, size = 20.dp, strokeWidth = 1.8f) { CallManager.useSpeaker(output != CallOutput.Speaker) }
            Spacer(Modifier.weight(1f))
            Row(Modifier.clip(RoundedCornerShape(18.dp)).background(c.failure).clickable(onClickLabel = "End the call") { CallManager.end() }
                .semantics { contentDescription = "End the call" }.padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(YIcon.Phone, Color.White, size = 15.dp, strokeWidth = 2f)
                Spacer(Modifier.width(6.dp))
                T("End", type.ui, Color.White)
            }
        }
    }
}

/**
 * The call's transcript lines, each in full (wrapped, never ellipsized), in a scroll of their
 * own that follows the newest line until the user scrolls up ([CallFollow]). [onOverflow] says
 * whether they no longer fit.
 */
@Composable
private fun CallLines(lines: List<CallLine>, modifier: Modifier, onOverflow: (Boolean) -> Unit = {}) {
    val c = LocalYaac.current
    val type = LocalType.current
    val scroll = rememberScrollState()
    val slack = with(LocalDensity.current) { 8.dp.roundToPx() }
    var following by remember { mutableStateOf(true) }
    LaunchedEffect(scroll) {
        var prev = CallFollow.Pos(scroll.value, scroll.maxValue)
        snapshotFlow { CallFollow.Pos(scroll.value, scroll.maxValue) }.collect { now ->
            following = CallFollow.next(following, prev, now, slack)
            prev = now
            onOverflow(now.max > 0)
        }
    }
    LaunchedEffect(scroll.maxValue, following) { if (following) scroll.scrollTo(scroll.maxValue) }
    Column(modifier.verticalScroll(scroll), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        for (l in lines) T((if (l.user) "You: " else "Agent: ") + l.text,
            if (l.final) type.small else type.small.copy(fontStyle = FontStyle.Italic), if (l.user) c.text else c.muted)
    }
}

/** The whole call transcript in a sheet, live, following the newest line like the bar. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TranscriptSheet(onDismiss: () -> Unit) {
    val c = LocalYaac.current
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = c.raised, contentColor = c.text,
        tonalElevation = 0.dp, scrimColor = Color.Black.copy(alpha = 0.45f)) {
        Column(Modifier.fillMaxWidth().fillMaxHeight().navigationBarsPadding().padding(horizontal = 20.dp).padding(bottom = 16.dp)) {
            T("Call transcript", LocalType.current.title)
            Spacer(Modifier.height(10.dp))
            CallLines(CallManager.lines, Modifier.fillMaxWidth().weight(1f))
        }
    }
}

/** The call's elapsed time, redrawn once a second only while shown. */
@Composable
private fun Clock(startedAt: Long) {
    var now by remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }
    LaunchedEffect(startedAt) {
        while (true) {
            now = SystemClock.elapsedRealtime()
            delay(1000 - (now - startedAt) % 1000)
        }
    }
    T(CallTime.elapsed(now - startedAt), LocalType.current.ui, LocalYaac.current.muted)
}
