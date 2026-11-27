package dev.yaac.android.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.list
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** A01: the list (the desktop sidebar), the home screen at compact width. */
@OptIn(ExperimentalFoundationApi::class, FlowPreview::class)
@Composable
fun ListScreen(sidebar: JsonElement?, shell: JsonElement?, send: (String, Any?) -> Unit, openThread: (String) -> Unit, onNeeds: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    var sortSheet by remember { mutableStateOf(false) }
    var rowMenu by remember { mutableStateOf<JsonObject?>(null) }
    var headerMenu by remember { mutableStateOf<JsonObject?>(null) }
    var prOthers by remember { mutableStateOf<JsonObject?>(null) }
    var prMenu by remember { mutableStateOf<JsonObject?>(null) }
    var usageTip by remember { mutableStateOf<String?>(null) }
    val clipboard = LocalClipboardManager.current
    val listState = rememberLazyListState()
    val sections = sidebar["sections"].objects

    // Selection (A03): a tap on a row's state mark adds or removes it (Ctrl+click); once there is
    // a selection, a tap anywhere on a row does. The controller learns it through sidebar.selection.
    var selection by remember { mutableStateOf(listOf<String>()) }
    var rangeMode by remember { mutableStateOf(false) }
    var moveSheet by remember { mutableStateOf(false) }
    var selectionMenu by remember { mutableStateOf(false) }
    val visibleIds = sections.filter { !it["collapsed"].flag() }.flatMap { it["rows"].objects }.map { it["id"].str }
    fun select(next: List<String>) {
        val clean = next.distinct().filter { it in visibleIds }
        if (clean != selection) { selection = clean; send("sidebar.selection", mapOf("ids" to clean)) }
    }
    val reset = sidebar["selectionReset"].text
    LaunchedEffect(reset) { if (selection.isNotEmpty()) { selection = emptyList(); rangeMode = false } }
    LaunchedEffect(visibleIds) { if (selection.any { it !in visibleIds }) select(selection) }
    androidx.activity.compose.BackHandler(enabled = selection.isNotEmpty()) { rangeMode = false; select(emptyList()) }
    var dragAt by remember { mutableStateOf<androidx.compose.ui.geometry.Offset?>(null) }
    var listOrigin by remember { mutableStateOf(androidx.compose.ui.geometry.Offset.Zero) }
    val sectionOfKey = remember(sections) {
        HashMap<String, String>().apply {
            for (sec in sections) {
                val sid = sec["id"].str
                put("section:$sid", sid)
                for (r in sec["rows"].objects) put("row:" + r["id"].str, sid)
            }
        }
    }
    // The group under the finger, from the list's laid-out items.
    val hoverSection = dragAt?.let { at ->
        val y = at.y - listOrigin.y
        listState.layoutInfo.visibleItemsInfo.firstOrNull { y >= it.offset && y < it.offset + it.size }?.let { sectionOfKey[it.key as? String] }
    }
    val hoverDrop = sections.firstOrNull { it["id"].str == hoverSection }?.get("drop").obj()
    val dragHandler = DragHandler(
        move = { dragAt = it },
        drop = {
            val action = ActionItem.of(hoverDrop?.get("action"))
            if (hoverSection != null && action?.enabled == true)
                send("sidebar.action", mapOf("action" to action.id, "ids" to selection, "sectionId" to hoverSection))
            dragAt = null
        },
        cancel = { dragAt = null },
    )
    val toggle: (String) -> Unit = { id ->
        if (rangeMode && selection.isNotEmpty()) {
            val a = visibleIds.indexOf(selection.last()); val b = visibleIds.indexOf(id)
            if (a >= 0 && b >= 0) select(selection + visibleIds.subList(minOf(a, b), maxOf(a, b) + 1))
            rangeMode = false
        } else select(if (id in selection) selection - id else selection + id)
    }

    // The controller refreshes pull requests for the rows on screen (`shell.prVisible`).
    val currentSections by rememberUpdatedState(sections)
    LaunchedEffect(listState) {
        snapshotFlow { listState.layoutInfo.visibleItemsInfo.mapNotNull { (it.key as? String)?.removePrefix("row:")?.takeIf { _ -> (it.key as String).startsWith("row:") } } }
            .distinctUntilChanged().debounce(250).collect { ids -> send("shell.prVisible", mapOf("ids" to ids, "agentsVisible" to false)) }
    }

    Column(Modifier.fillMaxSize().background(c.panel).statusBarsPadding()) {
        if (selection.isNotEmpty()) SelectionBar(selection.size, rangeMode, ActionItem.list(sidebar["bulkActions"]),
            onClear = { rangeMode = false; select(emptyList()) },
            onBulk = { a -> send("sidebar.action", mapOf("action" to a.id, "ids" to selection)) },
            onMove = { moveSheet = true }, onMore = { selectionMenu = true })
        // Top bar: mark · Needs you · search · ⇅ · Commander · New.
        else Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 14.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            // In the demo, touch and hold the mark for the scenes (WPF's Scenes button).
            var scenes by remember { mutableStateOf(false) }
            val sceneList = shell["scenes"].objects
            T("YAAC", type.title.copy(fontSize = 19.sp), weight = FontWeight.Bold,
                modifier = if (sceneList.isEmpty()) Modifier else Modifier.tapAndHold({}, { scenes = true }))
            if (scenes) ActionSheet("Demo scenes", sceneList.map { ActionItem("scene:" + it["id"].int, it["label"].str, true, null, false, null, emptyList(), false) }, { scenes = false }) { a ->
                send("shell.scene", mapOf("id" to a.id.removePrefix("scene:").toInt()))
            }
            Spacer(Modifier.width(6.dp))
            NeedsButton(shell["needs"].int ?: 0, onNeeds)
            Spacer(Modifier.width(4.dp))
            SearchField(sidebar, send, Modifier.weight(1f), shell["focusSearch"].flag())
            IconButton(YIcon.Sort, "Grouping and order") { sortSheet = true }
            IconButton(YIcon.Command, "Commander") { send("commander.open", mapOf("card" to null, "threadId" to null)) }
            val newThread = ActionItem.of(sidebar["newThread"])
            IconButton(YIcon.Plus, newThread?.label ?: "New", enabled = newThread?.enabled == true) { send("sidebar.new", null); openThread("") }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(c.rule))

        val searching = sidebar["searchResults"].flag()
        Box(Modifier.weight(1f).fillMaxWidth()) {
        LazyColumn(Modifier.fillMaxSize().onGloballyPositioned { listOrigin = it.positionInRoot() }, state = listState, contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 12.dp)) {
            if (searching) {
                sidebar["searchStatus"].text?.let { s -> item("searchStatus") { T(s, type.small, c.muted, modifier = Modifier.padding(16.dp, 10.dp)) } }
                sidebar["searchError"].text?.let { s -> item("searchError") { T(s, type.small, c.failure, modifier = Modifier.padding(16.dp, 4.dp)) } }
                if (sidebar["searchEmpty"].flag()) item("searchEmpty") { T("No threads found", type.ui, c.muted, modifier = Modifier.padding(16.dp, 10.dp)) }
                items(sidebar["searchRows"].objects, key = { "row:" + it["id"].str }) { row ->
                    ThreadRow(row, onOpen = { send("sidebar.open", mapOf("id" to row["id"].str)); openThread(row["id"].str) }, onHold = null, prAction = { _, _ -> }, prOthers = {}, prMenu = {})
                }
                ActionItem.of(sidebar["searchMore"])?.let { more ->
                    item("searchMore") { TextAction(more.label, more.enabled, modifier = Modifier.padding(horizontal = 6.dp)) { send("sidebar.searchMore", mapOf("query" to sidebar["search"].str)) } }
                }
            } else {
                for (section in sections) {
                    val sid = section["id"].str
                    val header = section["header"].obj()
                    stickyHeader(key = "section:$sid") {
                        SectionHeader(header, section["collapsed"].flag(), outlined = dragAt != null && hoverSection == sid,
                            onToggle = { ActionItem.of(header?.get("toggle"))?.takeIf { it.enabled }?.let { send("sidebar.action", mapOf("action" to it.id, "ids" to emptyList<String>(), "sectionId" to sid)) } },
                            onPrimary = { a -> send("sidebar.action", mapOf("action" to a.id, "ids" to emptyList<String>(), "sectionId" to sid)) },
                            onHold = { if (header?.get("actions").list.isNotEmpty()) headerMenu = JsonObject(mapOf("sectionId" to JsonPrimitive(sid), "header" to (header ?: JsonObject(emptyMap())))) },
                        )
                    }
                    if (!section["collapsed"].flag()) {
                        items(section["rows"].objects, key = { "row:" + it["id"].str }) { row ->
                            val rid = row["id"].str
                            ThreadRow(row,
                                selected = rid in selection,
                                drag = dragHandler,
                                onMark = { toggle(rid) },
                                onOpen = { if (selection.isNotEmpty()) toggle(rid) else { send("sidebar.open", mapOf("id" to rid)); openThread(rid) } },
                                onHold = { rowMenu = row },
                                prAction = { action, number -> send("pr.action", mapOf("id" to row["id"].str, "action" to action, "number" to number)) },
                                prOthers = { prOthers = row }, prMenu = { prMenu = row })
                        }
                    }
                }
            }
        }

        // The card that follows the finger: "Move to api", or why the drop is refused.
        dragAt?.let { at ->
            val action = ActionItem.of(hoverDrop?.get("action"))
            val label = sections.firstOrNull { it["id"].str == hoverSection }?.get("header")?.obj()?.get("label").text
            val text = when {
                hoverSection == null -> "Move ${selection.size} to…"
                action?.enabled == true -> "Move to $label"
                else -> hoverDrop?.get("detail").text ?: "Cannot move here"
            }
            val y = with(androidx.compose.ui.platform.LocalDensity.current) { (at.y - listOrigin.y).toDp() }
            Box(Modifier.padding(start = 56.dp, top = (y - 52.dp).coerceAtLeast(0.dp)).clip(RoundedCornerShape(6.dp)).background(c.raised).padding(horizontal = 12.dp, vertical = 8.dp)) {
                T(text, type.ui, if (action?.enabled == true || hoverSection == null) c.text else c.failure, maxLines = 2)
            }
        }
        }
        sidebar["notice"].obj()?.let { notice ->
            Row(Modifier.fillMaxWidth().background(c.input).padding(horizontal = 14.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                T(notice["label"].str, type.small, modifier = Modifier.weight(1f))
                for (a in ActionItem.list(notice["actions"])) TextAction(a.label, a.enabled) { send("sidebar.action", mapOf("action" to a.id, "ids" to emptyList<String>())) }
            }
        }
        Footer(sidebar, shell, send, onUsage = { usageTip = it })
    }

    if (sortSheet) {
        val items = ActionItem.list(sidebar["grouping"]) + listOf(ActionItem("", "", false, null, false, null, emptyList(), false)) + ActionItem.list(sidebar["ordering"])
        ActionSheet("Grouping and order", items, { sortSheet = false }, keepOpenOnCheck = true) { a -> send("sidebar.action", mapOf("action" to a.id, "ids" to emptyList<String>())) }
    }
    if (moveSheet) {
        val targets = sections.mapNotNull { sec ->
            val drop = sec["drop"].obj() ?: return@mapNotNull null
            val a = ActionItem.of(drop["action"]) ?: return@mapNotNull null
            val h = sec["header"].obj()
            ActionItem(sec["id"].str, listOfNotNull(h?.get("label").text, h?.get("machine").text).joinToString(" · "), a.enabled, drop["detail"].text, false, null, emptyList(), false) to a.id
        }
        ActionSheet("Move ${selection.size} to", targets.map { it.first }, { moveSheet = false }) { item ->
            val action = targets.first { it.first.id == item.id }.second
            send("sidebar.action", mapOf("action" to action, "ids" to selection, "sectionId" to item.id))
        }
    }
    if (selectionMenu) {
        val bulk = ActionItem.list(sidebar["bulkActions"]).filter { it.id !in setOf("bulk.archive", "bulk.mute") }
        val local = listOf(
            ActionItem("local.range", "Select range", true, "Selects every row up to the next one you tap", false, null, emptyList(), false),
            ActionItem("local.all", "Select all", true, null, false, null, emptyList(), false),
            ActionItem("local.clear", "Clear selection", true, null, false, null, emptyList(), false),
        )
        ActionSheet("${selection.size} selected", bulk + local, { selectionMenu = false }) { a ->
            when (a.id) {
                "local.range" -> rangeMode = true
                "local.all" -> select(visibleIds)
                "local.clear" -> { rangeMode = false; select(emptyList()) }
                else -> send("sidebar.action", mapOf("action" to a.id, "ids" to selection))
            }
        }
    }
    rowMenu?.let { row ->
        ActionSheet(row["title"].str, ActionItem.list(row["actions"]), { rowMenu = null }) { a ->
            if (a.id == "copyId") clipboard.setText(AnnotatedString(row["copyId"].text ?: row["id"].str))
            else send("sidebar.action", mapOf("action" to a.id, "ids" to listOf(row["id"].str)))
        }
    }
    headerMenu?.let { m ->
        val header = m["header"].obj()
        ActionSheet(header?.get("label").str, ActionItem.list(header?.get("actions")), { headerMenu = null }) { a ->
            send("sidebar.action", mapOf("action" to a.id, "ids" to emptyList<String>(), "sectionId" to m["sectionId"].str))
        }
    }
    prOthers?.let { row ->
        val badge = row["badge"].obj()
        val others = badge?.get("others").objects.map { ActionItem(it["number"].int.toString(), it["label"].str, it["enabled"].flag(), null, false, null, emptyList(), false) }
        ActionSheet("Pull requests", others, { prOthers = null }) { a -> send("pr.action", mapOf("id" to row["id"].str, "action" to "openOther", "number" to a.id.toLongOrNull())) }
    }
    prMenu?.let { row ->
        val badge = row["badge"].obj()
        ActionSheet(badge?.get("detail").text, ActionItem.list(badge?.get("actions")), { prMenu = null }) { a -> send("pr.action", mapOf("id" to row["id"].str, "action" to a.id, "number" to badge?.get("number").int)) }
    }
    usageTip?.let { tip -> ActionSheet(tip, emptyList(), { usageTip = null }) {} }
}

fun JsonElement?.obj(): JsonObject? = this as? JsonObject

@Composable
private fun SearchField(sidebar: JsonElement?, send: (String, Any?) -> Unit, modifier: Modifier, focusSearch: Boolean = false) {
    val c = LocalYaac.current
    val enabled = sidebar["searchEnabled"].flag(true)
    var text by remember { mutableStateOf(sidebar["search"].str) }
    // Search threads (Commander, Ctrl+K): the controller asks for focus once, then hears it is done.
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    LaunchedEffect(focusSearch) { if (focusSearch) { runCatching { focus.requestFocus() }; send("shell.focusSearchDone", null) } }
    Row(modifier.height(38.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(horizontal = 10.dp).alpha(if (enabled) 1f else 0.5f), verticalAlignment = Alignment.CenterVertically) {
        Icon(YIcon.Search, c.muted, size = 17.dp)
        Spacer(Modifier.width(8.dp))
        Box(Modifier.weight(1f)) {
            if (text.isEmpty()) T("Search", LocalType.current.ui.copy(fontSize = 15.sp), c.muted, maxLines = 1)
            BasicTextField(text, { v -> text = v; send("sidebar.search", mapOf("text" to v)) }, enabled = enabled, singleLine = true,
                textStyle = LocalType.current.ui.copy(fontSize = 15.sp, color = c.text), cursorBrush = SolidColor(c.text), modifier = Modifier.fillMaxWidth().focusRequester(focus))
        }
        if (text.isNotEmpty()) Box(Modifier.size(28.dp).clickable { text = ""; send("sidebar.search", mapOf("text" to "")) }, contentAlignment = Alignment.Center) { Icon(YIcon.Close, c.muted, size = 15.dp) }
    }
}

@Composable
private fun SectionHeader(header: JsonObject?, collapsed: Boolean, outlined: Boolean = false, onToggle: () -> Unit, onPrimary: (ActionItem) -> Unit, onHold: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val label = header?.get("label").str
    val collapsible = label.startsWith("Muted") || label.startsWith("Archived")
    val temporary = header?.get("temporary").flag()
    val offline = header?.get("tone").text == "offline"
    Row(
        Modifier.fillMaxWidth().background(c.panel).then(if (outlined) Modifier.border(2.dp, c.focus, RoundedCornerShape(6.dp)) else Modifier)
            .tapAndHold(onToggle, onHold).heightIn(min = 44.dp).padding(start = 14.dp, end = 14.dp, top = 8.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (collapsible || collapsed) {
            Icon(if (collapsed) YIcon.CaretRight else YIcon.CaretDown, c.text, size = 14.dp); Spacer(Modifier.width(10.dp))
        }
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
            T(label, type.heading.copy(fontSize = 16.sp, fontStyle = if (temporary) FontStyle.Italic else FontStyle.Normal),
                weight = if (collapsible) FontWeight.Normal else FontWeight.SemiBold, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
            header?.get("connection").text?.let { conn ->
                Spacer(Modifier.width(6.dp)); T("· $conn", type.ui.copy(fontSize = 15.sp), c.muted, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
            }
            if (header?.get("olderDaemon").flag()) { Spacer(Modifier.width(6.dp)); Icon(YIcon.Warning, c.warning, size = 15.dp) }
        }
        ActionItem.of(header?.get("primary"))?.let { a -> TextAction(a.label, a.enabled, style = type.small) { onPrimary(a) } }
        header?.get("machine").text?.let { T(it, type.ui.copy(fontSize = 15.sp), if (offline) c.muted else c.text, maxLines = 1) }
    }
}

@Composable
private fun ThreadRow(row: JsonObject, selected: Boolean = false, onMark: (() -> Unit)? = null, drag: DragHandler? = null, onOpen: () -> Unit, onHold: (() -> Unit)?, prAction: (String, Int?) -> Unit, prOthers: () -> Unit, prMenu: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val open = row["selected"].flag()
    val unseen = row["unseen"].flag()
    val tone = row["tone"].text
    val dim = row["muted"].flag() || tone == "offline" || tone == "off"
    val line2 = row["line2"].obj()
    val shape = RoundedCornerShape(6.dp)
    var m = Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 2.dp).clip(shape)
    if (selected) m = m.background(c.focus.copy(alpha = 0.18f)) else if (open) m = m.background(c.selected)
    if (unseen) m = m.border(1.5.dp, c.unread, shape)
    Row(m.tapAndHold(onOpen, onHold).height(IntrinsicSize.Min).heightIn(min = 56.dp).padding(end = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        // The multi-selection bar on the left edge.
        Box(Modifier.width(3.dp).fillMaxHeight().background(if (selected) c.focus else androidx.compose.ui.graphics.Color.Transparent))
        Box(Modifier.width(44.dp).fillMaxHeight().then(if (onMark != null) Modifier.clickable(onClickLabel = if (selected) "Deselect" else "Select", onClick = onMark) else Modifier),
            contentAlignment = Alignment.Center) {
            if (selected) Icon(YIcon.Check, c.text, size = 18.dp) else
            StateMark(tone, row["glyph"].text, fast = row["fast"].flag(), ultrafast = row["ultrafast"].flag())
        }
        Column(Modifier.weight(1f).padding(vertical = 6.dp).alpha(if (dim) 0.62f else 1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                    T(row["title"].str, type.ui.copy(fontSize = 16.sp), if (dim) c.muted else c.text, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                    // In use elsewhere (D140): watched, not sent to.
                    if (row["readOnly"].flag()) { Spacer(Modifier.width(6.dp)); Icon(YIcon.Lock, c.muted, size = 13.dp, strokeWidth = 1.8f) }
                    row["badge"].obj()?.let { badge -> Spacer(Modifier.width(8.dp)); PrBadge(badge, prAction, prOthers, prMenu) }
                }
                row["right"].text?.let { Spacer(Modifier.width(8.dp)); T(it, type.ui, c.text, maxLines = 1) }
            }
            if (line2 != null && (line2["branch"].text != null || (line2["workers"].int ?: 0) > 0)) {
                Spacer(Modifier.height(3.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                        // The harness mark (its real logo in its colour), then the branch.
                        line2["logo"].text?.let { Logo(it, parseColor(line2["logoColor"].text) ?: c.muted, size = 13.dp); Spacer(Modifier.width(6.dp)) }
                        line2["branch"].text?.let { branch ->
                            Icon(YIcon.Branch, c.muted, size = 14.dp, strokeWidth = 2f)
                            Spacer(Modifier.width(6.dp))
                            T(branch, type.mono.copy(fontSize = 14.sp), c.text, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                        }
                        GitCounts(line2)
                    }
                    val workers = line2["workers"].int ?: 0
                    if (workers > 0) { Icon(YIcon.Users, c.muted, size = 15.dp); Spacer(Modifier.width(4.dp)); T(workers.toString(), type.ui, c.text) }
                }
            } else row["detail"].text?.let { T(it, type.small, c.muted, maxLines = 1) }
        }
        // Selected rows carry a drag handle at the right edge (A03): drag onto a group to move.
        if (selected && drag != null && row["draggable"].flag(true)) {
            var origin by remember { mutableStateOf(androidx.compose.ui.geometry.Offset.Zero) }
            val current by rememberUpdatedState(drag)
            Box(Modifier.width(40.dp).fillMaxHeight()
                .onGloballyPositioned { origin = it.positionInRoot() }
                .pointerInput(Unit) {
                    // The handle owns its touch from the first contact, so the row underneath
                    // sees neither a tap nor a touch and hold.
                    awaitEachGesture {
                        val down = awaitFirstDown(requireUnconsumed = false)
                        down.consume()
                        current.move(origin + down.position)
                        while (true) {
                            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: run { current.cancel(); break }
                            if (!change.pressed) { current.drop(); break }
                            change.consume()
                            current.move(origin + change.position)
                        }
                    }
                }, contentAlignment = Alignment.Center) { Icon(YIcon.Grip, c.muted, size = 20.dp, strokeWidth = 3.5f) }
        }
    }
}

/** A drag of the selected rows: the pointer in root coordinates, then a drop or a cancel. */
class DragHandler(val move: (androidx.compose.ui.geometry.Offset) -> Unit, val drop: () -> Unit, val cancel: () -> Unit)


@Composable
fun GitCounts(line2: JsonObject) {
    val c = LocalYaac.current
    val type = LocalType.current.monoSmall.copy(fontSize = 13.sp)
    fun n(k: String) = line2[k].int ?: 0
    val parts = listOf(Triple("+", n("added"), c.success), Triple("~", n("modified"), c.warning), Triple("-", n("deleted"), c.failure),
        Triple("↑", n("ahead"), c.muted), Triple("↓", n("behind"), c.muted))
    for ((p, v, col) in parts) if (v > 0) { Spacer(Modifier.width(6.dp)); T("$p$v", type, col, maxLines = 1) }
}

/** The PR badge (`PullRequestBadge`): tap opens, `+N` lists the others, hold gives its menu. */
@Composable
fun PrBadge(badge: JsonObject, prAction: (String, Int?) -> Unit, prOthers: () -> Unit, prMenu: () -> Unit) {
    val c = LocalYaac.current
    val number = badge["number"].int
    val color = when { badge["checks"].text == "failing" -> c.failure; badge["hint"].text == "merged" -> c.success; else -> c.text }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.clip(RoundedCornerShape(5.dp)).border(1.dp, color, RoundedCornerShape(5.dp))
            .tapAndHold({ if (badge["enabled"].flag()) prAction("open", number) }, prMenu).padding(horizontal = 5.dp, vertical = 1.dp)) {
            T("PR# $number", LocalType.current.monoSmall, color, maxLines = 1)
        }
        badge["more"].text?.let { more -> Spacer(Modifier.width(4.dp)); T(more, LocalType.current.small, c.muted, modifier = Modifier.clickable(onClick = prOthers).padding(4.dp)) }
    }
}

@Composable
private fun Footer(sidebar: JsonElement?, shell: JsonElement?, send: (String, Any?) -> Unit, onUsage: (String) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    Row(
        Modifier.fillMaxWidth().navigationBarsPadding().padding(8.dp).clip(RoundedCornerShape(8.dp)).background(c.raised).padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            for (group in sidebar["usage"].objects) {
                val tint = parseColor(group["logoColor"].text) ?: c.text
                val bars = group["bars"].objects
                Row(Modifier.tapAndHold({ group["tooltip"].text?.let(onUsage) }, { group["tooltip"].text?.let(onUsage) }).alpha(if (group["stale"].flag()) 0.55f else 1f),
                    verticalAlignment = Alignment.CenterVertically) {
                    group["logo"].text?.let { Logo(it, tint, size = 16.dp) }
                    Spacer(Modifier.width(6.dp))
                    T(group["label"].str.replaceFirstChar { it.uppercase() }, type.ui, maxLines = 1, modifier = Modifier.width(54.dp))
                    // The phone shows the window that is closest to its limit; hold for the rest.
                    val bar = bars.maxByOrNull { b -> b["segments"].list.sumOf { (it as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0 } } ?: return@Row
                    SegmentBar(bar["segments"].list.map { (it as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0 }, bar["near"].flag())
                    Spacer(Modifier.width(8.dp))
                    T(bar["text"].str, type.small, c.text, maxLines = 1)
                }
            }
        }
        Box(Modifier.width(1.dp).height(36.dp).background(c.rule))
        Row(Modifier.clip(RoundedCornerShape(6.dp)).clickable { send("shell.settings", null) }.padding(horizontal = 8.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(YIcon.Settings, c.text, size = 18.dp, strokeWidth = 1.7f)
        }
        Box(Modifier.width(1.dp).height(36.dp).background(c.rule))
        val hub = shell["hub"].obj()
        Row(Modifier.clip(RoundedCornerShape(6.dp)).tapAndHold({ sidebar["footer"]["detail"].text?.let(onUsage) }, { sidebar["footer"]["detail"].text?.let(onUsage) }).padding(start = 8.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Icon(YIcon.Hub, if (hub?.get("reachable").flag(true)) c.text else c.failure, size = 17.dp, strokeWidth = 1.7f)
            Spacer(Modifier.width(6.dp))
            T("↑${hub?.get("up").int ?: 0} ↓${hub?.get("down").int ?: 0}", type.ui, maxLines = 1)
        }
    }
}

/** The selection bar that replaces the top bar (A03): count · Archive · Mute · Move · ⋮ · ×. */
@Composable
private fun SelectionBar(count: Int, rangeMode: Boolean, bulk: List<ActionItem>, onClear: () -> Unit, onBulk: (ActionItem) -> Unit, onMove: () -> Unit, onMore: () -> Unit) {
    val type = LocalType.current
    val c = LocalYaac.current
    Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 2.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(YIcon.Close, "Clear selection", onClick = onClear)
        T(if (rangeMode) "Tap the range end" else "$count selected", type.title.copy(fontSize = 17.sp), maxLines = 1, modifier = Modifier.weight(1f))
        for (id in listOf("bulk.archive", "bulk.mute")) bulk.firstOrNull { it.id == id }?.let { a ->
            TextAction(a.label.removeSuffix("…"), a.enabled, style = type.ui.copy(fontSize = 15.sp)) { onBulk(a) }
        }
        TextAction("Move", style = type.ui.copy(fontSize = 15.sp), onClick = onMove)
        IconButton(YIcon.More, "More", strokeWidth = 3f, onClick = onMore)
    }
}
