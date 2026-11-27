package dev.yaac.android.ui.transcript

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.yaac.android.core.LeftOff
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.objects
import dev.yaac.android.core.scalar
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import dev.yaac.android.ui.LocalType
import dev.yaac.android.ui.LocalYaac
import dev.yaac.android.ui.T
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.debounce
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** What ThreadScreen needs from the marker while it draws the transcript (all snapshot state). */
class UnreadState {
    /** The row the divider is drawn above, and its count. */
    var dividerBefore by mutableStateOf<String?>(null)
        internal set
    var count by mutableIntStateOf(0)
        internal set
    /** Resolving or jumping: the end is not followed and the restore anchor waits. */
    var busy by mutableStateOf(false)
        internal set
    var jumping by mutableStateOf(false)
        internal set
    /** Bumped when the divider took the place: KeepAnchor drops the anchor it was holding. */
    var tookPlace by mutableIntStateOf(0)
        internal set
    internal var visit: UnreadVisit? = null

    internal fun sync() {
        val v = visit
        dividerBefore = v?.dividerBefore
        count = v?.count ?: 0
        busy = v?.busy == true
        jumping = v?.jumping == true
    }

    /** Scrolled past it to the end, or a message was sent. */
    fun clear() { visit?.clear(); sync() }
}

private fun JsonObject.unreadRow() = UnreadRow(this["id"].str, this["actionId"].text, this["turnId"].text, this["kind"].text)

/**
 * The "Unread from here" marker of one thread (docs/07-ui.md#unread-from-here-on-android): reads
 * the seen point kept on the phone when the thread is opened (and again when the app returns to
 * the foreground), places the divider once, and records where the user stops reading.
 * [onPinned] stops following the end (the divider took the place).
 */
@OptIn(FlowPreview::class)
@Composable
fun rememberUnread(transcript: JsonElement?, rows: List<JsonObject>, listState: LazyListState, onPinned: () -> Unit,
    send: (String, Any?) -> Unit): UnreadState {
    val threadId = transcript["threadId"].str
    // Seen points are kept per hub (P404): thread ids are unique per target, not across hubs.
    val seenKey = dev.yaac.android.core.LocalHubKey.current + threadId
    val context = LocalContext.current
    val density = LocalDensity.current.density
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current.lifecycle
    var visitNo by remember(threadId) { mutableIntStateOf(0) }
    val state = remember(threadId) { UnreadState() }
    val currentRows by rememberUpdatedState(rows)
    val enabled = remember(threadId, visitNo) { LeftOff.enabled(context) }
    remember(threadId, visitNo) {
        state.visit = UnreadVisit(if (threadId.isEmpty()) null else LeftOff.seen(context, seenKey), enabled)
        state.sync()
        Unit
    }

    // Where the user is now: the end when it is in view, else the row at the bottom of the viewport.
    fun current(): SeenPoint? {
        val rs = currentRows
        if (rs.isEmpty()) return null
        val info = listState.layoutInfo
        val index = if (!listState.canScrollForward) rs.lastIndex
            else info.visibleItemsInfo.lastOrNull { it.offset + minOf(it.size, (40 * density).toInt()) <= info.viewportEndOffset }?.index
                ?: listState.firstVisibleItemIndex
        return Unread.seenAt(rs.map { it.unreadRow() }, index)
    }
    fun record() {
        if (threadId.isEmpty() || state.busy || !LeftOff.enabled(context)) return
        current()?.let { LeftOff.record(context, seenKey, it) }
    }

    // Place the divider once the page is in; follow it as rows fold.
    val page = transcript["page"]
    val ready = rows.isNotEmpty() && !transcript["loading"].flag() && !page["loading"].flag()
    val focused = transcript["focusToken"].scalar != null
    LaunchedEffect(threadId, visitNo, rows, ready) {
        val visit = state.visit ?: return@LaunchedEffect
        val unreadRows = rows.map { it.unreadRow() }
        val prompts = transcript["prompts"].objects.mapNotNull { it["id"].text }.toSet()
        val effect = visit.step(unreadRows, ready, prompts, page["older"].text != null, focused)
        state.sync()
        act(effect, state, rows, listState, threadId, onPinned, send)
    }
    LaunchedEffect(threadId, visitNo, state.jumping) {
        if (!state.jumping) return@LaunchedEffect
        delay(8_000)
        val effect = state.visit?.giveUp()
        state.sync()
        act(effect, state, currentRows, listState, threadId, onPinned, send)
    }

    // Record the seen point as the user reads (not while the divider is being placed).
    LaunchedEffect(threadId, visitNo, state.busy) {
        if (state.busy || !enabled) return@LaunchedEffect
        snapshotFlow { listState.firstVisibleItemIndex to listState.firstVisibleItemScrollOffset to currentRows.size }
            .debounce(300)
            .collect { record() }
    }
    // Leaving the thread or the app writes it; returning to the app is a new visit.
    DisposableEffect(threadId, lifecycle) {
        var stopped = false
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_STOP -> { record(); LeftOff.flush(context); stopped = true }
                Lifecycle.Event.ON_START -> if (stopped) { stopped = false; visitNo++ }
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            record()
            LeftOff.flush(context)
        }
    }
    return state
}

private suspend fun act(effect: UnreadVisit.Effect?, state: UnreadState, rows: List<JsonObject>, listState: LazyListState, threadId: String,
    onPinned: () -> Unit, send: (String, Any?) -> Unit) {
    when (effect) {
        null -> {}
        is UnreadVisit.Effect.Jump -> send("transcript.prompt", mapOf("id" to effect.promptId, "threadId" to threadId))
        UnreadVisit.Effect.Latest -> send("transcript.page", mapOf("id" to "latest", "threadId" to threadId))
        is UnreadVisit.Effect.ScrollTo -> {
            val i = rows.indexOfFirst { it["id"].str == effect.id }
            if (i < 0) return
            state.tookPlace++
            onPinned()
            listState.scrollToItem(i, 0)
        }
    }
}

/** The divider above the first new item: a hairline either side of "New since you left · N". */
@Composable
fun UnreadDivider(count: Int) {
    val c = LocalYaac.current
    DisableSelection {
        Row(Modifier.fillMaxWidth().padding(top = 6.dp, bottom = 8.dp).semantics { heading() }, verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.weight(1f).height(1.dp).background(c.accent))
            T(Unread.label(count), LocalType.current.small, c.accent, modifier = Modifier.padding(horizontal = 9.dp))
            Box(Modifier.weight(1f).height(1.dp).background(c.accent))
        }
    }
}
