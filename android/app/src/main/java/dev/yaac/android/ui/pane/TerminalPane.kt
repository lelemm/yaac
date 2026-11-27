package dev.yaac.android.ui.pane

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
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
import dev.yaac.android.ui.LocalType
import dev.yaac.android.ui.LocalYaac
import dev.yaac.android.ui.PrimaryButton
import dev.yaac.android.ui.SidePaneState
import dev.yaac.android.ui.T
import dev.yaac.android.ui.TextAction
import dev.yaac.android.ui.YaacColors
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull

private fun rgb(packed: Long) = Color(0xFF000000 or (packed and 0xFFFFFF))

/** One row of runs laid out by column, with the cursor cell drawn in the cursor's colour. */
private fun rowText(runs: List<TermRun>?, cursor: TermCursor?, c: YaacColors): AnnotatedString = buildAnnotatedString {
    var col = 0
    // Wide runs take two columns per glyph; record where each column starts in the text.
    val starts = ArrayList<Int>()
    for (r in runs.orEmpty()) {
        if (r.col > col) { repeat(r.col - col) { starts += length; append(' ') }; col = r.col }
        withStyle(SpanStyle(
            color = rgb(r.ink),
            background = if (r.paper >= 0) rgb(r.paper) else Color.Unspecified,
            fontWeight = if (r.bold) FontWeight.Bold else null,
            fontStyle = if (r.italic) FontStyle.Italic else null,
            textDecoration = when { r.underline && r.strike -> TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough)); r.underline -> TextDecoration.Underline; r.strike -> TextDecoration.LineThrough; else -> null },
        )) {
            var i = 0
            while (i < r.text.length) {
                val cp = r.text.codePointAt(i)
                val n = Character.charCount(cp)
                starts += length
                if (r.width == 2) starts += length
                append(r.text, i, i + n); i += n
            }
        }
        col += r.columns
    }
    if (cursor != null && cursor.visible) {
        while (starts.size <= cursor.col) { starts += length; append(' ') }
        val a = starts[cursor.col]
        val b = starts.getOrNull(cursor.col + if (cursor.wide) 2 else 1)?.takeIf { it > a } ?: (a + 1).coerceAtMost(length)
        when (cursor.shape) {
            "underline", "bar" -> addStyle(SpanStyle(textDecoration = TextDecoration.Underline, color = rgb(cursor.color)), a, b)
            "hollow" -> addStyle(SpanStyle(background = rgb(cursor.color).copy(alpha = .35f)), a, b)
            else -> addStyle(SpanStyle(background = rgb(cursor.color), color = c.canvas), a, b)
        }
    }
}

/**
 * The terminal (A24, E01–E07, K04): the controller's grid sized to the pane (`terminal.resize`);
 * a toolbar with the shell and folder, Search, Clear, Paste, Add to composer and Restart; the
 * exit status; swipes scroll back (wheel), touch and hold then drag selects; the keyboard goes
 * to the shell, hardware keys too, except bindings marked Works in the terminal.
 */
@Composable
fun TerminalPane(threadId: String, tab: JsonObject, screen: TermScreen?, side: SidePaneState, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val tabId = (tab["id"] as? JsonPrimitive)?.content?.toLongOrNull()
    val info = Tabs.info(tab)
    val style = type.mono.copy(fontSize = 12.sp, lineHeight = 15.sp)
    val measurer = rememberTextMeasurer()
    val cell = remember(style) { measurer.measure("MMMMMMMMMM", style).size.let { it.width / 10f to it.height.toFloat() } }
    var ctrl by remember { mutableStateOf(false) }
    var searching by remember(tabId) { mutableStateOf(false) }
    var query by remember(tabId) { mutableStateOf("") }
    val clipboard = LocalClipboardManager.current
    val focus = remember { FocusRequester() }
    fun input(name: String, data: Map<String, Any?>, extra: Map<String, Any?> = emptyMap()) =
        send("terminal.$name", mapOf("thread" to threadId, "tab" to tabId, "panel" to false, "data" to data) + extra)
    fun paste() {
        val (clip, failed) = try { clipboard.getText()?.text to false } catch (_: SecurityException) { null to true }
        when (val o = TermPaste.outcome(clip, failed)) {
            is PasteOutcome.Send -> input("paste", mapOf("text" to o.text))
            is PasteOutcome.Notice -> side.notice = o.text
        }
    }
    // The shell's paste key (Ctrl+V, Ctrl+Shift+V) asks for the clipboard.
    LaunchedEffect(side.readPaste) { if (side.readPaste != null && side.readPaste == tabId) { side.readPaste = null; paste() } }
    // The controller's search echo (a "Search for this" selection) opens the bar.
    LaunchedEffect(screen?.search) { screen?.search?.let { if (!searching) { searching = true; query = it } } }
    val sentinel = "​"
    var field by remember { mutableStateOf(TextFieldValue(sentinel, TextRange(1))) }
    var cols by remember { mutableStateOf(80) }
    var rows by remember { mutableStateOf(24) }
    val shortcuts by rememberUpdatedState(side.shortcuts)

    Column(Modifier.fillMaxSize().background(c.input)) {
        // Toolbar: shell · folder, then the actions.
        Row(Modifier.fillMaxWidth().background(c.canvas).padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            T(listOfNotNull(tab["shell"].let { (it as? JsonPrimitive)?.content }, info.detail).joinToString(" · "), type.small, c.muted, maxLines = 1, modifier = Modifier.weight(1f))
            ToolButton(PaneGlyph.Search, "Search") { searching = true }
            ToolButton(PaneGlyph.Eraser, "Clear terminal") { input("clear", emptyMap()) }
            ToolButton(PaneGlyph.Clipboard, "Paste") { paste() }
            ToolButton(PaneGlyph.AddToComposer, "Add selection to composer", enabled = screen?.selection == true) { input("add", emptyMap(), mapOf("caret" to side.caret())) }
            if (Tabs.canRestart(info)) ToolButton(PaneGlyph.Reload, "Restart shell") { input("restart", emptyMap()) }
            // Send commands to the agent (P345): shown only when the controller describes it.
            if (tab.containsKey("sendCommands")) {
                val on = (tab["sendCommands"] as? JsonPrimitive)?.booleanOrNull ?: false
                val reason = (tab["sendCommandsReason"] as? JsonPrimitive)?.takeIf { it.isString }?.content
                Row(Modifier.clickable(enabled = reason == null) { input("sendCommands", mapOf("on" to !on)) }
                    .semantics { contentDescription = "Send commands to the agent, " + (if (on) "on" else "off") + (reason?.let { ", unavailable: $it" } ?: "") }
                    .padding(horizontal = 8.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(width = 28.dp, height = 16.dp).clip(RoundedCornerShape(8.dp)).background(if (on) c.accent else c.rule).alpha(if (reason == null) 1f else .45f),
                        contentAlignment = if (on) Alignment.CenterEnd else Alignment.CenterStart) {
                        Box(Modifier.padding(2.dp).size(12.dp).clip(RoundedCornerShape(6.dp)).background(if (on) c.onAccent else c.muted))
                    }
                }
            }
        }
        if (tab.containsKey("sendCommands")) (tab["sendCommandsReason"] as? JsonPrimitive)?.takeIf { it.isString }?.content?.let {
            T(it, type.small, c.muted, modifier = Modifier.fillMaxWidth().background(c.canvas).padding(horizontal = 12.dp, vertical = 2.dp))
        }
        if (searching) Row(Modifier.fillMaxWidth().background(c.panel).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            val searchFocus = remember { FocusRequester() }
            LaunchedEffect(Unit) { runCatching { searchFocus.requestFocus() } }
            BasicTextField(query, { q -> query = q; input("search", mapOf("query" to q)) }, singleLine = true,
                textStyle = type.mono.copy(color = c.text), cursorBrush = SolidColor(c.text),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { input("next", mapOf("backward" to true)) }),
                modifier = Modifier.weight(1f).focusRequester(searchFocus).clip(RoundedCornerShape(6.dp)).background(c.input).padding(10.dp)
                    .semantics { contentDescription = "Search the terminal" })
            T(TerminalWire.searchStatus(screen?.search, screen?.found ?: false, screen?.matches ?: 0), type.small, c.muted, modifier = Modifier.padding(horizontal = 6.dp))
            // Older is up, as WPF's Enter; newer is down.
            ToolButton(PaneGlyph.Up, "Older match") { input("next", mapOf("backward" to true)) }
            ToolButton(PaneGlyph.Down, "Newer match") { input("next", mapOf("backward" to false)) }
            ToolButton(PaneGlyph.Close, "Close search") { query = ""; input("search", mapOf("query" to "")); searching = false; runCatching { focus.requestFocus() } }
        }
        if (screen?.selection == true) Row(Modifier.fillMaxWidth().background(c.panel).horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            TextAction("Copy", style = type.small) { input("copy", emptyMap()) }
            TextAction("Add to composer", style = type.small) { input("add", emptyMap(), mapOf("caret" to side.caret())) }
            TextAction("Search for this", style = type.small) { input("searchSelection", emptyMap()) }
        }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth().padding(6.dp)) {
            val density = LocalDensity.current
            val (fitCols, fitRows) = TermGestures.fit(with(density) { maxWidth.toPx() }, with(density) { maxHeight.toPx() }, cell.first, cell.second)
            LaunchedEffect(tabId, fitCols, fitRows) {
                cols = fitCols; rows = fitRows
                input("resize", mapOf("cols" to fitCols, "rows" to fitRows, "cellWidth" to cell.first.toInt(), "cellHeight" to cell.second.toInt(), "dark" to c.dark))
                runCatching { focus.requestFocus() }
            }
            fun mouse(action: String, x: Float, y: Float, button: String, clicks: Int) {
                val at = TermGestures.cellAt(x, y, cell.first, cell.second, cols, rows)
                input("mouse", mapOf("action" to action, "button" to button, "clicks" to clicks, "col" to at.col, "row" to at.row, "right" to at.right, "mods" to 0))
            }
            val wheel = remember(cell) { WheelAccumulator(cell.second) }
            val grid = Modifier.fillMaxSize()
                // A tap focuses the keyboard and clears a selection.
                .pointerInput(tabId, screen?.selection) {
                    detectTapGestures { o ->
                        if (screen?.selection == true) { mouse("down", o.x, o.y, "Left", 1); mouse("up", o.x, o.y, "Left", 0) }
                        runCatching { focus.requestFocus() }
                    }
                }
                // Swipes scroll back: the wheel, as the controller expects it (E05).
                .pointerInput(tabId, cols, rows) {
                    detectVerticalDragGestures(onDragEnd = { wheel.reset() }, onDragCancel = { wheel.reset() }) { change, dy ->
                        change.consume()
                        val n = wheel.drag(dy)
                        repeat(kotlin.math.abs(n)) { mouse("wheel", change.position.x, change.position.y, if (n > 0) "Up" else "Down", 0) }
                    }
                }
                // Touch and hold selects a word; dragging extends it (E07 copy, E03 add).
                .pointerInput(tabId, cols, rows) {
                    var last = androidx.compose.ui.geometry.Offset.Zero
                    detectDragGesturesAfterLongPress(
                        onDragStart = { o -> last = o; mouse("down", o.x, o.y, "Left", 2) },
                        onDragEnd = { mouse("up", last.x, last.y, "Left", 0) },
                        onDragCancel = { mouse("up", last.x, last.y, "Left", 0) },
                    ) { change, _ -> change.consume(); last = change.position; mouse("move", last.x, last.y, "Left", 0) }
                }
                .semantics { contentDescription = "Terminal, ${info.title}" }
            Box(grid) {
                if (screen == null) T("Starting the terminal…", type.small, c.muted)
                else Column {
                    for (r in 0 until screen.height) {
                        BasicText(rowText(screen.rows[r], screen.cursor?.takeIf { it.row == r }, c), style = style.copy(color = c.text), softWrap = false, maxLines = 1)
                    }
                }
            }
            // The keyboard: an invisible field keeps a one-character sentinel, so a deletion is
            // seen as Backspace and anything typed is sent as text. Hardware keys are routed first.
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
            }, modifier = Modifier.size(1.dp).alpha(0f).focusRequester(focus)
                .onFocusChanged { input("focus", mapOf("focused" to it.isFocused)) }
                .onPreviewKeyEvent { e ->
                    val n = e.nativeKeyEvent
                    if (e.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                    val base = n.getUnicodeChar(0).takeIf { it > 0 }?.toChar()
                    when (val r = TermKeys.route(n.keyCode, base, n.isShiftPressed, n.isAltPressed, n.isCtrlPressed, n.isMetaPressed, n.repeatCount > 0, shortcuts)) {
                        is KeyRoute.Command -> { send("shell.shortcut", mapOf("id" to r.id, "inText" to true, "terminalFocus" to true)); true }
                        is KeyRoute.Key -> { input("key", mapOf("key" to r.name, "mods" to r.mods)); true }
                        KeyRoute.Consume -> true
                        KeyRoute.Text -> false
                    }
                }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Send, autoCorrectEnabled = false),
                keyboardActions = KeyboardActions(onSend = { input("key", mapOf("key" to "Enter", "mods" to 0)) }))
        }
        // The exit status (E01): the code, or why the shell failed, with Restart.
        Tabs.terminalStatus(info)?.let { status ->
            Row(Modifier.fillMaxWidth().background(c.panel).padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                T(status, type.ui, if (info.mark == TabMark.Failed || (info.exitCode ?: 0) != 0) c.failure else c.text, modifier = Modifier.weight(1f).padding(vertical = 8.dp))
                if (Tabs.canRestart(info)) PrimaryButton("Restart", modifier = Modifier.padding(6.dp)) { input("restart", emptyMap()) }
            }
        }
        // The keys a touch keyboard lacks.
        Row(Modifier.fillMaxWidth().background(c.panel).navigationBarsPadding().horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            TextAction("Ctrl", color = if (ctrl) c.warning else c.text, style = type.mono) { ctrl = !ctrl; runCatching { focus.requestFocus() } }
            for ((label, key) in listOf("Esc" to "Escape", "Tab" to "Tab", "↑" to "Up", "↓" to "Down", "←" to "Left", "→" to "Right", "PgUp" to "PageUp", "PgDn" to "PageDown", "Home" to "Home", "End" to "End", "⏎" to "Enter"))
                TextAction(label, style = type.mono) { input("key", mapOf("key" to key, "mods" to 0)) }
        }
    }
}
