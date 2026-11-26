package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.FrameJson
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.list
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * What the side pane receives outside the views: file documents in chunks (`filesDocument`)
 * and terminal frames (`terminalFrame`), as WPF's `ReceiveFileDocument` and `ReceiveTerminal`.
 */
@Stable
class SidePaneState(private val send: (String, Any?) -> Unit) {
    private val chunks = HashMap<Long, StringBuilder>()
    /** Assembled documents by id: `{file?, old?, new?, rows?, sideBySide}`. */
    val documents = mutableStateMapOf<Long, JsonObject>()
    val terminals = mutableStateMapOf<Long, TerminalGrid>()

    fun document(frame: JsonObject) {
        val id = (frame["id"] as? JsonPrimitive)?.content?.toLongOrNull() ?: return
        if (frame["start"].flag()) chunks[id] = StringBuilder()
        val buffer = chunks[id] ?: return
        buffer.append(frame["chunk"].str)
        if (frame["end"].flag()) {
            chunks.remove(id)
            runCatching { FrameJson.parseToJsonElement(buffer.toString()) as? JsonObject }.getOrNull()?.let { documents[id] = it }
            // Keep a few recent documents only.
            if (documents.size > 6) documents.keys.sorted().take(documents.size - 6).forEach { documents.remove(it) }
        }
    }

    fun terminal(frame: JsonObject) {
        val tab = (frame["tab"] as? JsonPrimitive)?.content?.toLongOrNull() ?: return
        val grid = terminals.getOrPut(tab) { TerminalGrid() }
        grid.apply(frame)
        // Acknowledge each frame: the controller paces its frames on these (WPF `terminal.ack`).
        send("terminal.ack", mapOf("tab" to tab, "panel" to frame["panel"].flag(), "frame" to (frame["frame"] as? JsonPrimitive)?.content?.toLongOrNull()))
    }
}

/** One terminal's rows of runs `[col, width, ink, paper, flags, text]`, the height, the cursor. */
@Stable
class TerminalGrid {
    val rows = mutableStateMapOf<Int, JsonArray>()
    var height by mutableStateOf(0)
    var cursor by mutableStateOf<JsonArray?>(null)

    fun apply(frame: JsonObject) {
        val h = frame["height"].int ?: height
        if (h != height) { rows.keys.filter { it >= h }.forEach { rows.remove(it) }; height = h }
        for (r in frame["rows"].list) {
            val pair = r as? JsonArray ?: continue
            val index = (pair.getOrNull(0) as? JsonPrimitive)?.content?.toIntOrNull() ?: continue
            rows[index] = pair.getOrNull(1) as? JsonArray ?: JsonArray(emptyList())
        }
        cursor = frame["cursor"] as? JsonArray
    }
}

/** The tab row (compact width, P375 "tabs at compact width"): Chat, the side tabs, and ⊕. */
@Composable
fun SideTabs(browser: JsonElement?, threadId: String, send: (String, Any?) -> Unit, adding: Boolean, setAdding: (Boolean) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val tabs = browser["tabs"].objects
    val showChat = browser["active"].isNullishPrimitive() || browser["chat"].flag()
    var menu by remember { mutableStateOf<JsonObject?>(null) }
    if (tabs.isNotEmpty()) Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        run {
            SideTab("Chat", showChat) { send("browser.chat", mapOf("thread" to threadId)) }
            for (t in tabs) {
                val id = (t["id"] as? JsonPrimitive)?.content?.toLongOrNull()
                val selected = !showChat && browser["active"].scalarLong() == id
                val label = t["title"].text ?: t["path"].text?.substringAfterLast('/') ?: when (t["kind"].str) { "files" -> "Files"; "terminal" -> "Terminal"; else -> t["url"].text ?: "New tab" }
                SideTab(label, selected, mark = t["mark"].text, onHold = { menu = t }) { send("browser.select", mapOf("thread" to threadId, "tab" to id)) }
            }
        }
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).clickable { setAdding(true) }, contentAlignment = Alignment.Center) { Icon(YIcon.CirclePlus, c.text, size = 20.dp, strokeWidth = 1.7f) }
    }
    if (adding) {
        // Each kind keeps the controller's reason when it is unavailable (no browser host, …).
        val shells = browser["shells"].objects.map { ActionItem("shell:" + it["path"].str, it["name"].str, true, it["path"].text, false, null, emptyList(), false) }
        val items = listOf(
            ActionItem("files", "Files", browser["filesReason"].text == null, browser["filesReason"].text, false, null, emptyList(), false),
            ActionItem("terminal", "Terminal", browser["terminalReason"].text == null, browser["terminalReason"].text, false, null,
                if (shells.size > 1) listOf(ActionItem("terminal", "Default shell", true, null, false, null, emptyList(), false)) + shells else emptyList(), false),
            ActionItem("browser", "Browser", browser["reason"].text == null, browser["reason"].text, false, null, emptyList(), false),
        )
        ActionSheet("Open beside the chat", items, { setAdding(false) }) { a ->
            when {
                a.id == "files" -> send("files.new", mapOf("thread" to threadId))
                a.id == "terminal" -> send("terminal.new", mapOf("thread" to threadId))
                a.id.startsWith("shell:") -> send("terminal.new", mapOf("thread" to threadId, "shell" to a.id.removePrefix("shell:")))
                a.id == "browser" -> send("browser.new", mapOf("thread" to threadId))
            }
        }
    }
    menu?.let { t ->
        ActionSheet(t["title"].text ?: t["path"].text ?: t["kind"].str, listOf(ActionItem("close", "Close tab", true, null, false, null, emptyList(), false)), { menu = null }) {
            send("browser.close", mapOf("thread" to threadId, "tab" to (t["id"] as? JsonPrimitive)?.content?.toLongOrNull()))
        }
    }
}

private fun JsonElement?.isNullishPrimitive() = this == null || this is kotlinx.serialization.json.JsonNull
private fun JsonElement?.scalarLong() = (this as? JsonPrimitive)?.content?.toLongOrNull()

@Composable
private fun SideTab(label: String, selected: Boolean, mark: String? = null, onHold: (() -> Unit)? = null, onClick: () -> Unit) {
    val c = LocalYaac.current
    Row(Modifier.padding(end = 4.dp).clip(RoundedCornerShape(6.dp)).background(if (selected) c.selected else Color.Transparent)
        .tapAndHold(onClick, onHold).heightIn(min = 36.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        T(label, LocalType.current.ui.copy(fontSize = 15.sp), if (selected) c.text else c.muted, maxLines = 1)
        mark?.let { Spacer(Modifier.width(5.dp)); T(it, LocalType.current.monoSmall, gitColor(it, c)) }
    }
}

fun gitColor(mark: String, c: YaacColors): Color = when (mark.firstOrNull()) {
    'A', 'U', '?' -> c.success
    'M', 'R' -> c.warning
    'D', '!' -> c.failure
    else -> c.muted
}

/** The active side tab's content (compact width replaces the chat with it). */
@Composable
fun SideContent(browser: JsonElement?, threadId: String, side: SidePaneState, send: (String, Any?) -> Unit, modifier: Modifier = Modifier) {
    val c = LocalYaac.current
    val active = browser["active"].scalarLong()
    val tab = browser["tabs"].objects.firstOrNull { it["id"].scalarLong() == active }
    Box(modifier.fillMaxSize().background(c.canvas)) {
        when (tab?.get("kind").str) {
            // (the terminal pads its key row; the lists scroll under the navigation bar)
            "files" -> FilesTree(browser["files"].obj(), threadId, active, send)
            "file", "diff" -> FileDocument(browser["files"].obj(), tab!!, threadId, active, side, send)
            "terminal" -> Terminal(threadId, active, side.terminals[active ?: -1], send)
            "browser" -> Column(Modifier.padding(20.dp)) {
                T("Browser", LocalType.current.heading)
                T(browser["reason"].text ?: tab?.get("error").text ?: "The page is shown on the desktop window.", LocalType.current.ui, c.muted, modifier = Modifier.padding(top = 6.dp))
                tab?.get("url").text?.let { url -> TextAction("Open in the browser") { send("browser.external", mapOf("thread" to threadId, "tab" to active)) }; T(url, LocalType.current.monoSmall, c.muted) }
            }
            else -> {}
        }
    }
}

/** Files (A23): the worktree tree with git marks; folders expand, files open, changed files diff. */
@Composable
private fun FilesTree(view: JsonObject?, threadId: String, tab: Long?, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val folders = view?.get("folders").obj()
    var menu by remember { mutableStateOf<Pair<String, JsonObject>?>(null) }
    fun cmd(name: String, data: Map<String, Any?>) = send("files.$name", mapOf("thread" to threadId, "tab" to tab, "data" to data))
    // The visible rows in order: each folder's entries, with expanded folders' contents after them.
    val rows = ArrayList<Triple<String, Int, JsonObject>>()
    fun walk(path: String, depth: Int) {
        for (e in folders?.get(path).objects) {
            val full = if (path.isEmpty()) e["name"].str else "$path/${e["name"].str}"
            rows += Triple(full, depth, e)
            if (e["directory"].flag() && e["expanded"].flag()) walk(full, depth + 1)
        }
    }
    walk("", 0)
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            TextAction("Refresh", style = type.small) { cmd("refresh", emptyMap()) }
            TextAction("Collapse all", view?.get("canCollapse").flag(), style = type.small) { cmd("collapse", emptyMap()) }
            Spacer(Modifier.weight(1f))
            val shown = view?.get("showIgnored").flag()
            TextAction(if (shown) "Hide ignored" else "Show ignored", style = type.small) { cmd("ignored", mapOf("show" to !shown)) }
        }
        view?.get("status").text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
            items(rows, key = { it.first }) { (path, depth, e) ->
                val dir = e["directory"].flag()
                Row(Modifier.fillMaxWidth().tapAndHold({
                    if (dir) cmd("expand", mapOf("path" to path, "expanded" to !e["expanded"].flag())) else cmd("open", mapOf("path" to path))
                }, { if (!dir) menu = path to e }).heightIn(min = 40.dp).padding(start = (12 + 16 * depth).dp, end = 12.dp)
                    .alpha(if (e["ignored"].flag()) 0.5f else 1f), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.width(18.dp)) { if (dir) Icon(if (e["expanded"].flag()) YIcon.CaretDown else YIcon.CaretRight, c.muted, size = 11.dp) }
                    T(e["name"].str + if (dir) "/" else "", type.mono, if (e["changed"].flag() && dir) c.warning else c.text, maxLines = 1, modifier = Modifier.weight(1f))
                    e["mark"].text?.let { T(it, type.mono, gitColor(it, c)) }
                }
            }
        }
    }
    menu?.let { (path, e) ->
        val items = listOf(ActionItem("open", "Open", true, null, false, null, emptyList(), false)) +
            listOfNotNull(e["mark"].text?.let { ActionItem("diff", "Show changes", true, it, false, null, emptyList(), false) }) +
            listOf(ActionItem("duplicate", "Open in a new tab", true, null, false, null, emptyList(), false))
        ActionSheet(path, items, { menu = null }) { a ->
            when (a.id) {
                "open" -> cmd("open", mapOf("path" to path))
                "diff" -> cmd("diff", mapOf("path" to path))
                "duplicate" -> cmd("open", mapOf("path" to path, "duplicate" to true))
            }
        }
    }
}

/** A file or a diff (A23), from the assembled `filesDocument`. Unified at compact width. */
@Composable
private fun FileDocument(view: JsonObject?, tab: JsonObject, threadId: String, tabId: Long?, side: SidePaneState, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val docId = view?.get("document").scalarLong()
    val doc = docId?.let { side.documents[it] }
    // The phone shows diffs unified; ask the controller for the unified rows.
    LaunchedEffect(view?.get("sideBySide")) {
        if (tab["kind"].str == "diff" && view?.get("sideBySide").flag()) send("files.mode", mapOf("thread" to threadId, "tab" to tabId, "data" to mapOf("sideBySide" to false)))
    }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            T(tab["path"].str, type.mono, maxLines = 1, modifier = Modifier.weight(1f))
            tab["mark"].text?.let { T(it, type.mono, gitColor(it, c)) }
            if (tab["kind"].str == "diff") view?.get("changeCount").int?.let { T("  $it changes", type.small, c.muted) }
        }
        (view?.get("status").text)?.let { T(it, type.ui, c.muted, modifier = Modifier.padding(16.dp)) }
        if (doc == null) { if (view?.get("status").text == null) T("Loading…", type.small, c.muted, modifier = Modifier.padding(16.dp)); return }
        val lines: List<AnnotatedString> = remember(doc) { codeLines(doc, c) }
        Box(Modifier.fillMaxSize().horizontalScroll(rememberScrollState())) {
            LazyColumn(Modifier.width(1400.dp).fillMaxSize()) {
                items(lines.size) { i -> BasicText(lines[i], style = type.mono.copy(fontSize = 12.sp, color = c.text), softWrap = false, modifier = Modifier.padding(horizontal = 8.dp)) }
            }
        }
    }
}

private fun scopeColor(scope: String, c: YaacColors): Color? = when {
    scope.contains("comment") -> c["YaacCodeComment"]
    scope.contains("string") -> c["YaacCodeString"]
    scope.contains("keyword") || scope.contains("storage") -> c["YaacCodeKeyword"]
    scope.contains("number") || scope.contains("constant") -> c["YaacCodeNumber"]
    scope.contains("type") || scope.contains("entity") -> c["YaacCodeType"]
    else -> null
}

/** A source's lines, coloured by its syntax spans (UTF-16 ranges, as the wire carries them). */
private fun sourceLines(src: JsonObject?, c: YaacColors): List<AnnotatedString> {
    val text = src?.get("text").str
    val styles = src?.get("styles").obj()
    return text.split('\n').mapIndexed { i, line ->
        val spans = styles?.get(i.toString())?.get("spans").objects
        if (spans.isEmpty()) AnnotatedString(line) else buildAnnotatedString {
            append(line)
            for (s in spans) {
                val a = s["start"].int ?: continue; val b = s["end"].int ?: continue
                scopeColor(s["scope"].str, c)?.let { col -> if (a in 0..b && b <= line.length) addStyle(SpanStyle(color = col), a, b) }
            }
        }
    }
}

private fun codeLines(doc: JsonObject, c: YaacColors): List<AnnotatedString> {
    doc["file"].obj()?.let { file ->
        val lines = sourceLines(file, c)
        val w = lines.size.toString().length
        return lines.mapIndexed { i, l -> buildAnnotatedString { withStyle(SpanStyle(color = c.muted)) { append((i + 1).toString().padStart(w) + "  ") }; append(l) } }
    }
    val old = sourceLines(doc["old"].obj(), c)
    val new = sourceLines(doc["new"].obj(), c)
    val out = ArrayList<AnnotatedString>()
    for (r in doc["rows"].list) {
        val row = r as? JsonArray ?: continue
        val o = (row.getOrNull(0) as? JsonPrimitive)?.content?.toIntOrNull()
        val n = (row.getOrNull(1) as? JsonPrimitive)?.content?.toIntOrNull()
        val count = (row.getOrNull(2) as? JsonPrimitive)?.content?.toIntOrNull() ?: 0
        out += when {
            count > 0 -> buildAnnotatedString { withStyle(SpanStyle(color = c.muted)) { append("   ⋯ $count unchanged lines") } }
            o != null && n != null -> buildAnnotatedString { append("   "); append(new.getOrElse(n) { AnnotatedString("") }) }
            o != null -> buildAnnotatedString { withStyle(SpanStyle(background = c["YaacDiffRemoved"])) { withStyle(SpanStyle(color = c.failure)) { append(" - ") }; append(old.getOrElse(o) { AnnotatedString("") }) } }
            n != null -> buildAnnotatedString { withStyle(SpanStyle(background = c["YaacDiffAdded"])) { withStyle(SpanStyle(color = c.success)) { append(" + ") }; append(new.getOrElse(n) { AnnotatedString("") }) } }
            else -> AnnotatedString("")
        }
    }
    return out
}

/**
 * The terminal (A24): the controller's grid, sized to the pane (`terminal.resize`), with the
 * keyboard sent as text and keys, and a row of the keys a touch keyboard lacks.
 */
@Composable
private fun Terminal(threadId: String, tab: Long?, grid: TerminalGrid?, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val style = type.mono.copy(fontSize = 12.sp, lineHeight = 15.sp)
    val measurer = rememberTextMeasurer()
    val cell = remember(style) { measurer.measure("MMMMMMMMMM", style).size.let { it.width / 10f to it.height.toFloat() } }
    var ctrl by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    fun input(name: String, data: Map<String, Any?>) = send("terminal.$name", mapOf("thread" to threadId, "tab" to tab, "panel" to false, "data" to data))
    val sentinel = "​"
    var field by remember { mutableStateOf(TextFieldValue(sentinel, TextRange(1))) }
    Column(Modifier.fillMaxSize().background(c.input)) {
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth().clickable { runCatching { focus.requestFocus() } }.padding(6.dp)) {
            val density = LocalDensity.current
            val cols = (with(density) { maxWidth.toPx() } / cell.first).toInt().coerceAtLeast(2)
            val rows = (with(density) { maxHeight.toPx() } / cell.second).toInt().coerceAtLeast(1)
            LaunchedEffect(tab, cols, rows) {
                input("resize", mapOf("cols" to cols, "rows" to rows, "cellWidth" to cell.first.toInt(), "cellHeight" to cell.second.toInt(), "dark" to c.dark))
                input("focus", mapOf("focused" to true))
            }
            if (grid == null) T("Starting the terminal…", type.small, c.muted)
            else Column {
                val cursor = grid.cursor
                val cc = (cursor?.getOrNull(0) as? JsonPrimitive)?.content?.toIntOrNull()
                val cr = (cursor?.getOrNull(1) as? JsonPrimitive)?.content?.toIntOrNull()
                val visible = (cursor?.getOrNull(2) as? JsonPrimitive)?.content != "hidden"
                for (r in 0 until grid.height) {
                    BasicText(terminalRow(grid.rows[r], if (visible && r == cr) cc else null, c), style = style.copy(color = c.text), softWrap = false, maxLines = 1)
                }
            }
            // The keyboard: an invisible field keeps a one-character sentinel, so a deletion is
            // seen as Backspace and anything typed is sent as text.
            BasicTextField(field, { v ->
                val text = v.text
                when {
                    text.length < sentinel.length -> input("key", mapOf("key" to "Backspace", "mods" to 0))
                    text.length > sentinel.length -> {
                        val typed = text.removePrefix(sentinel)
                        if (ctrl && typed.length == 1) { input("key", mapOf("key" to typed.lowercase(), "mods" to 4)); ctrl = false }
                        else input("text", mapOf("text" to typed, "mods" to 0))
                    }
                }
                field = TextFieldValue(sentinel, TextRange(1))
            }, modifier = Modifier.size(1.dp).alpha(0f).focusRequester(focus).onPreviewKeyEvent { e ->
                if (e.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                val name = when (e.key) { Key.Enter -> "Enter"; Key.Tab -> "Tab"; Key.Escape -> "Escape"; Key.DirectionUp -> "Up"; Key.DirectionDown -> "Down"
                    Key.DirectionLeft -> "Left"; Key.DirectionRight -> "Right"; else -> null } ?: return@onPreviewKeyEvent false
                input("key", mapOf("key" to name, "mods" to 0)); true
            }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Send, autoCorrectEnabled = false),
                keyboardActions = KeyboardActions(onSend = { input("key", mapOf("key" to "Enter", "mods" to 0)) }))
        }
        // The keys a touch keyboard lacks.
        Row(Modifier.fillMaxWidth().background(c.panel).navigationBarsPadding().horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            TextAction("Ctrl", color = if (ctrl) c.warning else c.text, style = type.mono) { ctrl = !ctrl; runCatching { focus.requestFocus() } }
            for ((label, key) in listOf("Esc" to "Escape", "Tab" to "Tab", "↑" to "Up", "↓" to "Down", "←" to "Left", "→" to "Right", "⏎" to "Enter"))
                TextAction(label, style = type.mono) { input("key", mapOf("key" to key, "mods" to 0)) }
            TextAction("Restart", style = type.small) { input("restart", emptyMap()) }
        }
    }
}

/** One row of runs, laid out by column, with the cursor cell inverted. */
private fun terminalRow(runs: JsonArray?, cursorCol: Int?, c: YaacColors): AnnotatedString = buildAnnotatedString {
    var col = 0
    for (r in runs.orEmpty()) {
        val run = r as? JsonArray ?: continue
        val start = (run.getOrNull(0) as? JsonPrimitive)?.content?.toIntOrNull() ?: continue
        val ink = (run.getOrNull(2) as? JsonPrimitive)?.content?.toLongOrNull()
        val paper = (run.getOrNull(3) as? JsonPrimitive)?.content?.toLongOrNull() ?: -1
        val flags = (run.getOrNull(4) as? JsonPrimitive)?.content?.toIntOrNull() ?: 0
        val text = (run.getOrNull(5) as? JsonPrimitive)?.content ?: ""
        if (start > col) { append(" ".repeat(start - col)); col = start }
        withStyle(SpanStyle(
            color = ink?.let { Color(0xFF000000 or (it and 0xFFFFFF)) } ?: Color.Unspecified,
            background = if (paper >= 0) Color(0xFF000000 or (paper and 0xFFFFFF)) else Color.Unspecified,
            fontWeight = if (flags and 1 != 0) FontWeight.Bold else null,
            fontStyle = if (flags and 2 != 0) FontStyle.Italic else null,
            textDecoration = when { flags and 4 != 0 -> TextDecoration.Underline; flags and 32 != 0 -> TextDecoration.LineThrough; else -> null },
        )) { append(text) }
        col += text.length
    }
    if (cursorCol != null) {
        if (cursorCol >= length) append(" ".repeat(cursorCol - length + 1))
        addStyle(SpanStyle(background = c.text, color = c.canvas), cursorCol, cursorCol + 1)
    }
}

/** Whether the chat is shown (no side tab active, or the chat chosen). */
fun showsChat(browser: JsonElement?): Boolean = browser["active"].isNullishPrimitive() || browser["chat"].flag() || browser["tabs"].objects.isEmpty()
