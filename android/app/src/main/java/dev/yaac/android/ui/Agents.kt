package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import dev.yaac.android.ui.agents.Budget
import dev.yaac.android.ui.agents.PillEntry
import dev.yaac.android.ui.agents.RowBounds
import dev.yaac.android.ui.agents.SheetEntry
import dev.yaac.android.ui.agents.TimeMenu
import dev.yaac.android.ui.agents.foldedGroupOf
import dev.yaac.android.ui.agents.marksOf
import dev.yaac.android.ui.agents.pillEntries
import dev.yaac.android.ui.agents.rowActivity
import dev.yaac.android.ui.agents.rowAt
import dev.yaac.android.ui.agents.rowInfo
import dev.yaac.android.ui.agents.rowMeta
import dev.yaac.android.ui.agents.sheetEntries
import dev.yaac.android.ui.agents.timeLimitChoice
import dev.yaac.android.ui.agents.timeLimitHours
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/**
 * The 44 dp pill on the right edge (A07): the clusters with their direction glyphs (↑ spawned by,
 * ⑂ workers, ⇄ in contact), each agent's logo with the in-flight arrow, the red "!" on the one
 * that needs you, the unseen ring, finished agents dimmed, and "more (23) · 4 alive".
 * [onOpen] gets true when "more" was tapped: the sheet then opens at the first hidden agent.
 */
@Composable
fun AgentsPill(view: JsonElement?, onOpen: (atHidden: Boolean) -> Unit, modifier: Modifier = Modifier, onParent: ((String) -> Unit)? = null) {
    if (!view["visible"].flag()) return
    val entries = remember(view) { pillEntries(view) }
    if (entries.isEmpty()) return
    val c = LocalYaac.current
    val small = LocalType.current.small
    Column(
        modifier.width(44.dp).clip(RoundedCornerShape(10.dp)).background(c.raised).clickable { onOpen(false) }.padding(vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        // Back to the parent, when there is one (the phone's quick way up the orchestration).
        val parent = remember(view) { dev.yaac.android.ui.agents.parentOf(view) }
        if (parent != null && onParent != null) {
            Box(Modifier.size(36.dp).clip(RoundedCornerShape(8.dp)).clickable { onParent(parent) }
                .semantics { contentDescription = "Open the parent agent" }, contentAlignment = Alignment.Center) {
                Icon(YIcon.ArrowUp, c.text, size = 18.dp)
            }
            Box(Modifier.padding(vertical = 4.dp).width(22.dp).height(1.dp).background(c.rule))
        }
        for (e in entries) when (e) {
            PillEntry.Paused -> T("Ⅱ", small, c.failure, weight = FontWeight.Bold, modifier = Modifier.padding(vertical = 2.dp))
            PillEntry.Related -> T("⇄", small, c.muted, modifier = Modifier.padding(vertical = 2.dp))
            PillEntry.Rule -> Box(Modifier.padding(vertical = 4.dp).width(22.dp).height(1.dp).background(c.rule))
            is PillEntry.Cluster -> T(e.glyph, small.copy(fontSize = 11.sp), c.muted)
            is PillEntry.Agent -> PillAgent(e)
            is PillEntry.More -> Box(Modifier.fillMaxWidth().clickable { onOpen(true) }.padding(vertical = 2.dp), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    for (line in e.lines) T(line, small.copy(fontSize = 10.sp, textAlign = TextAlign.Center), c.muted, maxLines = 1)
                }
                if (e.needsYou) T("!", small, c.failure, weight = FontWeight.Bold, modifier = Modifier.align(Alignment.TopEnd).padding(end = 4.dp))
            }
        }
    }
}

@Composable
private fun PillAgent(e: PillEntry.Agent) {
    val c = LocalYaac.current
    val small = LocalType.current.small
    val r = e.row
    val m = e.marks
    Box(Modifier.padding(vertical = 3.dp).size(32.dp), contentAlignment = Alignment.Center) {
        // Unseen completion: the cornflower ring (D114), at full strength even when finished.
        if (m.unseen) Box(Modifier.size(28.dp).border(1.5.dp, c.unread, RoundedCornerShape(14.dp)))
        Box(Modifier.alpha(if (m.finished) 0.45f else 1f)) {
            r["logo"].text?.let { Logo(it, parseColor(r["logoColor"].text) ?: c.text, size = 20.dp) }
                ?: T(r["provider"].text?.take(1)?.uppercase() ?: "?", small, c.text)
        }
        if (m.needsYou) T("!", small, c.failure, weight = FontWeight.Bold, modifier = Modifier.align(Alignment.BottomEnd))
        if (m.flow.isNotEmpty()) T(m.flow, small.copy(fontSize = 11.sp), c.accent, weight = FontWeight.Bold, modifier = Modifier.align(Alignment.TopEnd))
    }
}

/**
 * Related agents as a sheet (A08): Mark all as read, the budget line with its time-limit menu,
 * the docked rows with "› N finished", *Spawned by* at the foot. Touch and hold a row previews
 * it (the sheet drops to half height and the transcript above shows it); ⋮ opens its menu.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AgentsSheet(view: JsonElement?, send: (String, Any?) -> Unit, onDismiss: () -> Unit, atHidden: Boolean = false, onOpenThread: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    var menu by remember { mutableStateOf<JsonObject?>(null) }
    var timeMenu by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current
    val scope = rememberCoroutineScope()
    val sheet = rememberModalBottomSheetState()
    val scroll = rememberScrollState()
    val firstHidden = view["pill"]["firstHiddenId"].text
    val unfolded = remember { mutableStateMapOf<String, Boolean>().apply { if (atHidden) foldedGroupOf(view, firstHidden)?.let { put(it, true) } } }
    val entries = remember(view, unfolded.toMap()) { sheetEntries(view, unfolded.filterValues { it }.keys) }
    val rowTops = remember { mutableStateMapOf<String, Int>() }
    val rowBounds = remember { HashMap<String, RowBounds>() }
    val owner = view["threadId"].str

    // The preview (A08c): which row the finger is on, and the hover the controller is told about.
    var previewing by remember { mutableStateOf<String?>(null) }
    fun hover(id: String?) {
        if (id == previewing) return
        previewing = id
        // A hold is the intent itself: `now` arms the preview without the pointer's 2 s delay.
        send("agents.hover", mapOf("owner" to owner, "inside" to (id != null), "id" to (id ?: ""), "now" to (id != null)))
    }
    val endPreview = { if (previewing != null) hover(null) }

    // Open at the first hidden agent when "more" was tapped (WPF ScheduleCardPosition).
    var scrolledToHidden by remember { mutableStateOf(!atHidden) }
    val hiddenTop = firstHidden?.let { rowTops[it] }
    LaunchedEffect(hiddenTop) {
        if (!scrolledToHidden && hiddenTop != null) {
            scrolledToHidden = true
            sheet.expand()
            scroll.animateScrollTo(hiddenTop)
        }
    }

    ModalBottomSheet(
        onDismissRequest = { endPreview(); onDismiss() }, sheetState = sheet, containerColor = c.raised, contentColor = c.text, tonalElevation = 0.dp,
        scrimColor = if (previewing != null) Color.Transparent else BottomSheetDefaults.ScrimColor,
    ) {
        // *Spawned by* stays at the foot while the rest scrolls above it, as in the docked panel.
        val footAt = entries.indexOfFirst { it is SheetEntry.Heading && it.key == "spawnedBy" }
        val body = if (footAt >= 0) entries.subList(0, footAt) else entries
        val foot = if (footAt >= 0) entries.subList(footAt, entries.size) else emptyList()
        @Composable fun Entry(e: SheetEntry) {
            when (e) {
                is SheetEntry.Heading -> T(e.label, type.ui.copy(fontSize = 15.sp), c.muted, modifier = Modifier.padding(start = 16.dp, top = 14.dp, bottom = 4.dp))
                is SheetEntry.Fold -> TextAction(if (e.open) "Hide ${e.label.substringAfter(' ')}" else "› ${e.label}", modifier = Modifier.padding(start = 8.dp)) {
                    unfolded[e.group] = !e.open
                }
                is SheetEntry.Info -> T(e.text, type.ui, c.muted, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
                is SheetEntry.Root -> TextAction(e.title, enabled = e.canOpen, modifier = Modifier.padding(start = 8.dp)) {
                    e.id?.let { send("agents.open", mapOf("id" to it)); onDismiss(); onOpenThread() }
                }
                is SheetEntry.Agent -> {
                    val r = e.row
                    val id = e.id
                    AgentRow(
                        r, previewing == id,
                        Modifier.onGloballyPositioned { co ->
                            rowTops[id] = co.positionInParent().y.toInt()
                            co.boundsInRoot().let { b -> rowBounds[id] = RowBounds(id, b.top, b.bottom) }
                        },
                        onOpen = { if (r["canOpen"].flag(true)) { send("agents.open", mapOf("id" to id)); onDismiss(); onOpenThread() } },
                        onMenu = { menu = r },
                        onHold = { scope.launch { sheet.partialExpand() }; hover(id.takeIf { r["canOpen"].flag(true) && !r["self"].flag() }) },
                        onSlide = { y -> hover(rowAt(y, rowBounds.values)?.takeIf { it != owner }) },
                        onRelease = endPreview,
                        send = send,
                    )
                }
            }
        }
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(bottom = 16.dp)) {
        Column(Modifier.fillMaxWidth().weight(1f, fill = false).verticalScroll(scroll, enabled = previewing == null)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                T("Related agents", type.title.copy(fontSize = 19.sp), modifier = Modifier.weight(1f))
                // Every unseen agent, hidden ones included (A05): the docked set, not the card's.
                if (view["canMarkRead"].flag()) TextAction("Mark all as read") { send("agents.markRead", mapOf("floating" to false)) }
            }
            Budget.of(view["budget"])?.let { b -> BudgetLine(b, send) { timeMenu = true } }
            for (e in body) Entry(e)
        }
        if (foot.isNotEmpty()) {
            Box(Modifier.fillMaxWidth().height(1.dp).background(c.rule))
            for (e in foot) Entry(e)
        }
        }
    }
    menu?.let { r ->
        ActionSheet(r["title"].str, ActionItem.list(r["actions"]), { menu = null }) { a ->
            if (a.id == "copyId") clipboard.setText(AnnotatedString(r["id"].str)) else send("agents.action", mapOf("id" to r["id"].str, "action" to a.id))
        }
    }
    if (timeMenu) Budget.of(view["budget"])?.let { b -> b.timeMenu?.let { TimeLimitSheet(b.root, it, send) { timeMenu = false } } }
}

/** "41 of 100 agent turns", "14 h since you acted (limit 12 h)"; the one that ran out in red; Paused · Continue. */
@Composable
private fun BudgetLine(b: Budget, send: (String, Any?) -> Unit, onTimeMenu: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    Column(Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) {
        b.turns?.let { T(it, type.ui, if (b.failing("turns")) c.failure else c.muted) }
        b.time?.let {
            // Touch and hold opens Set / Extend time limit (the desktop's right-click).
            T(it, type.ui, if (b.failing("time")) c.failure else c.muted,
                modifier = Modifier.fillMaxWidth().tapAndHold({}, if (b.timeMenu != null && b.root.isNotEmpty()) onTimeMenu else null).padding(vertical = 2.dp))
        }
        if (b.exhausted) Row(verticalAlignment = Alignment.CenterVertically) {
            b.reached?.let { T("$it ·", type.ui, c.failure); Spacer(Modifier.width(8.dp)) }
            PrimaryButton("Continue", b.root.isNotEmpty()) { send("agents.continue", mapOf("id" to b.root)) }
        }
    }
}

/** Set / Extend time limit (A08d): +1/+4/+12/+24 h, your own hours with Add, Remove, or why not. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TimeLimitSheet(root: String, m: TimeMenu, send: (String, Any?) -> Unit, onDismiss: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.raised, contentColor = c.text, tonalElevation = 0.dp) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().imePadding().padding(bottom = 12.dp)) {
            T(m.title, type.heading, modifier = Modifier.padding(horizontal = 20.dp, vertical = 10.dp))
            if (m.unavailable != null) {
                T(m.unavailable, type.ui, c.muted, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
                return@Column
            }
            for (ch in m.choices) MenuRow(ActionItem("choice:${ch.index}", ch.label, true, ch.detail, false, null, emptyList(), false)) {
                send("agents.timeLimit", timeLimitChoice(root, ch)); onDismiss()
            }
            var hours by remember { mutableStateOf(m.customDefault.toString()) }
            val valid = TimeMenu.hours(hours) != null
            val submit = { timeLimitHours(root, hours)?.let { send("agents.timeLimit", it); onDismiss() } }
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(start = 48.dp, end = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                T("+", type.ui.copy(fontSize = 15.sp)); Spacer(Modifier.width(6.dp))
                BasicTextField(
                    hours, { hours = it.filter(Char::isDigit).take(5) }, singleLine = true,
                    textStyle = type.ui.copy(fontSize = 15.sp, color = c.text), cursorBrush = SolidColor(c.text),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { submit() }),
                    modifier = Modifier.width(64.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(horizontal = 8.dp, vertical = 8.dp),
                )
                Spacer(Modifier.width(6.dp)); T("h", type.ui.copy(fontSize = 15.sp)); Spacer(Modifier.width(12.dp))
                PrimaryButton("Add", valid) { submit() }
            }
            m.remove?.let { rm ->
                Spacer(Modifier.height(6.dp)); Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp).height(1.dp).background(c.rule)); Spacer(Modifier.height(6.dp))
                MenuRow(ActionItem("choice:${rm.index}", rm.label, true, rm.detail, false, null, emptyList(), false)) {
                    send("agents.timeLimit", timeLimitChoice(root, rm)); onDismiss()
                }
            }
        }
    }
}

/**
 * A tap opens; a touch and hold starts the preview, the finger may then slide onto other rows
 * ([onSlide] gets the y in root coordinates), and lifting it ends the preview.
 */
private fun Modifier.holdToPreview(onTap: () -> Unit, onHold: () -> Unit, onSlide: (Float) -> Unit, onRelease: () -> Unit, origin: () -> Float): Modifier =
    pointerInput(Unit) {
        val timeout = viewConfiguration.longPressTimeoutMillis
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false)
            var cancelled = false
            val up = withTimeoutOrNull(timeout) { waitForUpOrCancellation().also { if (it == null) cancelled = true } }
            when {
                up != null -> { up.consume(); onTap() }
                cancelled -> Unit // scrolled away before the hold
                else -> {
                    down.consume()
                    onHold()
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        change.consume()
                        if (!change.pressed) break
                        onSlide(origin() + change.position.y)
                    }
                    onRelease()
                }
            }
        }
    }

@Composable
private fun AgentRow(
    r: JsonObject, previewed: Boolean, modifier: Modifier,
    onOpen: () -> Unit, onMenu: () -> Unit, onHold: () -> Unit, onSlide: (Float) -> Unit, onRelease: () -> Unit,
    send: (String, Any?) -> Unit,
) {
    val c = LocalYaac.current
    val type = LocalType.current
    val self = r["self"].flag()
    val marks = marksOf(r)
    val mono = type.mono.copy(fontSize = 13.sp)
    var top by remember { mutableStateOf(0f) }
    val tap by rememberUpdatedState(onOpen)
    val hold by rememberUpdatedState(onHold)
    val slide by rememberUpdatedState(onSlide)
    val release by rememberUpdatedState(onRelease)
    Row(
        modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 2.dp).onGloballyPositioned { top = it.boundsInRoot().top }
            .clip(RoundedCornerShape(8.dp))
            .background(if (self || previewed) c.selected else Color.Transparent)
            .holdToPreview({ tap() }, { hold() }, { slide(it) }, { release() }) { top }
            .padding(start = 8.dp, top = 8.dp, bottom = 8.dp).alpha(if (marks.finished) 0.6f else 1f),
    ) {
        Box(Modifier.width(36.dp).padding(top = 2.dp)) {
            if (marks.unseen) Box(Modifier.size(28.dp).border(1.5.dp, c.unread, RoundedCornerShape(14.dp)))
            Box(Modifier.size(28.dp), contentAlignment = Alignment.Center) { r["logo"].text?.let { Logo(it, parseColor(r["logoColor"].text) ?: c.text, size = 22.dp) } }
            if (marks.flow.isNotEmpty()) T(marks.flow, type.small, c.accent, weight = FontWeight.Bold, modifier = Modifier.align(Alignment.TopEnd))
        }
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                T(r["title"].str, type.ui.copy(fontSize = 17.sp), maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                if (r["ultrafast"].flag()) { Spacer(Modifier.width(4.dp)); Icon(YIcon.Bolt, c["YaacUltrafast"], size = 14.dp) }
                else if (r["fast"].flag()) { Spacer(Modifier.width(4.dp)); Icon(YIcon.Bolt, c["YaacFast"], size = 14.dp) }
                if (self) { Spacer(Modifier.width(6.dp)); T("self", type.small, c.muted, modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(c.input).padding(horizontal = 6.dp, vertical = 1.dp)) }
                Spacer(Modifier.weight(1f).width(8.dp))
                val mark = r["mark"].text
                StateMark(mark, size = 14.dp, fast = r["fast"].flag(), ultrafast = r["ultrafast"].flag())
                Spacer(Modifier.width(6.dp))
                T(r["markLabel"].str.replaceFirstChar { it.uppercase() }, type.ui, when (mark) { "needsYou", "needs", "failed" -> c.failure; "done" -> c.success; else -> c.text }, maxLines = 1)
            }
            // The flags after the state word: in flight, attention, unread, read-only.
            rowInfo(r).split(" · ").drop(if (r["markLabel"].text != null) 1 else 0).filter { it.isNotEmpty() }.takeIf { it.isNotEmpty() }?.let {
                T(it.joinToString(" · "), type.small, if (marks.needsYou) c.failure else c.muted, maxLines = 1)
            }
            val git = r["git"].obj()?.takeIf { it["branch"].text != null }
            val badge = r["badge"].obj()
            if (git != null || badge != null) Row(verticalAlignment = Alignment.CenterVertically) {
                git?.let { g ->
                    Icon(YIcon.Branch, c.muted, size = 13.dp); Spacer(Modifier.width(5.dp))
                    T(g["branch"].str, mono, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                }
                badge?.let { b -> Spacer(Modifier.width(6.dp)); PrBadge(b, { action, n -> send("pr.action", mapOf("id" to r["id"].str, "action" to action, "number" to n)) }, {}, {}) }
                git?.let { g ->
                    g["changes"].text?.let { T("  $it", mono, c.warning) }
                    (g["ahead"].int ?: 0).takeIf { it > 0 }?.let { T("  ↑$it", mono, c.muted) }
                    (g["behind"].int ?: 0).takeIf { it > 0 }?.let { T("  ↓$it", mono, c.muted) }
                }
            }
            rowMeta(r).takeIf { it.isNotEmpty() }?.let { T(it, mono, maxLines = 1) }
            rowActivity(r).takeIf { it.isNotEmpty() }?.let { T(it, mono, c.muted, maxLines = 1) }
        }
        if (r["actions"] != null) Box(Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).clickable(onClick = onMenu), contentAlignment = Alignment.Center) { Icon(YIcon.More, c.text, size = 20.dp, strokeWidth = 3f) }
    }
}
