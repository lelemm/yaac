package dev.yaac.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.focusable
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.obj
import dev.yaac.android.core.objects
import dev.yaac.android.core.scalar
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** The pages, in the controller's order (`Page::ALL`); Notifications matters most on a phone. */
private val Pages = listOf("targets", "providers", "skills", "approvals", "appearance", "window", "android", "keyboard", "terminal", "notifications", "advanced", "about")
private fun title(id: String) = id.replaceFirstChar { it.uppercase() }

/**
 * Settings (A27/A28), drawn from the `settings` view as `SettingsView` draws it. At compact width
 * the page list is its own screen; a page has Back and its name. Commands: `settings.page{id}`,
 * `settings.action{id, values?}`, `settings.close`.
 */
@Composable
fun SettingsScreen(view: JsonElement?, send: (String, Any?) -> Unit, onChangeHub: (() -> Unit)? = null) {
    val c = LocalYaac.current
    val type = LocalType.current
    val mode = view["mode"].str
    val page = view["page"].str.lowercase().takeIf { it in Pages } ?: "targets"
    var onPage by rememberSaveable { mutableStateOf(false) }
    val close = { send("settings.close", null) }
    val settings = mode != "firstRun" && mode != "updateNeeded"
    BackHandler { if (settings && onPage) onPage = false else if (view["closeAllowed"].flag(true)) close() }
    Column(Modifier.fillMaxSize().background(c.canvas).statusBarsPadding().imePadding()) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            if ((settings && view["closeAllowed"].flag(true)) || onPage) IconButton(YIcon.Back, "Back") { if (settings && onPage) onPage = false else close() }
            Spacer(Modifier.width(6.dp))
            T(when { !settings -> if (mode == "firstRun") "YAAC" else "Update YAAC"; onPage -> title(page); else -> "Settings" }, type.title.copy(fontSize = 19.sp))
        }
        if (settings && !onPage) {
            Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())) {
                for (p in Pages) {
                    Row(Modifier.fillMaxWidth().clickable { send("settings.page", mapOf("id" to p)); onPage = true }.heightIn(min = 52.dp).padding(horizontal = 20.dp),
                        verticalAlignment = Alignment.CenterVertically) {
                        T(title(p), type.ui.copy(fontSize = 16.sp), modifier = Modifier.weight(1f))
                        Icon(YIcon.ChevronRight, c.muted, size = 18.dp)
                    }
                }
                // Which hubs this phone attaches to (the window's attach target), P404: Android only.
                if (onChangeHub != null) {
                    Spacer(Modifier.height(12.dp))
                    Row(Modifier.fillMaxWidth().clickable(onClick = onChangeHub).heightIn(min = 52.dp).padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            T("Hubs…", type.ui.copy(fontSize = 16.sp))
                            val rt = dev.yaac.android.core.YaacRuntime
                            rt.selected?.let { sel ->
                                val n = rt.running.size
                                T(when {
                                    sel.hub == null -> "Running the demo"
                                    n > 1 -> "$n hubs · showing ${sel.name}"
                                    else -> "Connected to ${sel.hub.address}"
                                }, type.small, c.muted)
                            }
                        }
                    }
                    // This app's build against the hub's, and downloading another (ui/Updates.kt).
                    UpdatesRow()
                    VoiceLanguageRow()
                    LeftOffRow()
                }
            }
        } else {
            SettingsPage(view, send)
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SettingsPage(view: JsonElement?, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    // Drafts of text fields, by "section/row/field", included when a submit action runs.
    val drafts = remember(view["page"].str) { mutableStateMapOf<String, String>() }
    if (view["mode"].str != "firstRun") {
        // Pages can be long (Keyboard has a row per binding): compose only what is on screen.
        androidx.compose.foundation.lazy.LazyColumn(Modifier.fillMaxWidth().navigationBarsPadding(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp)) {
            view["detail"].text?.let { d -> item("detail") { T(d, type.ui.copy(fontSize = 15.sp), c.muted, modifier = Modifier.padding(top = 4.dp)) } }
            val top = view["actions"].objects
            if (top.isNotEmpty()) item("actions") { FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) { for (a in top) SettingsAction(a, null, send) } }
            for (section in view["sections"].objects) {
                val sid = section["id"].str
                item("s:$sid") {
                    Column(Modifier.padding(top = 18.dp)) {
                        section["title"].text?.let { T(it, type.heading, c.tone(section["tone"].text, c.text)) }
                        section["detail"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(top = 2.dp)) }
                    }
                }
                val cards = section["cards"].objects
                if (cards.isNotEmpty()) item("c:$sid") { ThemeCards(cards, send) }
                for (row in section["rows"].objects) item("r:$sid/${row["id"].str}") { SettingsRow(row, "$sid/${row["id"].str}/", drafts, send) }
                val sectionActions = section["actions"].objects
                if (sectionActions.isNotEmpty()) item("a:$sid") {
                    FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) { for (a in sectionActions) SettingsAction(a, null, send) }
                }
            }
            view["footer"].text?.let { f -> item("footer") { T(f, type.small, c.muted, modifier = Modifier.padding(top = 18.dp)) } }
        }
        return
    }
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(horizontal = 16.dp).padding(bottom = 24.dp)) {
        val firstRun = view["mode"].str == "firstRun"
        view["detail"].text?.let { T(it, type.ui.copy(fontSize = 15.sp), c.muted, modifier = Modifier.padding(top = 4.dp)) }
        val top = view["actions"].objects
        if (top.isNotEmpty()) FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) { for (a in top) SettingsAction(a, null, send) }
        for (section in view["sections"].objects) {
            Spacer(Modifier.height(if (firstRun) 12.dp else 18.dp))
            // First run stacks its sections as cards (A15); Settings keeps them on the canvas.
            Column(if (firstRun) Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.raised).padding(16.dp) else Modifier.fillMaxWidth()) {
            section["title"].text?.let { T(it, type.heading, c.tone(section["tone"].text, c.text)) }
            section["detail"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(top = 2.dp)) }
            for (row in section["rows"].objects) SettingsRow(row, section["id"].str + "/" + row["id"].str + "/", drafts, send)
            val sectionActions = section["actions"].objects
            if (sectionActions.isNotEmpty()) FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                for (a in sectionActions) SettingsAction(a, null, send)
            }
            }
        }
        view["footer"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(top = 18.dp)) }
    }
}


/** One row: label and status, a lone toggle at the right, detail, fields, then actions. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun SettingsRow(row: JsonObject, prefix: String, drafts: MutableMap<String, String>, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    Column(Modifier.fillMaxWidth().padding(top = 10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val lone = row["actions"].objects.singleOrNull()?.takeIf { it["kind"].text == "toggle" }
            Column(Modifier.weight(1f)) {
                // A row with only a toggle is labelled by the toggle (as WPF's check box).
                (row["label"].text ?: lone?.get("label").text)?.let { T(it, type.ui.copy(fontSize = 15.sp), if (row["dimmed"].flag()) c.muted else c.text) }
                row["status"].text?.let { T(it, type.small, c.tone(row["tone"].text, c.muted)) }
            }
            // A single toggle sits at the row's right, like a switch.
            val actions = row["actions"].objects
            actions.singleOrNull()?.takeIf { it["kind"].text == "toggle" }?.let { SettingsAction(it, null, send) }
        }
        row["detail"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(top = 2.dp)) }
        for (f in row["fields"].objects) SettingsField(f, prefix, drafts, send)
        val actions = row["actions"].objects.filterNot { row["actions"].objects.size == 1 && it["kind"].text == "toggle" }
        val fieldValues = row["fields"].objects.associate { it["id"].str to (drafts[prefix + it["id"].str] ?: it["value"].scalar ?: "") }
        // Each toggle gets its own line with its label; the other actions share a row.
        for (t in actions.filter { it["kind"].text == "toggle" }) {
            Row(Modifier.fillMaxWidth().padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                T(t["label"].str, type.ui.copy(fontSize = 15.sp), modifier = Modifier.weight(1f))
                SettingsAction(t, fieldValues, send)
            }
        }
        val rest = actions.filterNot { it["kind"].text == "toggle" }
        if (rest.isNotEmpty()) FlowRow(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            for (a in rest) SettingsAction(a, fieldValues) { command, payload ->
                if (a["submit"].flag() || a["cancel"].flag()) {
                    for (field in row["fields"].objects.filter { it["type"].text == "password" }) {
                        drafts.remove(prefix + field["id"].str)
                    }
                }
                send(command, payload)
            }
        }
        // A disabled action's reason, under the actions rather than among them.
        for (a in actions.filter { !it["enabled"].flag(true) }) a["detail"].text?.let { T("${a["label"].str}: $it", type.small, c.muted) }
    }
            }

/** An action: `toggle` (switch; sends the new `checked`), `primary`, `copy`, a menu, or text. */
@Composable
private fun SettingsAction(a: JsonObject, fields: Map<String, String>?, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val clipboard = LocalClipboardManager.current
    val enabled = a["enabled"].flag(true)
    val id = a["id"].str
    var menu by remember { mutableStateOf(false) }
    // An action that asks first (`confirm`): the dialog sends it, Cancel does nothing.
    val confirm = dev.yaac.android.core.ActionConfirm.parse(a)
    var confirming by remember { mutableStateOf(false) }
    fun values(extra: Map<String, Any?> = emptyMap()): Map<String, Any?> {
        val v = LinkedHashMap<String, Any?>()
        a["values"].obj?.forEach { (k, x) -> v[k] = (x as? JsonPrimitive)?.let { p -> if (p.isString) p.content else p.content.toBooleanStrictOrNull() ?: p.content.toLongOrNull() ?: p.content } }
        if (a["submit"].flag() && fields != null) v.putAll(fields)
        v.putAll(extra)
        return v
    }
    fun dispatch(extra: Map<String, Any?>) {
        val v = values(extra)
        send("settings.action", if (v.isEmpty()) mapOf("id" to id) else mapOf("id" to id, "values" to v))
    }
    fun run(extra: Map<String, Any?> = emptyMap()) {
        if (confirm != null) confirming = true else dispatch(extra)
    }
    if (confirming && confirm != null) {
        val type = LocalType.current
        androidx.compose.ui.window.Dialog(onDismissRequest = { confirming = false }) {
            Column(Modifier.clip(RoundedCornerShape(8.dp)).background(c.raised).padding(20.dp)) {
                T(confirm.title, type.title)
                if (confirm.text.isNotEmpty()) T(confirm.text, type.ui, c.muted, modifier = Modifier.padding(top = 10.dp))
                Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.End) {
                    TextAction("Cancel") { confirming = false }
                    PrimaryButton(confirm.ok) { confirming = false; dispatch(emptyMap()) }
                }
            }
        }
    }
    when {
        a["kind"].text == "toggle" -> {
            val on = a["checked"].flag()
            Row(Modifier.clip(RoundedCornerShape(6.dp)).clickable(enabled = enabled) { run(mapOf("checked" to !on)) }.padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Switch(on, enabled)
            }
        }
        a["children"].objects.isNotEmpty() -> {
            TextAction(a["label"].str, enabled) { menu = true }
            if (menu) ActionSheet(a["label"].str, ActionItem.list(a["children"]), { menu = false }) { child -> send("settings.action", mapOf("id" to child.id)) }
        }
        a["kind"].text == "copy" -> TextAction(a["label"].str, enabled) { a["text"].text?.let { clipboard.setText(AnnotatedString(it)) } ?: run() }
        a["kind"].text == "primary" || a["default"].flag() -> PrimaryButton(a["label"].str, enabled) { run() }
        else -> TextAction(a["label"].str, enabled, color = if (a["kind"].text == "danger") c.failure else c.text) { run() }
    }
}

/** A neutral switch: the track in primary text when on (no accent colour). */
@Composable
private fun Switch(on: Boolean, enabled: Boolean) {
    val c = LocalYaac.current
    val track = if (on) c.accent else c.rule
    Box(Modifier.width(40.dp).height(22.dp).clip(RoundedCornerShape(11.dp)).background(track.copy(alpha = if (enabled) 1f else 0.45f)).padding(3.dp),
        contentAlignment = if (on) Alignment.CenterEnd else Alignment.CenterStart) {
        Box(Modifier.size(16.dp).clip(RoundedCornerShape(8.dp)).background(if (on) c.onAccent else c.muted))
    }
}

@Composable
private fun SettingsField(f: JsonObject, prefix: String, drafts: MutableMap<String, String>, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val id = f["id"].str
    val action = f["action"].text
    val value = f["value"].scalar ?: ""
    var sheet by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(top = 6.dp)) {
        f["label"].text?.let { T(it, type.small, c.muted) }
        when (f["type"].text ?: "text") {
            "radio" -> for (o in f["options"].objects) {
                val oid = o["id"].str
                Row(Modifier.fillMaxWidth().clickable(enabled = o["enabled"].flag(true) && action != null) { send("settings.action", mapOf("id" to action, "values" to mapOf(id to oid))) }
                    .heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(20.dp).clip(RoundedCornerShape(10.dp)).background(if (oid == value) c.accent else c.rule), contentAlignment = Alignment.Center) {
                        if (oid == value) Box(Modifier.size(8.dp).clip(RoundedCornerShape(4.dp)).background(c.onAccent))
                    }
                    Spacer(Modifier.width(12.dp)); T(o["label"].str, type.ui.copy(fontSize = 15.sp))
                }
            }
            "choice", "menu" -> {
                val options = f["options"].objects
                val current = options.firstOrNull { it["id"].str == value }?.get("label").text ?: value
                Row(Modifier.clip(RoundedCornerShape(6.dp)).background(c.input).clickable { sheet = true }.heightIn(min = 44.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    T(current, type.ui.copy(fontSize = 15.sp)); Spacer(Modifier.width(6.dp)); Icon(YIcon.CaretDown, c.text, size = 11.dp)
                }
                if (sheet) ActionSheet(f["label"].text, options.map { ActionItem(it["id"].str, it["label"].str, it["enabled"].flag(true), it["detail"].text, false, it["id"].str == value, emptyList(), false) }, { sheet = false }) { o ->
                    if (action != null) send("settings.action", mapOf("id" to action, "values" to mapOf(id to o.id))) else drafts[prefix + id] = o.id
                }
            }
            // A key binding is recorded by pressing it, as WPF does: tap the key cap, press the
            // chord. Esc cancels; Backspace or Delete clears. Without a hardware keyboard there is
            // nothing to press, so the binding is only shown.
            "progress" -> {
                // `value` is a percentage, or null while it is unknown.
                val pct = (f["value"] as? JsonPrimitive)?.content?.toDoubleOrNull()
                Box(Modifier.padding(top = 6.dp).fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)).background(c.rule)) {
                    if (pct != null) Box(Modifier.fillMaxWidth((pct / 100).toFloat().coerceIn(0f, 1f)).height(6.dp).background(c.accent))
                }
            }
            "shortcut" -> ShortcutField(value, f["placeholder"].text, action) { values -> if (action != null) send("settings.action", mapOf("id" to action, "values" to values)) }
            else -> {
                val secret = f["type"].text == "password"
                BasicTextField(
                    drafts[prefix + id] ?: value, { drafts[prefix + id] = it },
                    singleLine = f["type"].text != "textarea", textStyle = (if (f["type"].text == "code") type.mono else type.ui.copy(fontSize = 15.sp)).copy(color = c.text),
                    cursorBrush = SolidColor(c.text), visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(c.input).padding(12.dp).heightIn(min = if (f["type"].text == "textarea") 80.dp else 0.dp),
                )
                f["placeholder"].text?.takeIf { (drafts[prefix + id] ?: value).isEmpty() }?.let { T(it, type.small, c.muted) }
            }
        }
        (f["error"].text ?: f["validation"].text)?.let { T(it, type.small, c.failure) }
    }
}

@Composable
private fun ShortcutField(value: String, placeholder: String?, action: String?, record: (Map<String, Any?>) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val hardKeys = androidx.compose.ui.platform.LocalConfiguration.current.keyboard == android.content.res.Configuration.KEYBOARD_QWERTY
    var capturing by remember { mutableStateOf(false) }
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    androidx.compose.runtime.LaunchedEffect(capturing) { if (capturing) runCatching { focus.requestFocus() } }
    val label = when {
        capturing -> "Press a key…  Esc cancels"
        value.isNotEmpty() -> value
        else -> placeholder ?: "Add a key"
    }
    Box(
        Modifier.clip(RoundedCornerShape(6.dp)).background(if (capturing) c.selected else c.input)
            .clickable(enabled = hardKeys && action != null) { capturing = !capturing }
            .focusRequester(focus).focusable()
            .onPreviewKeyEvent { e ->
                if (!capturing) return@onPreviewKeyEvent false
                if (e.type != androidx.compose.ui.input.key.KeyEventType.KeyDown) return@onPreviewKeyEvent true
                val native = e.nativeKeyEvent
                if (native.repeatCount > 0) return@onPreviewKeyEvent true
                val plain = !native.isCtrlPressed && !native.isAltPressed && !native.isShiftPressed && !native.isMetaPressed
                when {
                    native.keyCode == android.view.KeyEvent.KEYCODE_ESCAPE && plain -> capturing = false
                    (native.keyCode == android.view.KeyEvent.KEYCODE_DEL || native.keyCode == android.view.KeyEvent.KEYCODE_FORWARD_DEL) && plain -> {
                        capturing = false; record(chord("", native))
                    }
                    else -> canonicalKey(native)?.let { capturing = false; record(chord(it, native)) }
                }
                true
            }
            .padding(12.dp),
    ) { T(label, type.mono, if (capturing) c.text else if (value.isEmpty()) c.muted else c.text) }
}

private fun chord(key: String, e: android.view.KeyEvent) = mapOf(
    "key" to key, "ctrl" to e.isCtrlPressed, "alt" to e.isAltPressed, "shift" to e.isShiftPressed, "command" to e.isMetaPressed,
)

/** The key names WPF's `CanonicalKey` produces, from Android key codes. Modifiers alone are not keys. */
private fun canonicalKey(e: android.view.KeyEvent): String? {
    val k = e.keyCode
    return when (k) {
        in android.view.KeyEvent.KEYCODE_A..android.view.KeyEvent.KEYCODE_Z -> ('A' + (k - android.view.KeyEvent.KEYCODE_A)).toString()
        in android.view.KeyEvent.KEYCODE_0..android.view.KeyEvent.KEYCODE_9 -> (k - android.view.KeyEvent.KEYCODE_0).toString()
        in android.view.KeyEvent.KEYCODE_NUMPAD_0..android.view.KeyEvent.KEYCODE_NUMPAD_9 -> (k - android.view.KeyEvent.KEYCODE_NUMPAD_0).toString()
        in android.view.KeyEvent.KEYCODE_F1..android.view.KeyEvent.KEYCODE_F12 -> "F" + (k - android.view.KeyEvent.KEYCODE_F1 + 1)
        android.view.KeyEvent.KEYCODE_ESCAPE -> "Escape"
        android.view.KeyEvent.KEYCODE_DEL -> "Backspace"
        android.view.KeyEvent.KEYCODE_ENTER, android.view.KeyEvent.KEYCODE_NUMPAD_ENTER -> "Enter"
        android.view.KeyEvent.KEYCODE_TAB -> "Tab"
        android.view.KeyEvent.KEYCODE_SPACE -> "Space"
        android.view.KeyEvent.KEYCODE_INSERT -> "Insert"
        android.view.KeyEvent.KEYCODE_FORWARD_DEL -> "Delete"
        android.view.KeyEvent.KEYCODE_MOVE_HOME -> "Home"
        android.view.KeyEvent.KEYCODE_MOVE_END -> "End"
        android.view.KeyEvent.KEYCODE_PAGE_UP -> "PageUp"
        android.view.KeyEvent.KEYCODE_PAGE_DOWN -> "PageDown"
        android.view.KeyEvent.KEYCODE_DPAD_LEFT -> "Left"
        android.view.KeyEvent.KEYCODE_DPAD_RIGHT -> "Right"
        android.view.KeyEvent.KEYCODE_DPAD_UP -> "Up"
        android.view.KeyEvent.KEYCODE_DPAD_DOWN -> "Down"
        android.view.KeyEvent.KEYCODE_EQUALS -> if (e.isShiftPressed) "Plus" else "Equals"
        android.view.KeyEvent.KEYCODE_PLUS, android.view.KeyEvent.KEYCODE_NUMPAD_ADD -> "Plus"
        android.view.KeyEvent.KEYCODE_MINUS, android.view.KeyEvent.KEYCODE_NUMPAD_SUBTRACT -> "Minus"
        android.view.KeyEvent.KEYCODE_COMMA -> "Comma"
        android.view.KeyEvent.KEYCODE_PERIOD, android.view.KeyEvent.KEYCODE_NUMPAD_DOT -> "Period"
        android.view.KeyEvent.KEYCODE_SLASH, android.view.KeyEvent.KEYCODE_NUMPAD_DIVIDE -> "Slash"
        android.view.KeyEvent.KEYCODE_BACKSLASH -> "Backslash"
        android.view.KeyEvent.KEYCODE_SEMICOLON -> if (e.isShiftPressed) "Colon" else "Semicolon"
        android.view.KeyEvent.KEYCODE_APOSTROPHE -> "Quote"
        android.view.KeyEvent.KEYCODE_GRAVE -> "Backtick"
        android.view.KeyEvent.KEYCODE_LEFT_BRACKET -> "OpenBracket"
        android.view.KeyEvent.KEYCODE_RIGHT_BRACKET -> "CloseBracket"
        else -> null
    }
}

/**
 * Custom theme cards (S11): each a small live mock of the phone's own UI in that theme's
 * colours (`palette`, the same map as `shell.customPalette`); a tap selects the theme.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ThemeCards(cards: List<JsonObject>, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    FlowRow(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        for (card in cards) {
            val name = card["name"].str
            val selected = card["selected"].flag()
            Column(Modifier.clip(RoundedCornerShape(8.dp))
                .then(if (selected) Modifier.border(2.dp, c.focus, RoundedCornerShape(8.dp)) else Modifier)
                .clickable { send("settings.action", mapOf("id" to "appearance.selectTheme", "values" to mapOf("name" to name))) }.padding(4.dp)) {
                val palette = card["palette"].obj
                if (palette != null) {
                    val tc = YaacColors.of("custom", palette, c.dark)
                    androidx.compose.runtime.CompositionLocalProvider(LocalYaac provides tc) { ThemeMock() }
                } else Box(Modifier.width(150.dp).height(96.dp).background(c.input), contentAlignment = Alignment.Center) {
                    T(card["status"].text ?: card["error"].text ?: "", type.small, c.muted)
                }
                Row(Modifier.padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (selected) { Icon(YIcon.Check, c.text, size = 14.dp); Spacer(Modifier.width(4.dp)) }
                    T(name, type.small, maxLines = 1)
                }
                card["error"].text?.let { T(it, type.small, c.failure, maxLines = 2) }
            }
        }
    }
}

/** A thread in miniature: header, a user bubble, agent prose and a request card. */
@Composable
private fun ThemeMock() {
    val c = LocalYaac.current
    val tiny = LocalType.current.small.copy(fontSize = 7.sp, lineHeight = 9.sp)
    Column(Modifier.width(150.dp).height(96.dp).clip(RoundedCornerShape(6.dp)).background(c.canvas).padding(6.dp)) {
        T("Fix frame decoder", tiny.copy(fontSize = 8.sp), c.text, maxLines = 1)
        T("Needs you · approval", tiny, c.warning, maxLines = 1)
        Box(Modifier.align(Alignment.End).padding(top = 3.dp).clip(RoundedCornerShape(3.dp)).background(c.user).padding(horizontal = 4.dp, vertical = 2.dp)) {
            T("Why does it drop a frame?", tiny, c.text, maxLines = 1)
        }
        T("The prefix counts the kind byte.", tiny, c.text, maxLines = 1, modifier = Modifier.padding(top = 3.dp))
        Row(Modifier.padding(top = 3.dp).fillMaxWidth().clip(RoundedCornerShape(3.dp)).background(c.card)) {
            Box(Modifier.width(2.dp).height(22.dp).background(c.warning))
            Column(Modifier.padding(3.dp)) {
                T("Run a command", tiny, c.text, maxLines = 1)
                Box(Modifier.clip(RoundedCornerShape(2.dp)).background(c.accent).padding(horizontal = 4.dp)) { T("Allow once", tiny, c.onAccent, maxLines = 1) }
            }
        }
    }
}

/**
 * Settings → Voice call language: this phone's own choice (core/VoiceLanguage.kt), never the
 * hub's. Calls and dictation speak it; "Same as this device" follows the phone's language.
 */
@Composable
private fun VoiceLanguageRow() {
    val c = LocalYaac.current
    val type = LocalType.current
    val context = androidx.compose.ui.platform.LocalContext.current
    val device = java.util.Locale.getDefault()
    var chosen by remember { mutableStateOf(dev.yaac.android.core.VoiceLanguage.choice(context)) }
    var open by rememberSaveable { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().clickable { open = !open }.heightIn(min = 52.dp).padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            T("Voice call language", type.ui.copy(fontSize = 16.sp))
            T(dev.yaac.android.core.VoiceLanguage.label(chosen, device), type.small, c.muted)
        }
        Icon(YIcon.ChevronRight, c.muted, size = 18.dp)
    }
    if (open) {
        val options = listOf("" to dev.yaac.android.core.VoiceLanguage.label("", device)) + dev.yaac.android.core.VoiceLanguage.choices
        for ((tag, name) in options) {
            Row(Modifier.fillMaxWidth().clickable {
                dev.yaac.android.core.VoiceLanguage.choose(context, tag)
                chosen = tag
                open = false
            }.heightIn(min = 44.dp).padding(start = 36.dp, end = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                T(name, type.ui.copy(fontSize = 15.sp), if (tag == chosen) c.text else c.muted, modifier = Modifier.weight(1f))
                if (tag == chosen) T("✓", type.ui, c.text)
            }
        }
    }
}

/**
 * Settings → Mark where I left off: this phone's own toggle (core/LeftOff.kt, on by default).
 * With it on, a thread opens where you stopped reading, below "New since you left".
 */
@Composable
private fun LeftOffRow() {
    val c = LocalYaac.current
    val type = LocalType.current
    val context = androidx.compose.ui.platform.LocalContext.current
    var on by remember { mutableStateOf(dev.yaac.android.core.LeftOff.enabled(context)) }
    Row(Modifier.fillMaxWidth().clickable(onClickLabel = if (on) "Turn off" else "Turn on") {
        on = !on
        dev.yaac.android.core.LeftOff.setEnabled(context, on)
    }.heightIn(min = 52.dp).padding(horizontal = 20.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            T("Mark where I left off", type.ui.copy(fontSize = 16.sp))
            T("Open a thread where you stopped reading, with a line above what is new", type.small, c.muted)
        }
        Spacer(Modifier.width(12.dp))
        Switch(on, true)
    }
}
