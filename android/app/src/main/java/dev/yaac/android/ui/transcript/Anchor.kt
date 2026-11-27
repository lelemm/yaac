package dev.yaac.android.ui.transcript

import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.platform.LocalDensity
import dev.yaac.android.core.double
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.scalar
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.debounce
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/**
 * Where the transcript should be put: the top of row [id] scrolled [offsetDp] above the top of
 * the viewport (WPF's `ScrollPin(id, -offset)`).
 */
data class Pin(val id: String, val offsetDp: Float)

/**
 * The controller's restore anchor (`anchor` {id, offset} with `anchorToken`), applied once per
 * token, as `TranscriptView.Update` does: a new token pins the anchor, a missing token forgets
 * the last one, the same token again does nothing.
 */
class AnchorRestore {
    var token: String? = null
        private set

    fun incoming(token: String?, anchor: JsonElement?): Pin? {
        if (token == null) { this.token = null; return null }
        if (token == this.token) return null
        val id = anchor.text ?: anchor["id"].text ?: return null
        val offset = anchor["offset"].double?.takeIf { it.isFinite() }?.coerceAtLeast(0.0) ?: 0.0
        this.token = token
        return Pin(id, offset.toFloat())
    }
}

/**
 * What `transcript.anchor` reports as the user scrolls (`TranscriptView.ReportAnchor`): the
 * first visible row and how far its top is above the viewport, in dp; `id: ""` while the end
 * is followed. Nothing is sent when it has not moved by a dp.
 */
class AnchorReporter {
    private var last: Pair<String, Float>? = null

    fun report(threadId: String, follow: Boolean, historyAway: Boolean, row: JsonObject?, offsetDp: Float): Map<String, Any?>? {
        if (follow && !historyAway) {
            if (last?.first == "") return null
            last = "" to 0f
            return mapOf("id" to "", "offset" to 0, "threadId" to threadId)
        }
        val id = row?.get("id").text ?: return null
        val offset = offsetDp.coerceAtLeast(0f)
        last?.let { (lid, lo) -> if (lid == id && kotlin.math.abs(lo - offset) < 1f) return null }
        last = id to offset
        return mapOf("id" to id, "itemId" to row["actionId"].text, "offset" to offset.toDouble(), "threadId" to threadId)
    }
}

/**
 * The row to keep in view when [anchor] (the first visible row) is gone from the new rows, as
 * when a turn folds into its "Worked for…" line or a run of commands into one group: the row
 * that took its place (a new row right after the nearest survivor before it), else that
 * survivor, else the nearest survivor after it. Null when [anchor] is still there or nothing
 * survives.
 */
fun relocate(old: List<String>, new: List<String>, anchor: String): String? {
    if (anchor in new) return null
    val at = old.indexOf(anchor)
    if (at < 0) return null
    val kept = new.toHashSet()
    val known = old.toHashSet()
    val before = (at - 1 downTo 0).firstOrNull { old[it] in kept }?.let { old[it] }
    val start = before?.let { new.indexOf(it) + 1 } ?: 0
    new.getOrNull(start)?.takeIf { it !in known }?.let { return it }
    if (before != null) return before
    return (at + 1 until old.size).firstOrNull { old[it] in kept }?.let { old[it] }
}

/**
 * Keeps the transcript's place (T02): applies the controller's restore anchor, reports the
 * first visible row as the user scrolls (`transcript.anchor`), and keeps the place when the row
 * at the top folds away. Call it beside the transcript's LazyColumn with its state; [onPinned]
 * stops following the end (an anchor was applied).
 */
@OptIn(FlowPreview::class)
@Composable
fun KeepAnchor(transcript: JsonElement?, rows: List<JsonObject>, listState: LazyListState, follow: Boolean,
    onPinned: () -> Unit, send: (String, Any?) -> Unit) {
    val threadId = transcript["threadId"].str
    val density = LocalDensity.current.density
    val restore = remember(threadId) { AnchorRestore() }
    val reporter = remember(threadId) { AnchorReporter() }
    val ids = remember(rows) { rows.map { it["id"].str } }
    val currentRows by rememberUpdatedState(rows)
    val currentFollow by rememberUpdatedState(follow)
    val historyAway by rememberUpdatedState(transcript["historyAway"].flag())

    // The restore anchor: once per token, as soon as its row is in the rows.
    var pending by remember(threadId) { mutableStateOf<Pin?>(null) }
    val token = transcript["anchorToken"].scalar
    val anchor by rememberUpdatedState(transcript["anchor"])
    LaunchedEffect(threadId, token) { restore.incoming(token, anchor)?.let { pending = it } }
    LaunchedEffect(pending, ids) {
        val pin = pending ?: return@LaunchedEffect
        val i = ids.indexOf(pin.id)
        if (i < 0) return@LaunchedEffect
        pending = null
        onPinned()
        listState.scrollToItem(i, (pin.offsetDp * density).toInt())
    }

    // The first visible row, kept for when it folds away.
    val top = remember(threadId) { arrayOf<String?>(null, null) }
    var previous by remember(threadId) { mutableStateOf(ids) }
    LaunchedEffect(ids) {
        val old = previous
        previous = ids
        val anchor = top[0] ?: return@LaunchedEffect
        if (currentFollow || pending != null) return@LaunchedEffect
        val target = relocate(old, ids, anchor) ?: return@LaunchedEffect
        val i = ids.indexOf(target)
        if (i >= 0) listState.scrollToItem(i, 0)
    }

    LaunchedEffect(listState, threadId) {
        snapshotFlow { listState.firstVisibleItemIndex to listState.firstVisibleItemScrollOffset }
            .debounce(250)
            .collect { (index, offset) ->
                val row = currentRows.getOrNull(index)
                top[0] = row?.get("id").text
                reporter.report(threadId, currentFollow, historyAway, row, offset / density)?.let { send("transcript.anchor", it) }
            }
    }
}
