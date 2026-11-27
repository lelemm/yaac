package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.FrameJson
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import dev.yaac.android.ui.pane.ComposerCaret
import dev.yaac.android.ui.pane.FilePane
import dev.yaac.android.ui.pane.PaneGlyph
import dev.yaac.android.ui.pane.PaneIcon
import dev.yaac.android.ui.pane.Rename
import dev.yaac.android.ui.pane.Shortcut
import dev.yaac.android.ui.pane.TabInfo
import dev.yaac.android.ui.pane.TabMark
import dev.yaac.android.ui.pane.Tabs
import dev.yaac.android.ui.pane.TermPaste
import dev.yaac.android.ui.pane.TermScreen
import dev.yaac.android.ui.pane.TerminalPane
import dev.yaac.android.ui.pane.TerminalWire
import dev.yaac.android.ui.pane.glyphOf
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * What the side pane receives outside the views, as WPF's `ReceiveFileDocument` and
 * `ReceiveTerminal`: file documents in chunks (`filesDocument`), terminal frames
 * (`terminalFrame`, acknowledged one by one), the change the controller moved to
 * (`filesChange`), a multi-line paste to confirm (`terminalPasteConfirm`), a request to read the
 * clipboard for the shell's paste key (`terminalReadPaste`) and quiet notices (`terminalNotice`).
 */
@Stable
class SidePaneState(private val send: (String, Any?) -> Unit) {
    private val chunks = HashMap<Long, StringBuilder>()
    /**
     * At compact width the pane replaces the chat, so the phone remembers when Chat was chosen:
     * the controller's `chat` flag only applies in full-screen mode (the desktop shows both).
     * Cleared when another tab becomes active.
     */
    var chatChosen by mutableStateOf(false)
    var chatChosenFor by mutableStateOf<Long?>(null)
    /** Assembled documents by id: `{file?, old?, new?, rows?, sideBySide}`. */
    val documents = mutableStateMapOf<Long, JsonObject>()
    /** Side-pane terminals by tab id (the panel under the composer is not shown on a phone). */
    val terminals = mutableStateMapOf<Long, TermScreen>()
    /** The change the controller moved to: `filesChange` {id, row, ordinal}, with a serial so a repeat still scrolls. */
    var change by mutableStateOf<FilesChange?>(null)
    /** A multi-line paste waiting for the user's decision (`terminalPasteConfirm`). */
    var pasteConfirm by mutableStateOf<JsonObject?>(null)
    /** The shell's paste key was pressed (`terminalReadPaste` {tab, panel}): read the clipboard and paste. */
    var readPaste by mutableStateOf<Long?>(null)
    /** A quiet notice under the pane ("Copied by the terminal", a refused clipboard read). */
    var notice by mutableStateOf<String?>(null)
    /** Key bindings (`shortcuts` view), for the focused terminal (K04). */
    var shortcuts by mutableStateOf<List<Shortcut>>(emptyList())
    /** The open composer's key (`composer` view), whose saved caret Add to composer uses. */
    var composerKey: String? = null
    private var changes = 0

    /** The `caret` field of `files.add` / `terminal.add`. */
    fun caret(): Int? = ComposerCaret.caret(composerKey)

    data class FilesChange(val id: Long, val row: Int, val ordinal: Int?, val serial: Int)

    fun change(frame: JsonObject) {
        val id = (frame["id"] as? JsonPrimitive)?.content?.toLongOrNull() ?: return
        val row = frame["row"].int ?: return
        change = FilesChange(id, row, frame["ordinal"].int, ++changes)
    }

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
        val f = TerminalWire.decode(frame) ?: return
        if (!f.panel) terminals[f.tab] = (terminals[f.tab] ?: TermScreen()).apply(f)
        // Acknowledge each frame: the controller paces its frames on these (WPF `terminal.ack`).
        send("terminal.ack", mapOf("tab" to f.tab, "panel" to f.panel, "frame" to f.serial))
    }

    /** `terminalReadPaste`: only the side pane's terminals are on screen here. */
    fun readPaste(frame: JsonObject) {
        if (frame["panel"].flag()) return
        readPaste = (frame["tab"] as? JsonPrimitive)?.content?.toLongOrNull()
    }

    fun shortcuts(view: JsonElement?) { shortcuts = Shortcut.list(view) }
}

private fun item(id: String, label: String, enabled: Boolean = true, detail: String? = null, children: List<ActionItem> = emptyList(), danger: Boolean = false) =
    ActionItem(id, label, enabled, detail, danger, null, children, false)

/**
 * The tab strip (B02, compact width, P375 "tabs at compact width"): the pinned Chat tab, then
 * the side tabs (icon, ellipsized title, marks, × on the selected one) scrolling, an overflow
 * list, full screen, and ⊕.
 */
@Composable
fun SideTabs(browser: JsonElement?, threadId: String, send: (String, Any?) -> Unit, adding: Boolean, side: SidePaneState? = null, setAdding: (Boolean) -> Unit) {
    val c = LocalYaac.current
    val tabs = browser["tabs"].objects
    val showChat = showsChat(browser, side)
    var menu by remember { mutableStateOf<TabInfo?>(null) }
    var renaming by remember { mutableStateOf<TabInfo?>(null) }
    var overflow by remember { mutableStateOf(false) }
    val infos = tabs.map { Tabs.info(it) }
    if (tabs.isNotEmpty()) Row(Modifier.fillMaxWidth().padding(start = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        // Chat stays pinned at the start, outside the scrolling tabs, with the thread's mark.
        SideTab(Tabs.info(JsonObject(mapOf("kind" to JsonPrimitive("chat")))).copy(title = "Chat", icon = dev.yaac.android.ui.pane.TabIcon.Chat), showChat,
            dot = if (browser["chatMarked"].flag()) c.accent else null) { side?.chooseChat(browser); send("browser.chat", mapOf("thread" to threadId)) }
        Row(Modifier.weight(1f).horizontalScroll(rememberScrollState()), verticalAlignment = Alignment.CenterVertically) {
            for (info in infos) {
                val selected = !showChat && browser["active"].scalarLong() == info.id
                SideTab(info, selected, onClose = { send("browser.close", mapOf("thread" to threadId, "tab" to info.id)) }.takeIf { selected },
                    onHold = { menu = info }) { side?.chatChosen = false; send("browser.select", mapOf("thread" to threadId, "tab" to info.id)) }
            }
        }
        if (tabs.size > 2) Box(Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).clickable(onClickLabel = "All tabs") { overflow = true }
            .semantics { contentDescription = "All tabs (${tabs.size + 1})" }, contentAlignment = Alignment.Center) { PaneIcon(PaneGlyph.More, c.text, size = 20.dp) }
        if (!showChat) {
            val full = browser["fullScreen"].flag()
            Box(Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).clickable { send("browser.fullScreen", mapOf("thread" to threadId, "fullScreen" to !full)) }
                .semantics { contentDescription = if (full) "Restore" else "Full screen" }, contentAlignment = Alignment.Center) {
                PaneIcon(if (full) PaneGlyph.Restore else PaneGlyph.Expand, c.text, size = 18.dp)
            }
        }
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).clickable { setAdding(true) }.semantics { contentDescription = "Open beside the chat" },
            contentAlignment = Alignment.Center) { Icon(YIcon.CirclePlus, c.text, size = 20.dp, strokeWidth = 1.7f) }
    }
    if (adding) {
        // Each kind keeps the controller's reason when it is unavailable (no browser host, …).
        val shells = browser["shells"].objects.map { item("shell:" + it["path"].str, it["name"].str, detail = it["path"].text) }
        val items = listOf(
            item("files", "Files", browser["filesReason"].text == null, browser["filesReason"].text),
            item("terminal", "Terminal", browser["terminalReason"].text == null, browser["terminalReason"].text,
                if (shells.size > 1) listOf(item("terminal", "Default shell")) + shells else emptyList()),
            item("browser", "Browser", browser["reason"].text == null, browser["reason"].text),
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
    if (overflow) {
        val items = listOf(item("chat", "Chat", detail = if (showChat) "Shown" else null)) + infos.map { i ->
            item("tab:${i.id}", i.title, detail = listOfNotNull(i.detail, Tabs.describe(i, !showChat && browser["active"].scalarLong() == i.id).substringAfter(", ${i.title}").removePrefix(", ").takeIf { it.isNotEmpty() }).joinToString(" · ").ifEmpty { null })
        }
        ActionSheet("Tabs", items, { overflow = false }) { a ->
            if (a.id == "chat") { side?.chooseChat(browser); send("browser.chat", mapOf("thread" to threadId)) }
            else { side?.chatChosen = false; send("browser.select", mapOf("thread" to threadId, "tab" to a.id.removePrefix("tab:").toLongOrNull())) }
        }
    }
    menu?.let { t ->
        val terminal = t.kind == "terminal"
        val items = listOfNotNull(
            item("rename", "Rename").takeIf { terminal },
            item("clearName", "Use the shell's title").takeIf { terminal },
            item("restart", "Restart").takeIf { terminal && Tabs.canRestart(t) },
            item("close", "Close tab", danger = false),
        )
        ActionSheet(listOfNotNull(t.title, t.detail).joinToString(" · "), items, { menu = null }) { a ->
            when (a.id) {
                "rename" -> renaming = t
                "clearName" -> send("terminal.rename", mapOf("thread" to threadId, "tab" to t.id, "panel" to false, "data" to mapOf("name" to "")))
                "restart" -> send("terminal.restart", mapOf("thread" to threadId, "tab" to t.id, "panel" to false, "data" to emptyMap<String, Any?>()))
                "close" -> send("browser.close", mapOf("thread" to threadId, "tab" to t.id))
            }
        }
    }
    renaming?.let { t -> RenameDialog(t.title, { renaming = null }) { data -> send("terminal.rename", mapOf("thread" to threadId, "tab" to t.id, "panel" to false, "data" to data)) } }
}

/** Rename a terminal (E02, P343): the title selected; Save keeps it, Cancel or Back discards, Clear restores the shell's title. */
@Composable
private fun RenameDialog(original: String, onDone: () -> Unit, onSave: (Map<String, Any?>) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    var value by remember { mutableStateOf(TextFieldValue(original, TextRange(0, original.length))) }
    val focus = remember { FocusRequester() }
    fun save() { Rename.data(original, value.text)?.let(onSave); onDone() }
    androidx.compose.ui.window.Dialog(onDismissRequest = onDone) {
        Column(Modifier.clip(RoundedCornerShape(8.dp)).background(c.raised).padding(20.dp)) {
            T("Rename terminal", type.title)
            BasicTextField(value, { value = it }, singleLine = true, textStyle = type.ui.copy(color = c.text, fontSize = 15.sp), cursorBrush = SolidColor(c.text),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { save() }),
                modifier = Modifier.padding(top = 12.dp).fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(c.input).padding(12.dp).focusRequester(focus)
                    .onPreviewKeyEvent { e ->
                        if (e.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                        when (e.key) { Key.Enter -> { save(); true }; Key.Escape -> { onDone(); true }; else -> false }
                    })
            T("An empty name goes back to the shell's title.", type.small, c.muted, modifier = Modifier.padding(top = 6.dp))
            Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.End) {
                TextAction("Clear") { value = TextFieldValue("") }
                TextAction("Cancel") { onDone() }
                PrimaryButton("Save") { save() }
            }
        }
    }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
}

private fun JsonElement?.isNullishPrimitive() = this == null || this is kotlinx.serialization.json.JsonNull
internal fun JsonElement?.scalarLong() = (this as? JsonPrimitive)?.content?.toLongOrNull()

@Composable
private fun SideTab(info: TabInfo, selected: Boolean, dot: Color? = null, onClose: (() -> Unit)? = null, onHold: (() -> Unit)? = null, onClick: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val tint = if (selected) c.text else c.muted
    Row(Modifier.padding(end = 4.dp).clip(RoundedCornerShape(6.dp)).background(if (selected) c.selected else Color.Transparent)
        .tapAndHold(onClick, onHold).semantics { contentDescription = Tabs.describe(info, selected) }
        .heightIn(min = 36.dp).padding(start = 10.dp, end = if (onClose != null) 2.dp else 10.dp), verticalAlignment = Alignment.CenterVertically) {
        when (info.mark) {
            // Loading replaces the icon; a bell flashes it in the warning colour.
            TabMark.Loading -> Icon(YIcon.Spinner, c.muted, size = 14.dp, strokeWidth = 1.8f)
            TabMark.Bell -> PaneIcon(PaneGlyph.Bell, c.warning, size = 14.dp, strokeWidth = 1.8f)
            else -> PaneIcon(glyphOf(info.icon), if (info.mark == TabMark.Failed) c.failure else tint, size = 14.dp, strokeWidth = 1.8f)
        }
        Spacer(Modifier.width(6.dp))
        T(info.title, type.ui.copy(fontSize = 15.sp), tint, maxLines = 1, modifier = Modifier.widthIn(max = 150.dp))
        info.git?.let { Spacer(Modifier.width(5.dp)); T(it, type.monoSmall, gitColor(it, c)) }
        if (info.mark == TabMark.Exited) { Spacer(Modifier.width(5.dp)); T("exit ${info.exitCode}", type.monoSmall, if (info.exitCode == 0) c.muted else c.failure) }
        val mark = dot ?: if (info.mark == TabMark.Unseen) c.accent else null
        mark?.let { Spacer(Modifier.width(5.dp)); Box(Modifier.size(6.dp).clip(CircleShape).background(it)) }
        if (onClose != null) Box(Modifier.size(32.dp).clip(RoundedCornerShape(6.dp)).clickable(onClick = onClose).semantics { contentDescription = "Close ${info.title}" },
            contentAlignment = Alignment.Center) { PaneIcon(PaneGlyph.Close, c.muted, size = 12.dp) }
    }
}

fun gitColor(mark: String, c: YaacColors): Color = when (mark.firstOrNull()) {
    'A', 'U', '?' -> c.success
    'M', 'R' -> c.warning
    'D', '!' -> c.failure
    else -> c.muted
}

/** The active side tab's content (compact width replaces the chat with it), with the pane's notice. */
@Composable
fun SideContent(browser: JsonElement?, threadId: String, side: SidePaneState, send: (String, Any?) -> Unit, modifier: Modifier = Modifier) {
    val c = LocalYaac.current
    val active = browser["active"].scalarLong()
    val tab = browser["tabs"].objects.firstOrNull { it["id"].scalarLong() == active }
    Box(modifier.fillMaxSize().background(c.canvas)) {
        when (tab?.get("kind").str) {
            // (the terminal pads its key row; the lists scroll under the navigation bar)
            "files" -> FilesTree(browser["files"].obj(), threadId, active, send)
            "file", "diff" -> FilePane(browser["files"].obj(), tab!!, threadId, active, side, send)
            "terminal" -> TerminalPane(threadId, tab!!, side.terminals[active ?: -1], side, send)
            "browser" -> Column(Modifier.padding(20.dp)) {
                T("Browser", LocalType.current.heading)
                T(browser["reason"].text ?: tab?.get("error").text ?: "The page is shown on the desktop window.", LocalType.current.ui, c.muted, modifier = Modifier.padding(top = 6.dp))
                tab?.get("url").text?.let { url -> TextAction("Open in the browser") { send("browser.external", mapOf("thread" to threadId, "tab" to active)) }; T(url, LocalType.current.monoSmall, c.muted) }
            }
            else -> {}
        }
        side.notice?.let { n ->
            LaunchedEffect(n) { kotlinx.coroutines.delay(3000); if (side.notice == n) side.notice = null }
            T(n, LocalType.current.small, c.text, modifier = Modifier.align(Alignment.TopCenter).padding(top = 8.dp)
                .clip(RoundedCornerShape(6.dp)).background(c.raised).padding(horizontal = 12.dp, vertical = 6.dp))
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

/** Whether the chat is shown (no side tab active, or the chat chosen). */
fun showsChat(browser: JsonElement?, side: SidePaneState? = null): Boolean =
    browser["active"].isNullishPrimitive() || browser["chat"].flag() || browser["tabs"].objects.isEmpty() ||
        (side != null && side.chatChosen && side.chatChosenFor == browser["active"].scalarLong())

/** Chat chosen while [browser]'s active tab is open: shown until another tab becomes active. */
fun SidePaneState.chooseChat(browser: JsonElement?) { chatChosen = true; chatChosenFor = browser["active"].scalarLong() }

/** A multi-line paste into a terminal waits for a decision (`terminal.pasteDecision`, E07). */
@Composable
fun PasteConfirm(p: JsonObject, send: (String, Any?) -> Unit, onDone: () -> Unit) {
    val c = LocalYaac.current
    var remember by remember { mutableStateOf(false) }
    fun decide(allow: Boolean) {
        send("terminal.pasteDecision", mapOf("thread" to p["thread"].str, "tab" to p["tab"].scalarLong(), "panel" to p["panel"].flag(),
            "data" to mapOf("token" to p["token"].scalarLong(), "allow" to allow, "remember" to (allow && remember))))
        onDone()
    }
    androidx.compose.ui.window.Dialog(onDismissRequest = { decide(false) }) {
        Column(Modifier.clip(RoundedCornerShape(8.dp)).background(c.raised).padding(20.dp)) {
            T(TermPaste.question(p["lines"].int ?: 2), LocalType.current.title)
            T("The shell did not ask for bracketed paste, so each line runs as a command.", LocalType.current.ui, c.muted, modifier = Modifier.padding(top = 6.dp))
            Row(Modifier.padding(top = 10.dp).clickable { remember = !remember }.semantics { contentDescription = "Don't ask again, " + if (remember) "on" else "off" },
                verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(18.dp).clip(RoundedCornerShape(4.dp)).background(if (remember) c.accent else c.input), contentAlignment = Alignment.Center) {
                    if (remember) Icon(YIcon.Check, c.onAccent, size = 14.dp)
                }
                Spacer(Modifier.width(8.dp)); T("Don't ask again", LocalType.current.ui)
            }
            Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.End) {
                TextAction("Cancel") { decide(false) }
                PrimaryButton("Paste") { decide(true) }
            }
        }
    }
}
