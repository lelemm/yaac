package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.double
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.objects
import dev.yaac.android.core.list
import dev.yaac.android.core.scalar
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.coroutines.launch
import dev.yaac.android.ui.transcript.timelineGestures
import kotlinx.serialization.json.JsonElement

/** The thread (A05, A07, A12): header, transcript, request cards, composer, control row, pill. */
@Composable
fun ThreadScreen(
    shell: JsonElement?, transcript: JsonElement?, composer: JsonElement?, agents: JsonElement?, sidebar: JsonElement?, history: JsonElement?,
    states: TranscriptStates, send: (String, Any?) -> Unit, onBack: () -> Unit, onNeeds: () -> Unit, showBack: Boolean = true,
    browser: JsonElement? = null, side: SidePaneState? = null,
) {
    var addingSide by remember { mutableStateOf(false) }
    var visibleApproval by remember { mutableStateOf<String?>(null) }
    val c = LocalYaac.current
    val type = LocalType.current
    var agentsOpen by remember { mutableStateOf(false) }
    var agentsAtHidden by remember { mutableStateOf(false) }
    var threadMenu by remember { mutableStateOf(false) }
    var headerSheet by remember { mutableStateOf(false) }
    val threadId = transcript["threadId"].str
    val rows = transcript["rows"].objects
    val listState = remember(threadId) { androidx.compose.foundation.lazy.LazyListState(firstVisibleItemIndex = maxOf(0, rows.size - 1)) }
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current

    Column(Modifier.fillMaxSize().background(c.canvas).statusBarsPadding().imePadding()) {
        Header(shell, onBack.takeIf { showBack }, onNeeds, send, { headerSheet = true },
            onAddSide = { addingSide = true }.takeIf { browser != null && browser["tabs"].objects.isEmpty() },
            // With an approval on screen the Commander offers its Allow / Deny rows (WPF sends the focused card).
            onCommander = { send("commander.open", mapOf("card" to visibleApproval, "threadId" to visibleApproval?.let { threadId })) }) { threadMenu = true }
        // The side pane's tabs (Files, Terminal, Browser) beside the chat, at compact width.
        if (browser != null) SideTabs(browser, shell["threadId"].str, send, addingSide, side) { addingSide = it }
        Spacer(Modifier.height(4.dp))
        if (browser != null && side != null && !showsChat(browser, side)) {
            SideContent(browser, shell["threadId"].str, side, send, Modifier.weight(1f))
        } else {
        for (b in shell["banners"].objects) Banner(b, shell["threadId"].str, send)
        shell["status"].obj()?.let { st ->
            st["text"].text?.let { text ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    T(text, type.ui, c.failure, modifier = Modifier.weight(1f))
                    TextAction("Dismiss") { send("shell.dismissStatus", mapOf("id" to (st["id"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull())) }
                }
            }
        }

        val prompts = transcript["prompts"].objects
        val timelineState = dev.yaac.android.ui.transcript.rememberTimelineState()
        val haptics = androidx.compose.ui.platform.LocalHapticFeedback.current
        // A prompt picked on the timeline: jumped to as soon as its row is loaded (it may be on an older page).
        var pendingJump by remember(threadId) { mutableStateOf<String?>(null) }
        val currentPrompts by androidx.compose.runtime.rememberUpdatedState(prompts)
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth().timelineGestures(timelineState, { currentPrompts }, { id ->
            pendingJump = id
            send("transcript.prompt", mapOf("id" to id, "threadId" to threadId))
        }) { strong ->
            haptics.performHapticFeedback(if (strong) androidx.compose.ui.hapticfeedback.HapticFeedbackType.LongPress else androidx.compose.ui.hapticfeedback.HapticFeedbackType.TextHandleMove)
        }) {
            val height = maxHeight
            if (rows.isEmpty()) {
                T(if (transcript["loading"].flag()) "Loading…" else transcript["emptyText"].str, type.ui, c.muted, modifier = Modifier.align(Alignment.Center))
            }
            // The timeline (A11f) is hidden: hold a finger at the left edge to scrub it.
            // One selection over the whole transcript, so it runs across rows (T05).
            androidx.compose.foundation.text.selection.SelectionContainer { LazyColumn(Modifier.fillMaxSize(), state = listState, contentPadding = PaddingValues(start = dev.yaac.android.ui.transcript.TimelineGutter, end = if (agents["visible"].flag() && agents["pill"]["rowIds"].list.isNotEmpty()) 54.dp else 14.dp, top = 4.dp, bottom = 12.dp)) {
                items(rows, key = { it["id"].str }, contentType = { it["kind"].str }) { row ->
                    TranscriptRow(row, threadId, states.of(threadId, row["id"].str)) { payload -> send("transcript.action", payload) }
                }
            } }
            dev.yaac.android.ui.transcript.TimelineOverlay(timelineState, prompts)
            // Follow the end while it is in view; page older and newer history at the edges.
            val atEnd by remember(listState) { derivedStateOf { !listState.canScrollForward } }
            var follow by remember(threadId) { mutableStateOf(true) }
            // Only the user's own scrolling changes whether the end is followed; new rows do not.
            LaunchedEffect(listState) {
                snapshotFlow { listState.isScrollInProgress }.collect { scrolling -> if (!scrolling) follow = !listState.canScrollForward }
            }
            dev.yaac.android.ui.transcript.KeepAnchor(transcript, rows, listState, follow, { follow = false }, send) // T02
            val lastKey = rows.lastOrNull()?.hashCode()
            // Rows already seen: everything while the end is followed.
            var seenCount by remember(threadId) { mutableStateOf(rows.size) }
            if (follow && seenCount != rows.size) seenCount = rows.size
            LaunchedEffect(threadId, rows.size, lastKey) {
                if (follow && rows.isNotEmpty()) listState.scrollToItem(rows.size - 1, Int.MAX_VALUE / 2)
            }
            val page = transcript["page"]
            val first by remember(listState) { derivedStateOf { listState.firstVisibleItemIndex } }
            var requested by remember(threadId) { mutableStateOf<String?>(null) }
            LaunchedEffect(first, page) {
                if (page["loading"].flag()) return@LaunchedEffect
                val older = page["older"].text
                if (first < 2 && older != null && older != requested) { requested = older; send("transcript.page", mapOf("id" to "older", "threadId" to threadId)) }
                val newer = page["newer"].text
                if (atEnd && newer != null && newer != requested) { requested = newer; send("transcript.page", mapOf("id" to "newer", "threadId" to threadId)) }
            }
            // The request cards on screen, so the hub does not notify them (as WPF reports them).
            LaunchedEffect(listState, threadId) {
                snapshotFlow {
                    listState.layoutInfo.visibleItemsInfo.mapNotNull { info -> rows.getOrNull(info.index)?.get("requestLocation").obj() }
                        .map { mapOf("target" to it["target"].str, "threadId" to it["threadId"].str, "requestId" to it["requestId"].str) }
                }.distinctUntilChanged().collect { cards -> send("shell.visibleRequests", mapOf("cards" to cards)) }
            }
            androidx.compose.runtime.DisposableEffect(threadId) { onDispose { send("shell.visibleRequests", mapOf("cards" to emptyList<Any>())) } }
            // The approval on screen, for the Commander's Allow once / Allow for the session / Deny.
            LaunchedEffect(listState, threadId) {
                snapshotFlow { listState.layoutInfo.visibleItemsInfo.firstNotNullOfOrNull { info -> rows.getOrNull(info.index)?.takeIf { it["kind"].str == "approval" }?.get("actionId").text } }
                    .distinctUntilChanged().collect { visibleApproval = it }
            }
            val focusToken = transcript["focusToken"].scalar
            // Applied once per focus: retried only until its row has arrived (it may be on a page
            // that is still loading), never again afterwards, so later row changes leave the view alone.
            var appliedFocus by remember(threadId) { mutableStateOf<String?>(null) }
            LaunchedEffect(focusToken, rows.size) {
                val token = focusToken ?: return@LaunchedEffect
                if (token == appliedFocus) return@LaunchedEffect
                val id = transcript["focusId"].text ?: return@LaunchedEffect
                val i = rows.indexOfFirst { it["id"].str == id || it["actionId"].str == id }
                if (i >= 0) { appliedFocus = token; follow = false; listState.animateScrollToItem(i) }
            }
            LaunchedEffect(pendingJump, rows.size) {
                val id = pendingJump ?: return@LaunchedEffect
                val i = rows.indexOfFirst { it["id"].str == id }
                if (i >= 0) { follow = false; listState.scrollToItem(i); pendingJump = null }
            }
            if (!atEnd && !follow && rows.isNotEmpty()) {
                Row(Modifier.align(Alignment.BottomCenter).padding(bottom = 8.dp).clip(RoundedCornerShape(16.dp)).background(c.raised)
                    .clickable { scope.launch { follow = true; listState.scrollToItem(rows.size - 1, Int.MAX_VALUE / 2) } }.padding(horizontal = 14.dp, vertical = 7.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    val fresh = rows.size - seenCount
                    T(when {
                        transcript["historyAway"].flag() -> "Jump to latest"
                        fresh > 0 -> "$fresh new ${if (fresh == 1) "message" else "messages"}"
                        else -> "Latest"
                    }, type.ui)
                    Spacer(Modifier.width(6.dp)); Icon(YIcon.ArrowDown, c.text, size = 15.dp)
                }
            }
            val fraction = (agents["pillPosition"].double ?: 0.5).toFloat().coerceIn(0.05f, 0.95f)
            AgentsPill(agents, { hidden -> agentsAtHidden = hidden; agentsOpen = true }, Modifier.align(Alignment.TopEnd).padding(end = 6.dp).offset(y = height * fraction * 0.6f)) { parent -> send("agents.open", mapOf("id" to parent)) }
        }
        Box(Modifier.navigationBarsPadding()) {
            // The thread's harness logo for the model control (a started thread never changes provider).
            val logo = remember(sidebar, threadId) { threadLogo(sidebar, threadId) }
            Composer(composer, send, history, logo)
        }
        }
    }

    if (headerSheet) HeaderSheet(shell, sidebar, send) { headerSheet = false }
    if (agentsOpen) AgentsSheet(agents, send, { agentsOpen = false }, atHidden = agentsAtHidden) { agentsOpen = false }
    if (threadMenu) {
        val selected = sidebar["sections"].objects.flatMap { it["rows"].objects }.firstOrNull { it["id"].str == shell["threadId"].str }
        fun places(key: String) = shell[key].objects.map { ActionItem("history:" + (it["steps"].int ?: 0), it["label"].str, true, it["detail"].text, false, null, emptyList(), false) }
        val nav = listOf(
            ActionItem("nav.forward", "Forward", shell["canForward"].flag(), null, false, null, emptyList(), false),
            ActionItem("nav.backList", "Back history", places("back").isNotEmpty(), null, false, null, places("back"), false),
            ActionItem("nav.forwardList", "Forward history", places("forward").isNotEmpty(), null, false, null, places("forward"), false),
            ActionItem("", "", false, null, false, null, emptyList(), false),
        )
        ActionSheet(shell["title"].str, nav + ActionItem.list(selected?.get("actions")), { threadMenu = false }) { a ->
            when (a.id) {
                "nav.forward" -> send("shell.forward", null)
                "copyId" -> clipboard.setText(AnnotatedString(selected?.get("copyId").text ?: shell["threadId"].str))
                else -> if (a.id.startsWith("history:")) send("shell.history", mapOf("steps" to a.id.removePrefix("history:").toInt()))
                    else send("sidebar.action", mapOf("action" to a.id, "ids" to listOf(shell["threadId"].str)))
            }
        }
    }
}

@Composable
private fun Header(shell: JsonElement?, onBack: (() -> Unit)?, onNeeds: () -> Unit, send: (String, Any?) -> Unit, onTitle: () -> Unit, onAddSide: (() -> Unit)? = null, onCommander: () -> Unit, onMenu: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    Row(Modifier.fillMaxWidth().padding(start = 2.dp, end = 2.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        if (onBack != null) IconButton(YIcon.Back, "Back to the list", onClick = onBack) else Spacer(Modifier.width(12.dp))
        val mark = shell["mark"].text
        if (shell["hasThread"].flag() && mark == null && shell["readOnly"].flag()) { Icon(YIcon.Lock, c.muted, size = 15.dp, strokeWidth = 1.8f); Spacer(Modifier.width(8.dp)) }
        if (shell["hasThread"].flag() && mark != null) {
            StateMark(mark, size = 16.dp, fast = shell["fast"].flag(), ultrafast = shell["ultrafast"].flag())
            Spacer(Modifier.width(8.dp))
        }
        Column(Modifier.weight(1f).clip(RoundedCornerShape(6.dp)).clickable(onClickLabel = "Thread details", onClick = onTitle).padding(vertical = 2.dp)) {
            T(shell["title"].str, type.title.copy(fontSize = 19.sp), maxLines = 1)
            val tag = shell["tag"].text
            val location = shell["location"].text
            // State · breadcrumb on one line, the breadcrumb ellipsised first.
            Row(verticalAlignment = Alignment.CenterVertically) {
                tag?.let { T(it, type.ui, toneColor(shell["tagTone"].text, c), maxLines = 1) }
                if (tag != null && location != null) T(" · ", type.ui, c.muted)
                location?.let { T(it, type.ui, c.muted, maxLines = 1, modifier = Modifier.weight(1f, fill = false)) }
            }
        }
        NeedsButton(shell["needs"].int ?: 0, onNeeds)
        // The orchestration's budget is limiting it: the mark opens the agents sheet's budget line.
        dev.yaac.android.ui.transcript.HeaderBudget(shell, send) // T01: the reason and Continue
        if (onAddSide != null) IconButton(YIcon.CirclePlus, "Open beside the chat", strokeWidthIcon = 1.7f, onClick = onAddSide)
        IconButton(YIcon.Command, "Commander") { onCommander() }
        IconButton(YIcon.More, "Thread menu", strokeWidthIcon = 3f, onClick = onMenu)
    }
    shell["links"].objects.takeIf { it.isNotEmpty() }?.let { links ->
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(start = 52.dp, bottom = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            for (l in links) {
                val id = l["id"].text
                T(l["label"].str, type.small, if (id == null) c.muted else c.text, maxLines = 1,
                    modifier = if (id == null) Modifier.padding(end = 5.dp) else Modifier.clip(RoundedCornerShape(4.dp)).clickable { send("sidebar.open", mapOf("id" to id)) }.padding(horizontal = 4.dp, vertical = 2.dp))
            }
        }
    }
}

@Composable
private fun IconButton(icon: YIcon, description: String, strokeWidthIcon: Float, onClick: () -> Unit) {
    val c = LocalYaac.current
    Box(Modifier.padding(0.dp).clip(RoundedCornerShape(6.dp)).clickable(onClickLabel = description, onClick = onClick)
        .semantics { contentDescription = description }.padding(11.dp), contentAlignment = Alignment.Center) {
        Icon(icon, c.text, size = 22.dp, strokeWidth = strokeWidthIcon)
    }
}

private fun toneColor(tone: String?, c: YaacColors) = when (tone) {
    "needs", "needsYou", "needs you" -> c.warning
    "failed" -> c.failure
    "done" -> c.success
    else -> c.muted
}

@Composable
private fun Banner(b: kotlinx.serialization.json.JsonObject, threadId: String, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(12.dp)) {
        T(b["label"].str, type.ui)
        b["detail"].text?.let { T(it, type.small, if (b["failed"].flag()) c.failure else c.muted) }
        Row {
            for (a in ActionItem.list(b["actions"])) {
                if (a.primary) PrimaryButton(a.label, a.enabled) { send("header.action", mapOf("id" to a.id, "threadId" to threadId)) }
                else TextAction(a.label, a.enabled) { send("header.action", mapOf("id" to a.id, "threadId" to threadId)) }
            }
        }
    }
}



/** The open thread's harness logo and colour, from its row in the list. */
private fun threadLogo(sidebar: JsonElement?, threadId: String): Pair<String, androidx.compose.ui.graphics.Color?>? {
    fun walk(e: JsonElement?): Pair<String, androidx.compose.ui.graphics.Color?>? {
        when (e) {
            is kotlinx.serialization.json.JsonObject -> {
                if (e["id"].str == threadId) {
                    val l2 = e["line2"].obj()
                    (l2?.get("logo").text ?: e["logo"].text)?.let { return it to parseColor(l2?.get("logoColor").text ?: e["logoColor"].text) }
                }
                for (v in e.values) walk(v)?.let { return it }
            }
            is kotlinx.serialization.json.JsonArray -> for (v in e) walk(v)?.let { return it }
            else -> {}
        }
        return null
    }
    return if (threadId.isEmpty()) null else walk(sidebar)
}
