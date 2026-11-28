package dev.yaac.android.ui

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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Slider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.yaac.android.core.DialogValues
import dev.yaac.android.core.double
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.list
import dev.yaac.android.core.objects
import dev.yaac.android.core.scalar
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/**
 * An external dialog (docs/16-external-dialogs.md), built at runtime from the integrator's layout
 * tree in the frame (`dialog.layout`), as WPF's `DialogLayoutView` builds it. [values] holds what
 * the user entered, by input id, in memory only; [onPress] gets a button's id — the caller sends
 * it with the values, and the controller validates and answers once. Unknown node types draw a
 * muted line, never a crash.
 */
@Composable
fun DialogLayout(dialog: JsonObject, values: SnapshotStateMap<String, Any?>, onPress: (String) -> Unit) {
    val c = LocalYaac.current
    val layout = dialog["layout"]
    // Defaults once per layout; what was typed survives the next frames.
    remember(layout) { DialogValues.initial(layout).forEach { (k, v) -> if (k !in values) values[k] = v }; 0 }
    val enabled = dialog["enabled"].flag()
    val problems = dialog["problems"]
    val newer = dialog["newerYaac"].text ?: "This part of the dialog needs a newer YAAC"
    dialog["source"].text?.let { T(it, LocalType.current.small, c.muted) }
    dialog["gone"].text?.let { T(it, LocalType.current.small, c.warning) }
    problems[""].text?.let { T(it, LocalType.current.small, c.failure) }
    Spacer(Modifier.height(6.dp))
    Node(layout, values, enabled, problems, newer, onPress, 0)
}

@Composable
private fun Header(n: JsonObject) {
    n["label"].text?.let { T(it + if (n["required"].flag()) " *" else "", LocalType.current.ui, weight = FontWeight.SemiBold, modifier = Modifier.padding(top = 6.dp)) }
}

@Composable
private fun Footer(n: JsonObject, problems: JsonElement?) {
    val c = LocalYaac.current
    n["help"].text?.let { T(it, LocalType.current.small, c.muted) }
    n["id"].text?.let { id -> problems[id].text?.let { T(it, LocalType.current.small, c.failure) } }
}

@Composable
private fun Field(value: String, enabled: Boolean, onChange: (String) -> Unit, secret: Boolean = false, multiline: Boolean = false, number: Boolean = false, hint: String? = null) {
    val c = LocalYaac.current
    Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(horizontal = 10.dp, vertical = 10.dp).alpha(if (enabled) 1f else 0.5f)) {
        if (value.isEmpty() && hint != null) T(hint, LocalType.current.ui.copy(fontSize = 15.sp), c.muted)
        BasicTextField(
            value, onChange, enabled = enabled, singleLine = !multiline,
            textStyle = LocalType.current.ui.copy(color = c.text, fontSize = 15.sp), cursorBrush = SolidColor(c.text),
            visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
            keyboardOptions = KeyboardOptions(keyboardType = when { secret -> KeyboardType.Password; number -> KeyboardType.Decimal; else -> KeyboardType.Text }),
            modifier = Modifier.fillMaxWidth().heightIn(min = if (multiline) 70.dp else 0.dp),
        )
    }
}

@Composable
private fun Choice(label: String, detail: String?, checked: Boolean, multi: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val c = LocalYaac.current
    Row(Modifier.fillMaxWidth().clickable(enabled = enabled, onClick = onClick).heightIn(min = 40.dp).alpha(if (enabled) 1f else 0.5f), verticalAlignment = Alignment.CenterVertically) {
        val shape = RoundedCornerShape(if (multi) 4.dp else 10.dp)
        Box(Modifier.size(20.dp).clip(shape).border(1.5.dp, if (checked) c.text else c.muted, shape), contentAlignment = Alignment.Center) {
            if (checked) Box(Modifier.size(10.dp).clip(RoundedCornerShape(if (multi) 2.dp else 5.dp)).background(c.text))
        }
        Spacer(Modifier.width(10.dp))
        Column { T(label); detail?.let { T(it, LocalType.current.small, c.muted) } }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Node(e: JsonElement?, values: SnapshotStateMap<String, Any?>, enabled: Boolean, problems: JsonElement?, newer: String, onPress: (String) -> Unit, depth: Int) {
    val n = e as? JsonObject ?: return
    if (depth > 16) return
    val c = LocalYaac.current
    val type = LocalType.current
    val id = n["id"].str
    val on = enabled && !n["disabled"].flag()
    val gap = (n["gap"].double ?: 6.0).coerceIn(0.0, 64.0).dp
    when (n["type"].str) {
        "column" -> Column(verticalArrangement = Arrangement.spacedBy(gap)) { for (child in n["children"].list) Node(child, values, enabled, problems, newer, onPress, depth + 1) }
        "row" -> FlowRow(horizontalArrangement = Arrangement.spacedBy(gap), verticalArrangement = Arrangement.spacedBy(4.dp), itemVerticalAlignment = Alignment.CenterVertically) {
            for (child in n["children"].list) Node(child, values, enabled, problems, newer, onPress, depth + 1)
        }
        "group" -> Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).border(1.dp, c.rule, RoundedCornerShape(6.dp)).padding(10.dp)) {
            n["label"].text?.let { T(it, type.ui, weight = FontWeight.SemiBold) }
            for (child in n["children"].list) Node(child, values, enabled, problems, newer, onPress, depth + 1)
        }
        "divider" -> Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).height(1.dp).background(c.rule))
        "text" -> {
            val style = n["style"].str
            val s = when (style) { "heading" -> type.title; "caption" -> type.small; "code" -> type.mono; else -> type.ui.copy(fontSize = 15.sp) }
            T(n["text"].str, s, c.tone(n["tone"].text, c.text))
        }
        "markdown" -> T(n["text"].str, type.ui.copy(fontSize = 15.sp))
        "image" -> T("🖼 " + n["alt"].str, type.small, c.muted)
        "link" -> {
            val uri = LocalUriHandler.current
            val url = n["url"].str
            T(n["label"].str, type.ui.copy(fontSize = 15.sp), c.accent, modifier = Modifier.clickable {
                if (url.startsWith("https://") || url.startsWith("http://")) runCatching { uri.openUri(url) }
            })
        }
        "list" -> for (item in n["items"].list) T("•  " + item.str)
        "table" -> Column {
            Row { for (col in n["columns"].list) T(col.str, type.small, weight = FontWeight.SemiBold, modifier = Modifier.weight(1f)) }
            for (row in n["rows"].list) Row { for (cell in row.list) T(cell.str, type.small, modifier = Modifier.weight(1f)) }
        }
        "progress" -> {
            val v = n["value"].double
            if (v != null) LinearProgressIndicator(progress = { v.toFloat().coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
            else LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
        }
        "textInput" -> {
            Header(n)
            Field(values[id] as? String ?: "", on, { values[id] = it }, secret = n["secret"].flag(), multiline = n["multiline"].flag(), hint = n["placeholder"].text)
            Footer(n, problems)
        }
        "number", "date" -> {
            Header(n)
            Field(values[id] as? String ?: "", on, { values[id] = it }, number = n["type"].str == "number", hint = if (n["type"].str == "date") "YYYY-MM-DD" else null)
            Footer(n, problems)
        }
        "slider" -> {
            Header(n)
            val min = n["min"].double ?: 0.0
            val max = (n["max"].double ?: min).coerceAtLeast(min)
            val step = n["step"].double?.takeIf { it > 0 }
            val v = (values[id] as? String)?.toDoubleOrNull() ?: min
            val steps = step?.let { (((max - min) / it).toInt() - 1).coerceIn(0, 200) } ?: 0
            Row(verticalAlignment = Alignment.CenterVertically) {
                Slider(value = v.toFloat(), onValueChange = { f -> values[id] = trim(f.toDouble()) }, valueRange = min.toFloat()..max.toFloat(), steps = steps, enabled = on, modifier = Modifier.weight(1f))
                T(values[id] as? String ?: trim(v), type.small, c.muted, modifier = Modifier.padding(start = 8.dp))
            }
            Footer(n, problems)
        }
        "checkbox", "toggle" -> {
            Choice(n["label"].str, null, values[id] == true, multi = true, enabled = on) { values[id] = values[id] != true }
            Footer(n, problems)
        }
        "select" -> {
            Header(n)
            var open by remember { mutableStateOf(false) }
            val options = n["options"].objects
            val shown = options.firstOrNull { it["value"].str == values[id] }?.get("label").text ?: n["placeholder"].text ?: "Choose…"
            Row(Modifier.padding(top = 4.dp).fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(c.input).clickable(enabled = on) { open = true }
                .heightIn(min = 44.dp).padding(horizontal = 12.dp).alpha(if (on) 1f else 0.5f), verticalAlignment = Alignment.CenterVertically) {
                T(shown, type.ui.copy(fontSize = 15.sp), modifier = Modifier.weight(1f)); Icon(YIcon.CaretDown, c.text, size = 11.dp)
            }
            if (open) {
                val items = options.map { o -> ActionItem(o["value"].str, o["label"].str, true, o["detail"].text, false, values[id] == o["value"].str, emptyList(), false) }
                ActionSheet(n["label"].text, items, { open = false }) { picked -> values[id] = picked.id; open = false }
            }
            Footer(n, problems)
        }
        "radio" -> {
            Header(n)
            for (o in n["options"].objects) Choice(o["label"].str, o["detail"].text, values[id] == o["value"].str, multi = false, enabled = on) { values[id] = o["value"].str }
            Footer(n, problems)
        }
        "multiSelect" -> {
            Header(n)
            @Suppress("UNCHECKED_CAST") val chosen = (values[id] as? List<String>).orEmpty()
            for (o in n["options"].objects) {
                val v = o["value"].str
                Choice(o["label"].str, o["detail"].text, v in chosen, multi = true, enabled = on) { values[id] = if (v in chosen) chosen - v else chosen + v }
            }
            Footer(n, problems)
        }
        "button" -> {
            val label = n["label"].str
            when (n["kind"].str) {
                "primary" -> PrimaryButton(label, enabled) { onPress(id) }
                "danger" -> TextAction(label, enabled, color = c.failure) { onPress(id) }
                else -> TextAction(label, enabled) { onPress(id) }
            }
        }
        else -> T(newer, type.small.copy(fontStyle = FontStyle.Italic), c.muted)
    }
}

private fun trim(v: Double): String = if (v == Math.floor(v) && kotlin.math.abs(v) < 1e15) v.toLong().toString() else v.toString()

/**
 * A dialog of a target's inbox (no thread), opened from the list's inbox rows or its
 * notification: a sheet over the app. Back (or ×) closes it — a cancellation the integrator is
 * told about; **Later** only hides it, and it stays in Needs you.
 */
@Composable
fun InboxDialogSheet(data: JsonObject, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val target = data["target"].str
    val requestId = data["requestId"].str
    val values = remember(target, requestId) { androidx.compose.runtime.mutableStateMapOf<String, Any?>() }
    val pending = data["pending"].flag()
    val act: (String) -> Unit = { action ->
        val payload = mutableMapOf<String, Any?>("target" to target, "requestId" to requestId, "action" to action)
        if (action.startsWith("dialog:")) payload["values"] = DialogValues.payload(data["layout"], values)
        send("inbox.action", payload)
    }
    val close = { if (pending) act("dialogDismiss") else act("later") }
    Dialog(onDismissRequest = close, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 24.dp).clip(RoundedCornerShape(8.dp)).background(c.raised).padding(20.dp).verticalScroll(rememberScrollState())) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                T(data["title"].str, LocalType.current.title, modifier = Modifier.weight(1f))
                IconButton(YIcon.Close, "Close", onClick = close)
            }
            if (pending) DialogLayout(data, values) { act("dialog:$it") }
            else {
                data["source"].text?.let { T(it, LocalType.current.small, c.muted) }
                data["status"].text?.let { Spacer(Modifier.height(8.dp)); T(it, LocalType.current.ui, c.tone(data["tone"].text, c.muted)) }
            }
            Spacer(Modifier.height(12.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                for (a in ActionItem.list(data["actions"])) TextAction(a.label, a.enabled) { act(a.id) }
            }
        }
    }
}

/** The inbox's rows at the top of the thread list (Needs you, under their target). */
@Composable
fun InboxRows(rows: JsonElement?, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    for (r in rows.objects) {
        Row(Modifier.fillMaxWidth().clickable { send("inbox.open", mapOf("target" to r["target"].str, "requestId" to r["requestId"].str) ) }
            .heightIn(min = 52.dp).padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            StateMark("needs")
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                T(r["title"].str, LocalType.current.ui.copy(fontSize = 15.sp), maxLines = 1)
                T("${r["source"].str} · ${r["targetName"].str}", LocalType.current.small, c.muted, maxLines = 1)
            }
        }
    }
}
