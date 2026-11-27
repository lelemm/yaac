package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import dev.yaac.android.core.list
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.double
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/**
 * The composer (`ComposerView`): notices, running commands, the queue, the field, the button row
 * and the control row. Commands carry the composer's `key`, as WPF sends them.
 */
@Composable
fun Composer(view: JsonElement?, send: (String, Any?) -> Unit, history: JsonElement? = null) {
    val composer = view as? JsonObject ?: return
    val c = LocalYaac.current
    val type = LocalType.current
    val key = composer["key"].str
    val act: (ActionItem) -> Unit = { a -> if (a.enabled) send("composer.action", mapOf("key" to key, "id" to a.id)) }

    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp)) {
        for (n in composer["notices"].objects) {
            Row(Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                T(n["text"].str, type.small, if (n["error"].flag()) c.failure else c.text, modifier = Modifier.weight(1f))
                for (a in ActionItem.list(n["actions"])) TextAction(a.label, a.enabled, style = type.small) { act(a) }
            }
        }
        RunningCommands(composer["runningCommands"].obj(), type, act)
        Queue(composer, act)

        composer["status"].obj()?.let { st ->
            Row(Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                T(st["text"].str, type.small, modifier = Modifier.weight(1f))
                for (a in ActionItem.list(st["actions"])) TextAction(a.label, a.enabled, style = type.small) { act(a) }
            }
        }
        composer["ultrafastWarning"].text?.let { T(it, type.small, c["YaacUltrafast"], modifier = Modifier.padding(vertical = 4.dp)) }
        val attachments = composer["attachments"].objects
        if (attachments.isNotEmpty()) Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            for (row in attachments) AttachmentRow(row, act)
        }
        // A draft's Project and Worktree selectors (A16).
        val project = ActionItem.of(composer["project"])
        if (project != null && composer["showEditor"].flag(true)) {
            var open by remember { mutableStateOf<ActionItem?>(null) }
            Row(Modifier.fillMaxWidth().padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                for (a in listOfNotNull(project, ActionItem.of(composer["worktree"]))) {
                    Row(Modifier.clip(RoundedCornerShape(5.dp)).clickable(enabled = a.enabled) { if (a.children.isNotEmpty()) open = a else act(a) }.heightIn(min = 40.dp).padding(horizontal = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        T(a.label, type.ui.copy(fontSize = 15.sp), maxLines = 1)
                        if (a.children.isNotEmpty()) { Spacer(Modifier.width(3.dp)); Icon(YIcon.CaretDown, c.text, size = 11.dp) }
                    }
                }
            }
            open?.let { o -> ActionSheet(o.label, o.children, { open = null }) { act(it) } }
        }

        if (composer["showEditor"].flag(true)) {
            Editor(composer, key, send, history)
        }
        ControlRow(composer, act)
    }
}

@Composable
private fun RunningCommands(strip: JsonObject?, type: YaacType, act: (ActionItem) -> Unit) {
    strip ?: return
    val c = LocalYaac.current
    val expanded = strip["expanded"].flag()
    Row(Modifier.fillMaxWidth().clickable { act(ActionItem("toggleCommands", "", true, null, false, null, emptyList(), false)) }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(if (expanded) YIcon.ChevronDown else YIcon.ChevronRight, c.text, size = 16.dp)
        Spacer(Modifier.width(8.dp))
        T(strip["title"].str.trimStart('▾', '▸', ' '), type.ui.copy(fontSize = 15.sp), weight = FontWeight.SemiBold)
    }
    for (r in strip["rows"].objects) {
        Row(Modifier.fillMaxWidth().padding(start = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(YIcon.Play, c.text, size = 13.dp)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                T(r["label"].str, type.mono.copy(fontSize = 15.sp), maxLines = 1)
                r["detail"].text?.let { T(it, type.small, c.muted, maxLines = 1) }
            }
            for (a in ActionItem.list(r["actions"])) TextAction(a.label, a.enabled, style = type.ui) { act(a) }
        }
    }
    strip["log"].obj()?.let { log ->
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(c.input).padding(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                T(log["title"].str, type.mono, modifier = Modifier.weight(1f), maxLines = 1)
                ActionItem.of(log["close"])?.let { a -> TextAction(a.label, a.enabled, style = type.small) { act(a) } }
            }
            log["status"].text?.let { T(it, type.small, c.muted) }
            log["output"].text?.let { T(it.takeLast(4000), type.monoSmall, modifier = Modifier.heightIn(max = 220.dp)) }
        }
    }
}

@Composable
private fun Queue(composer: JsonObject, act: (ActionItem) -> Unit) {
    val queue = composer["queue"].objects
    if (queue.isEmpty()) return
    val c = LocalYaac.current
    val type = LocalType.current
    Row(Modifier.padding(top = 8.dp, bottom = 2.dp), verticalAlignment = Alignment.CenterVertically) {
        T("Queued · ${queue.size}", type.ui.copy(fontSize = 15.sp), weight = FontWeight.SemiBold)
        T(" · sent when the turn ends", type.ui, c.muted)
    }
    for (q in queue) {
        Row(Modifier.fillMaxWidth().heightIn(min = 40.dp).alpha(if (q["busy"].flag()) 0.6f else 1f), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                T(q["label"].str, type.ui.copy(fontSize = 15.sp), maxLines = 1)
                q["detail"].text?.let { T(it, type.small, c.muted, maxLines = 1) }
            }
            for (a in ActionItem.list(q["actions"])) {
                if (a.id.startsWith("queueCancel")) Box(Modifier.clip(RoundedCornerShape(6.dp)).clickable(enabled = a.enabled) { act(a) }.padding(10.dp)) { Icon(YIcon.Close, c.text, size = 16.dp) }
                else TextAction(a.label, a.enabled, style = type.ui) { act(a) }
            }
        }
    }
}

@Composable
private fun Editor(composer: JsonObject, key: String, send: (String, Any?) -> Unit, history: JsonElement?) {
    val c = LocalYaac.current
    val type = LocalType.current
    val remote = composer["text"].str
    val editable = composer["editable"].flag(true)
    // As WPF's UpdateText: the field is the source of truth while typing. An echo of one of our
    // own edits (or of the text we last adopted) is ignored; any other text from the controller
    // (sent, cleared, a queued message opened for editing) replaces the field.
    var value by remember(key) { mutableStateOf(TextFieldValue(remote, TextRange(remote.length))) }
    val state = remember(key) { EditEcho(remote) }
    LaunchedEffect(remote) {
        if (state.adopt(remote) && remote != value.text) {
            value = TextFieldValue(remote, TextRange(remote.length))
        }
    }
    // `insert` {id, text}: put text at the caret once (a skill, a file mention, a paste).
    val insert = composer["insert"].obj()
    LaunchedEffect(insert?.get("id").text) {
        val id = insert?.get("id").text ?: return@LaunchedEffect
        val text = insert["text"].text ?: return@LaunchedEffect
        if (!editable || id in state.inserted) return@LaunchedEffect
        state.inserted += id
        val sel = value.selection
        val next = value.text.replaceRange(sel.min, sel.max, text)
        value = TextFieldValue(next, TextRange(sel.min + text.length))
        state.edit(next)
        send("composer.edit", mapOf("key" to key, "text" to next, "caret" to sel.min + text.length))
    }
    val focusRequester = remember { androidx.compose.ui.focus.FocusRequester() }
    val focusToken = composer["focus"].let { if (it.flag()) "true" else it.text }
    LaunchedEffect(focusToken) { if (focusToken != null && editable) runCatching { focusRequester.requestFocus() } }
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val shape = RoundedCornerShape(8.dp)
    val setText: (String, Int) -> Unit = { next, caret ->
        value = TextFieldValue(next, TextRange(caret.coerceIn(0, next.length)))
        state.edit(next)
        send("composer.edit", mapOf("key" to key, "text" to next, "caret" to caret))
    }
    // `!` mode (P346): a draft that starts with `!` on a target that can run commands.
    val shell = composer["shellCapable"].flag() && value.text.startsWith("!")
    val shellView = composer["shell"].obj()
    SkillPicker(composer, key, value, send, setText)
    FilePicker(composer, key, value, focused, send, setText)
    if (shell) {
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            T(">_", type.mono, c["YaacShell"])
            Spacer(Modifier.width(8.dp))
            T(shellView?.get("prompt").str, type.mono, c.muted, maxLines = 1, modifier = Modifier.weight(1f))
            Box(Modifier.clip(RoundedCornerShape(6.dp)).clickable { setText(value.text.removePrefix("!"), 0) }.padding(8.dp)) { Icon(YIcon.Close, c.muted, size = 15.dp) }
        }
    }
    Box(
        Modifier.fillMaxWidth().padding(top = 8.dp).clip(shape).background(c.input)
            .then(if (focused) Modifier.border(2.dp, c.focus, shape) else Modifier)
            .heightIn(min = 52.dp).padding(horizontal = 14.dp, vertical = 14.dp),
    ) {
        if (value.text.isEmpty()) T(composer["placeholder"].str, type.prose, c.muted, maxLines = 1)
        BasicTextField(
            value, { v ->
                val changed = v.text != value.text
                value = v
                if (changed) { state.edit(v.text); send("composer.edit", mapOf("key" to key, "text" to v.text, "caret" to v.selection.end)) }
                else send("composer.caret", mapOf("key" to key, "caret" to v.selection.end))
            },
            enabled = editable, maxLines = 5, interactionSource = interaction,
            textStyle = (if (shell) type.mono.copy(fontSize = 15.sp) else type.prose).copy(color = c.text), cursorBrush = SolidColor(c.text),
            modifier = Modifier.fillMaxWidth().focusRequester(focusRequester),
        )
    }
    if (shell) shellView?.get("hint").text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(top = 4.dp, start = 4.dp)) }
    ButtonRow(composer, key, send, shell, value.text.isEmpty(), history) { text -> setText(text, text.length) }
}

/** Our edits not yet echoed by the controller, and the text it last confirmed. */
private class EditEcho(var server: String) {
    private val pending = ArrayList<String>()
    val inserted = HashSet<String>()
    fun edit(text: String) { pending += text; if (pending.size > 64) pending.removeAt(0) }
    /** Whether [remote] is new text to show (not an echo). */
    fun adopt(remote: String): Boolean {
        if (remote == server) return false
        val i = pending.indexOf(remote)
        server = remote
        if (i >= 0) { repeat(i + 1) { pending.removeAt(0) }; return false }
        pending.clear()
        return true
    }
}

/** An attachment row: thumbnail or mark, label, progress or detail, actions (Retry, ✕). */
@Composable
private fun AttachmentRow(row: JsonObject, act: (ActionItem) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val thumb = row["thumbnail"].obj()
    val bitmap = remember(thumb) { thumb?.let { decodeThumbnail(it) } }
    Row(Modifier.padding(top = 6.dp).clip(RoundedCornerShape(6.dp)).background(c.raised).padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
        if (bitmap != null) androidx.compose.foundation.Image(bitmap, null, Modifier.size(40.dp).clip(RoundedCornerShape(4.dp)))
        else Icon(if (row["image"].flag()) YIcon.Image else YIcon.File, c.muted, size = 20.dp, strokeWidth = 1.6f)
        Spacer(Modifier.width(8.dp))
        Column(Modifier.widthIn(max = 220.dp)) {
            T(row["label"].str, type.mono, maxLines = 1)
            val progress = row["progress"].double
            val detail = progress?.let { "${(it * 100).toInt()}%" } ?: row["detail"].text
            detail?.let { T(it, type.small, if (row["error"].flag()) c.failure else c.muted, maxLines = 2) }
        }
        for (a in ActionItem.list(row["actions"])) {
            if (a.label == "✕" || a.label.equals("Remove", true)) Box(Modifier.clip(RoundedCornerShape(6.dp)).clickable(enabled = a.enabled) { act(a) }.padding(8.dp)) { Icon(YIcon.Close, c.text, size = 15.dp) }
            else TextAction(a.label, a.enabled, style = type.small) { act(a) }
        }
    }
}

/** `{width, height, pixels}`: base64 premultiplied BGRA, at most 160 × 160 (decoded by the controller). */
private fun decodeThumbnail(t: JsonObject): androidx.compose.ui.graphics.ImageBitmap? {
    val w = t["width"].int ?: return null
    val h = t["height"].int ?: return null
    if (w !in 1..160 || h !in 1..160) return null
    val bytes = runCatching { android.util.Base64.decode(t["pixels"].str, android.util.Base64.DEFAULT) }.getOrNull() ?: return null
    if (bytes.size != w * h * 4) return null
    val px = IntArray(w * h) { i ->
        val o = i * 4
        val bl = bytes[o].toInt() and 255; val g = bytes[o + 1].toInt() and 255; val r = bytes[o + 2].toInt() and 255; val al = bytes[o + 3].toInt() and 255
        fun un(v: Int) = if (al == 0) 0 else minOf(255, v * 255 / al)
        (al shl 24) or (un(r) shl 16) or (un(g) shl 8) or un(bl)
    }
    return android.graphics.Bitmap.createBitmap(px, w, h, android.graphics.Bitmap.Config.ARGB_8888).asImageBitmap()
}

@Composable
private fun ButtonRow(composer: JsonObject, key: String, send: (String, Any?) -> Unit, shell: Boolean, empty: Boolean, history: JsonElement?, recall: (String) -> Unit) {
    val act: (ActionItem) -> Unit = { a -> if (a.enabled) send("composer.action", mapOf("key" to key, "id" to a.id)) }
    var historySheet by remember { mutableStateOf(false) }
    var attachMenu by remember { mutableStateOf(false) }
    val c = LocalYaac.current
    val type = LocalType.current
    val sendA = if (shell) ActionItem.of(composer["shell"]["run"]) ?: ActionItem.of(composer["send"]) else ActionItem.of(composer["send"])
    val now = if (shell) null else ActionItem.of(composer["sendNow"])
    val stop = ActionItem.of(composer["stop"])
    val attach = ActionItem.of(composer["attach"])
    val editable = composer["editable"].flag(true)
    Row(Modifier.fillMaxWidth().padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        // + attaches; touch and hold offers Paste, which goes through the controller (a large
        // paste then offers Attach as file, as on desktop).
        if (!shell) attach?.let { a ->
            Box(Modifier.size(44.dp).clip(RoundedCornerShape(6.dp)).tapAndHold({ if (a.enabled && editable) act(a) }, { attachMenu = true }), contentAlignment = Alignment.Center) {
                Icon(YIcon.Plus, c.text, size = 22.dp)
            }
        }
        Spacer(Modifier.width(4.dp))
        sendA?.let { a -> PrimaryButton(a.label, a.enabled && editable) { send("composer.send", mapOf("key" to key)) } }
        now?.takeIf { it.enabled }?.let { a -> TextAction(a.label, editable) { send("composer.sendNow", mapOf("key" to key)) } }
        stop?.takeIf { it.enabled }?.let { a -> TextAction(a.label, editable) { send("composer.stop", mapOf("key" to key)) } }
        ActionItem.of(composer["cancelEdit"])?.let { a -> TextAction(a.label, a.enabled && editable) { act(a) } }
        Spacer(Modifier.weight(1f))
        // Message history: the last messages sent from this client, offered in an empty composer.
        // Message history in an empty composer; in `!` mode the thread's own command history (E08).
        val past = if (shell) composer["shell"]["history"].list.mapNotNull { it.text } else history.list.mapNotNull { it.text }
        if ((empty || shell) && past.isNotEmpty()) IconButton(YIcon.History, if (shell) "Command history" else "Message history") { historySheet = true }
    }
    if (historySheet) {
        val list = if (shell) composer["shell"]["history"].list else history.list
        ActionSheet(if (shell) "Commands run here" else "Sent from here", list.mapIndexedNotNull { i, m -> m.text?.let { ActionItem("h$i", it.lineSequence().first().take(120), true, null, false, null, emptyList(), false) } }, { historySheet = false }) { a ->
            list.getOrNull(a.id.removePrefix("h").toInt())?.text?.let { recall(if (shell) "!$it" else it) }
        }
    }
    if (attachMenu) ActionSheet(null, listOfNotNull(attach, ActionItem.of(composer["paste"])), { attachMenu = false }) { act(it) }
    composer["hint"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(top = 2.dp, start = 4.dp)) }
}

/** The control row: a line of text selectors that scrolls sideways; each opens its sheet. */
@Composable
private fun ControlRow(composer: JsonObject, act: (ActionItem) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    var open by remember { mutableStateOf<ActionItem?>(null) }
    // The order WPF uses: model, effort, speed, mode, agents, then the rest, Fork last.
    val rank = mapOf("model" to 0, "effort" to 1, "fast" to 2, "speed" to 2, "mode" to 3, "agents" to 4)
    val controls = composer["controls"].objects.sortedBy { if (it["id"].str == "fork") 9 else rank[it["id"].str] ?: 5 }
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        controls.forEachIndexed { i, ctl ->
            val a = ActionItem.of(ctl) ?: return@forEachIndexed
            if (i > 0) T("·", type.ui, c.muted, modifier = Modifier.padding(horizontal = 4.dp))
            val warn = ctl["warn"].flag()
            Row(Modifier.clip(RoundedCornerShape(5.dp)).clickable(enabled = a.enabled) { if (a.children.isNotEmpty()) open = a else act(a) }
                .heightIn(min = 40.dp).padding(horizontal = 6.dp).alpha(if (a.enabled) 1f else 0.5f), verticalAlignment = Alignment.CenterVertically) {
                if (warn) { Icon(YIcon.Warning, c.failure, size = 15.dp); Spacer(Modifier.width(4.dp)) }
                T(a.label, type.ui.copy(fontSize = 15.sp), if (warn) c.failure else c.text, maxLines = 1)
                if (a.children.isNotEmpty()) { Spacer(Modifier.width(3.dp)); Icon(YIcon.CaretDown, if (warn) c.failure else c.text, size = 11.dp) }
            }
        }
        composer["contextPercent"].double?.let {
            T("·", type.ui, c.muted, modifier = Modifier.padding(horizontal = 4.dp))
            T("${it.toInt()}%", type.ui.copy(fontSize = 15.sp), modifier = Modifier.padding(horizontal = 6.dp))
        }
        composer["note"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(horizontal = 8.dp)) }
    }
    open?.let { ctl -> ActionSheet(ctl.label, ctl.children, { open = null }) { act(it) } }
}

/** The skill picker (`/` at the start, `$` inside): the controller's rows, inserted in place of [start, end). */
@Composable
private fun SkillPicker(composer: JsonObject, key: String, value: TextFieldValue, send: (String, Any?) -> Unit, setText: (String, Int) -> Unit) {
    val skills = composer["skills"].obj() ?: return
    val c = LocalYaac.current
    val type = LocalType.current
    val status = skills["status"].str
    Column(Modifier.fillMaxWidth().padding(top = 8.dp).clip(RoundedCornerShape(8.dp)).background(c.raised).heightIn(max = 260.dp).verticalScroll(rememberScrollState()).padding(vertical = 4.dp)) {
        Row(Modifier.fillMaxWidth().padding(start = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            T(when (status) { "loading" -> "Loading skills…"; "failed" -> skills["message"].text ?: "Skills could not be loaded"; else -> skills["message"].text ?: "" },
                type.small, if (status == "failed") c.failure else c.muted, modifier = Modifier.weight(1f))
            if (status == "failed") TextAction("Retry", style = type.small) { send("composer.skillRetry", mapOf("key" to key)) }
            Box(Modifier.clip(RoundedCornerShape(6.dp)).clickable { send("composer.skillDismiss", mapOf("key" to key)) }.padding(10.dp)) { Icon(YIcon.Close, c.muted, size = 14.dp) }
        }
        val start = skills["start"].int ?: 0
        val end = skills["end"].int ?: 0
        for (r in skills["rows"].objects) {
            Row(Modifier.fillMaxWidth().clickable {
                val text = value.text
                if (start in 0..end && end <= text.length) {
                    val insert = r["insert"].str
                    setText(text.replaceRange(start, end, insert), start + insert.length)
                }
            }.heightIn(min = 44.dp).padding(horizontal = 14.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Row { T(r["label"].str, type.mono); r["hint"].text?.let { T("  $it", type.mono, c.muted) } }
                    r["detail"].text?.let { T(it, type.small, c.muted, maxLines = 2) }
                }
                r["scope"].text?.let { T(it, type.small, c.muted) }
            }
        }
    }
}

/** The `@` file picker: the token at the caret is the query (`composer.fileQuery`). */
@OptIn(kotlinx.coroutines.FlowPreview::class)
@Composable
private fun FilePicker(composer: JsonObject, key: String, value: TextFieldValue, focused: Boolean, send: (String, Any?) -> Unit, setText: (String, Int) -> Unit) {
    if (composer["fileScope"].text == null) return
    val c = LocalYaac.current
    val type = LocalType.current
    val text = value.text
    val caret = value.selection.end
    var start = caret
    while (start > 0 && !text[start - 1].isWhitespace()) start--
    val token = if (focused && value.selection.collapsed && start < caret && text[start] == '@' && !(start + 1 < caret && text[start + 1] == '"')) start to text.substring(start + 1, caret) else null
    var dismissed by remember { mutableStateOf<Pair<Int, String>?>(null) }
    LaunchedEffect(token) {
        if (token == null || token == dismissed) return@LaunchedEffect
        kotlinx.coroutines.delay(200)
        send("composer.fileQuery", mapOf("key" to key, "query" to token.second))
    }
    if (token == null || token == dismissed) return
    val search = composer["filePicker"].obj()
    val matches = search?.get("query").text == token.second || (search?.get("query").str == "" && token.second == "")
    Column(Modifier.fillMaxWidth().padding(top = 8.dp).clip(RoundedCornerShape(8.dp)).background(c.raised).heightIn(max = 260.dp).verticalScroll(rememberScrollState()).padding(vertical = 4.dp)) {
        Row(Modifier.fillMaxWidth().padding(start = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            T(if (matches) search?.get("hint").text ?: "" else "Finding files…", type.small, c.muted, modifier = Modifier.weight(1f))
            Box(Modifier.clip(RoundedCornerShape(6.dp)).clickable { dismissed = token }.padding(10.dp)) { Icon(YIcon.Close, c.muted, size = 14.dp) }
        }
        if (matches) for (e in search?.get("entries").objects) {
            Column(Modifier.fillMaxWidth().clickable {
                var end = caret
                while (end < text.length && !text[end].isWhitespace()) end++
                val ref = e["reference"].str.let { if (end == text.length) "$it " else it }
                setText(text.replaceRange(token.first, end, ref), token.first + ref.length)
            }.heightIn(min = 44.dp).padding(horizontal = 14.dp, vertical = 4.dp)) {
                T(e["name"].str, type.mono)
                T(e["path"].str, type.small, c.muted, maxLines = 1)
            }
        }
    }
}
