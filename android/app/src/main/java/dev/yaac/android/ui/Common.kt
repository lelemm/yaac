package dev.yaac.android.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

@Composable
fun T(
    text: String,
    style: TextStyle = LocalType.current.ui,
    color: Color = LocalYaac.current.text,
    modifier: Modifier = Modifier,
    maxLines: Int = Int.MAX_VALUE,
    weight: FontWeight? = null,
    align: TextAlign? = null,
) {
    BasicText(
        text, modifier,
        style = style.copy(color = color, fontWeight = weight ?: style.fontWeight, textAlign = align ?: TextAlign.Unspecified),
        maxLines = maxLines, overflow = TextOverflow.Ellipsis,
    )
}

/** The mark for a state word, as `SidebarItems.Mark` and `MainWindow.Header.MarkGlyph` draw it. */
@Composable
fun StateMark(tone: String?, glyph: String? = null, size: Dp = 16.dp, fast: Boolean = false, ultrafast: Boolean = false) {
    val c = LocalYaac.current
    when (tone) {
        "running" -> Icon(YIcon.Play, if (ultrafast) c["YaacUltrafast"] else if (fast) c["YaacFast"] else c.accent, size = size * 0.8f)
        "waiting" -> Icon(YIcon.Hourglass, c.muted, size = size, strokeWidth = 1.8f)
        "needs", "needsYou", "needs you" -> T("!", LocalType.current.title.copy(fontSize = 18.sp), c.failure, weight = FontWeight.Bold)
        "paused" -> Icon(YIcon.Pause, c.muted, size = size * 0.8f)
        "failed" -> Icon(YIcon.Close, c.failure, size = size)
        "done" -> Icon(YIcon.Check, c.success, size = size)
        "offline", "off" -> Icon(YIcon.Unlink, c.muted, size = size, strokeWidth = 1.8f)
        else -> if (!glyph.isNullOrEmpty()) T(glyph, color = c.muted) else Spacer(Modifier.size(size))
    }
}

/** "(!) 2": opens the next thread that needs you (`shell.needs`). Hidden when nothing does. */
@Composable
fun NeedsButton(count: Int, onClick: () -> Unit) {
    if (count <= 0) return
    val c = LocalYaac.current
    Row(
        Modifier.clip(RoundedCornerShape(6.dp)).clickable(onClick = onClick).padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        T("!", LocalType.current.title, c.failure, weight = FontWeight.Bold)
        Spacer(Modifier.width(4.dp))
        T(count.toString(), LocalType.current.title.copy(fontWeight = FontWeight.Normal), c.failure)
    }
}

@Composable
fun IconButton(icon: YIcon, description: String, tint: Color = LocalYaac.current.text, size: Dp = 22.dp, enabled: Boolean = true, strokeWidth: Float = 2f, onClick: () -> Unit) {
    Box(
        Modifier.size(44.dp).clip(RoundedCornerShape(6.dp)).clickable(enabled = enabled, onClickLabel = description, onClick = onClick)
            .alpha(if (enabled) 1f else 0.4f),
        contentAlignment = Alignment.Center,
    ) { Icon(icon, tint, size = size, strokeWidth = strokeWidth) }
}

/** The one filled button on a screen. */
@Composable
fun PrimaryButton(label: String, enabled: Boolean = true, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = LocalYaac.current
    // Dimmed by colour, not by a layer: a graphics layer around the button failed to draw on the
    // software-rendered emulator.
    val a = if (enabled) 1f else 0.45f
    Box(
        modifier.heightIn(min = 40.dp).clip(RoundedCornerShape(6.dp)).background(c.accent.copy(alpha = a))
            .clickable(enabled = enabled, onClick = onClick).padding(horizontal = 18.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) { T(label, LocalType.current.ui.copy(fontSize = 15.sp), c.onAccent.copy(alpha = if (enabled) 1f else 0.7f), weight = FontWeight.SemiBold, maxLines = 1) }
}

/** A plain text button: no outline, no fill. */
@Composable
fun TextAction(label: String, enabled: Boolean = true, color: Color = LocalYaac.current.text, modifier: Modifier = Modifier, style: TextStyle = LocalType.current.ui.copy(fontSize = 15.sp), onClick: () -> Unit) {
    Box(
        modifier.heightIn(min = 40.dp).clip(RoundedCornerShape(6.dp)).clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) { T(label, style, color.copy(alpha = if (enabled) color.alpha else 0.45f), maxLines = 1) }
}

@OptIn(ExperimentalFoundationApi::class)
fun Modifier.tapAndHold(onTap: () -> Unit, onHold: (() -> Unit)?): Modifier =
    combinedClickable(onClick = onTap, onLongClick = onHold)

/** An action descriptor: `{id, label, enabled, detail?, danger?, checked?, children?}`; empty id = separator. */
data class ActionItem(val id: String, val label: String, val enabled: Boolean, val detail: String?, val danger: Boolean, val checked: Boolean?, val children: List<ActionItem>, val primary: Boolean) {
    val separator get() = id.isEmpty() && label.isEmpty()
    companion object {
        fun of(e: JsonElement?): ActionItem? {
            val o = e as? JsonObject ?: return null
            val checked = o["checked"]
            return ActionItem(
                o["id"].str, o["label"].str, o["enabled"].flag(), o["detail"].text, o["danger"].flag(),
                if (checked is kotlinx.serialization.json.JsonPrimitive && !checked.isString && checked !is kotlinx.serialization.json.JsonNull) checked.flag() else null,
                o["children"].objects.mapNotNull { of(it) }, o["primary"].flag(),
            )
        }
        fun list(e: JsonElement?): List<ActionItem> = e.objects.mapNotNull { of(it) }
    }
}

/** A context menu delivered as a bottom sheet: the same items, order and separators (P375). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ActionSheet(title: String?, items: List<ActionItem>, onDismiss: () -> Unit, keepOpenOnCheck: Boolean = false, onPick: (ActionItem) -> Unit) {
    val c = LocalYaac.current
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var path by remember { mutableStateOf<List<ActionItem>>(emptyList()) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = c.raised, contentColor = c.text, tonalElevation = 0.dp, scrimColor = Color.Black.copy(alpha = 0.45f)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(bottom = 12.dp)) {
            val heading = path.lastOrNull()?.label ?: title
            if (path.isNotEmpty()) {
                Row(Modifier.fillMaxWidth().clickable { path = path.dropLast(1) }.padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(YIcon.Back, c.muted, size = 18.dp); Spacer(Modifier.width(10.dp)); T(heading ?: "", LocalType.current.heading)
                }
            } else if (!heading.isNullOrEmpty()) {
                T(heading, LocalType.current.heading, modifier = Modifier.padding(horizontal = 20.dp, vertical = 10.dp))
            }
            val shown = path.lastOrNull()?.children ?: items
            for (item in shown) {
                if (item.separator) { Spacer(Modifier.height(6.dp)); Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp).height(1.dp).background(c.rule)); Spacer(Modifier.height(6.dp)); continue }
                MenuRow(item) {
                    if (item.children.isNotEmpty()) path = path + item
                    else { onPick(item); if (!(keepOpenOnCheck && item.checked != null)) onDismiss() }
                }
            }
        }
    }
}

@Composable
fun MenuRow(item: ActionItem, onClick: () -> Unit) {
    val c = LocalYaac.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(enabled = item.enabled || item.children.isNotEmpty(), onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(28.dp)) { if (item.checked == true) Icon(YIcon.Check, c.text, size = 18.dp) }
        Column(Modifier.weight(1f)) {
            T(item.label, LocalType.current.ui.copy(fontSize = 15.sp), if (item.danger) c.failure else c.text, modifier = Modifier.alpha(if (item.enabled || item.children.isNotEmpty()) 1f else 0.5f))
            item.detail?.let { T(it, LocalType.current.small, c.muted) }
        }
        if (item.children.isNotEmpty()) Icon(YIcon.ChevronRight, c.muted, size = 18.dp)
    }
}

/** `shell.dialog`: title, body, fields, submit and cancel (`MainWindow.Overlays`). */
@Composable
fun ShellDialog(data: JsonObject, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val id = data["id"].str
    val instance = data["instance"].str
    val values = remember(id, instance, data["fieldRevision"]) { mutableStateMapOf<String, Any?>() }
    val fields = data["fields"].objects
    for (f in fields) {
        val fid = f["id"].str
        if (fid !in values) values[fid] = if (f["type"].str == "check") f["value"].flag() else f["value"].text ?: ""
    }
    // Full width on a phone: dialogs such as Fork carry long previews (A18).
    Dialog(onDismissRequest = { send("dialog.cancel", mapOf("id" to id, "instance" to instance)) },
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 24.dp).clip(RoundedCornerShape(8.dp)).background(c.raised).padding(20.dp).verticalScroll(rememberScrollState())) {
            T(data["title"].str, LocalType.current.title)
            data["body"].text?.let { Spacer(Modifier.height(10.dp)); T(it, LocalType.current.ui, c.text) }
            if (id == "pullRequest") PullRequestForm(data, id, instance, values, send)
            for (f in fields) {
                val fid = f["id"].str
                val label = f["label"].str
                Spacer(Modifier.height(10.dp))
                when (f["type"].str) {
                    "check" -> Row(Modifier.fillMaxWidth().clickable { values[fid] = !(values[fid] as? Boolean ?: false) }.padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(20.dp).border(1.5.dp, c.muted, RoundedCornerShape(4.dp)), contentAlignment = Alignment.Center) {
                            if (values[fid] == true) Icon(YIcon.Check, c.text, size = 16.dp)
                        }
                        Spacer(Modifier.width(10.dp)); T(label)
                    }
                    "choice" -> {
                        T(label, LocalType.current.small, c.muted)
                        for (o in f["options"].objects) {
                            val oid = o["id"].str
                            val disabled = o["disabled"].flag()
                            Row(Modifier.fillMaxWidth().clickable(enabled = !disabled) {
                                values[fid] = oid
                                send("dialog.change", mapOf("id" to id, "instance" to instance, "values" to mapOf(fid to oid)))
                            }.padding(vertical = 8.dp).alpha(if (disabled) 0.5f else 1f), verticalAlignment = Alignment.CenterVertically) {
                                Box(Modifier.width(26.dp)) { if (values[fid] == oid) Icon(YIcon.Check, c.text, size = 16.dp) }
                                Column { T(o["label"].str); o["detail"].text?.let { T(it, LocalType.current.small, c.muted) } }
                            }
                        }
                    }
                    "date" -> {
                        // The six-row calendar (A02d): a date picker restyled to YAAC tokens.
                        T(label, LocalType.current.small, c.muted)
                        var picking by remember { mutableStateOf(false) }
                        Row(Modifier.padding(top = 4.dp).clip(RoundedCornerShape(6.dp)).background(c.input).clickable { picking = true }.heightIn(min = 44.dp).padding(horizontal = 12.dp),
                            verticalAlignment = Alignment.CenterVertically) { T(values[fid] as? String ?: "", LocalType.current.ui); Spacer(Modifier.width(8.dp)); Icon(YIcon.CaretDown, c.text, size = 11.dp) }
                        if (picking) DatePick(values[fid] as? String, f["minDate"].text, { picking = false }) { d ->
                            values[fid] = d
                            send("dialog.change", mapOf("id" to id, "instance" to instance, "values" to mapOf(fid to d)))
                        }
                    }
                    else -> {
                        T(label, LocalType.current.small, c.muted)
                        Spacer(Modifier.height(4.dp))
                        BasicTextField(
                            values[fid] as? String ?: "", { v ->
                                values[fid] = v
                                if (f["live"].flag()) send("dialog.change", mapOf("id" to id, "instance" to instance, "values" to mapOf(fid to v)))
                            },
                            textStyle = LocalType.current.ui.copy(color = c.text), cursorBrush = SolidColor(c.text),
                            singleLine = f["type"].str != "multiline",
                            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(c.input).padding(10.dp).heightIn(min = if (f["type"].str == "multiline") 70.dp else 0.dp),
                        )
                    }
                }
            }
            data["hint"].text?.let { Spacer(Modifier.height(8.dp)); T(it, LocalType.current.small, c.muted) }
            Spacer(Modifier.height(16.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
                val cancel = if (id in setOf("mode", "ultrafast", "pullRequest", "project", "mute", "fork", "quit")) "Cancel" else "Close"
                if (id != "updateNotice") TextAction(cancel) { send("dialog.cancel", mapOf("id" to id, "instance" to instance)) }
                val submit = data["submit"].str
                val blocked = data["submitBlocked"].text
                if (submit.isNotEmpty()) {
                    Spacer(Modifier.width(8.dp))
                    PrimaryButton(submit, enabled = blocked == null) {
                        send("dialog.submit", mapOf("id" to id, "instance" to instance, "values" to values.toMap()))
                    }
                }
            }
            data["submitBlocked"].text?.let { T(it, LocalType.current.small, c.muted, modifier = Modifier.padding(top = 6.dp)) }
        }
    }
}

/** Five-segment usage bar (`UsageMeter`). */
@Composable
fun SegmentBar(segments: List<Double>, near: Boolean, modifier: Modifier = Modifier) {
    val c = LocalYaac.current
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        for (s in segments) {
            Box(Modifier.width(13.dp).height(7.dp).clip(RoundedCornerShape(2.dp)).background(c.rule)) {
                val fill = if (near && s > 0) c.warning else c.accent.copy(alpha = 0.85f)
                Box(Modifier.fillMaxWidth(s.toFloat().coerceIn(0f, 1f)).height(7.dp).background(fill))
            }
        }
    }
}

fun parseColor(hex: String?): Color? {
    val h = hex?.removePrefix("#") ?: return null
    return h.toLongOrNull(16)?.let { if (h.length == 6) Color(0xFF000000 or it) else if (h.length == 8) Color(it) else null }
}

/** Create pull request (A19, WPF's `PullRequestForm`): Into, Title, Description, Draft, Ask the agent. */
@Composable
private fun PullRequestForm(data: JsonObject, id: String, instance: String, values: MutableMap<String, Any?>, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    var revision by remember(instance) { mutableStateOf(0L) }
    var title by remember(instance) { mutableStateOf(data["prTitle"].str) }
    var body by remember(instance) { mutableStateOf(data["description"].str) }
    // Earlier snapshots may arrive while the user types: adopt only those at or after our edits.
    val remoteRevision = (data["editRevision"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull() ?: 0L
    androidx.compose.runtime.LaunchedEffect(data["prTitle"], data["description"], remoteRevision) {
        if (remoteRevision >= revision) { title = data["prTitle"].str; body = data["description"].str }
    }
    if (values["draft"] == null) values["draft"] = data["draft"].flag()
    val busy = data["creating"].flag() || data["writing"].flag()
    fun edit() { revision += 1; send("dialog.change", mapOf("id" to id, "instance" to instance, "editRevision" to revision, "values" to mapOf("title" to title, "body" to body))) }
    var into by remember { mutableStateOf(false) }
    val branches = data["branches"].objects
    Spacer(Modifier.height(10.dp))
    T("Into", type.small, c.muted)
    Row(Modifier.clip(RoundedCornerShape(6.dp)).background(c.input).clickable(enabled = !data["creating"].flag() && branches.isNotEmpty()) { into = true }.heightIn(min = 44.dp).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically) {
        T(branches.firstOrNull { it["id"].str == data["base"].str }?.get("label").text ?: data["base"].str, type.ui); Spacer(Modifier.width(6.dp)); Icon(YIcon.CaretDown, c.text, size = 11.dp)
    }
    if (into) ActionSheet("Into", branches.map { ActionItem(it["id"].str, it["label"].str, true, null, false, it["id"].str == data["base"].str, emptyList(), false) }, { into = false }) {
        send("dialog.change", mapOf("id" to id, "instance" to instance, "values" to mapOf("base" to it.id)))
    }
    for ((label, value, set, lines) in listOf(
        Quad("Title", title, { v: String -> title = v; edit() }, 1),
        Quad("Description", body, { v: String -> body = v; edit() }, 6),
    )) {
        Spacer(Modifier.height(10.dp))
        T(label, type.small, c.muted)
        BasicTextField(value, set, enabled = !busy, singleLine = lines == 1, textStyle = type.ui.copy(color = c.text), cursorBrush = SolidColor(c.text),
            modifier = Modifier.fillMaxWidth().padding(top = 4.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(10.dp).heightIn(min = if (lines > 1) 110.dp else 0.dp))
    }
    Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(Modifier.clickable(enabled = !data["creating"].flag()) { values["draft"] = !(values["draft"] as? Boolean ?: false) }.padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(20.dp).border(1.5.dp, c.muted, RoundedCornerShape(4.dp)), contentAlignment = Alignment.Center) { if (values["draft"] == true) Icon(YIcon.Check, c.text, size = 16.dp) }
            Spacer(Modifier.width(8.dp)); T("Draft")
        }
        Spacer(Modifier.weight(1f))
        TextAction("Ask the agent to write it", data["askBlocked"].text == null) { send("dialog.askAgent", mapOf("id" to id, "instance" to instance)) }
    }
    data["askBlocked"].text?.let { T(it, type.small, c.muted) }
    if (data["loading"].flag()) T("Reading the branch…", type.small, c.muted)
    data["agentStatus"].text?.let { T(it, type.small, if (data["agentFailed"].flag()) c.failure else c.muted) }
    data["notice"].text?.let { T(it, type.small, c.muted) }
    data["warning"].text?.let { T(it, type.small, c.warning) }
    data["error"].text?.let { T(it, type.small, c.failure) }
}

private data class Quad<A, B, C, D>(val a: A, val b: B, val c: C, val d: D)

@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun DatePick(value: String?, minDate: String?, onDismiss: () -> Unit, onPick: (String) -> Unit) {
    val c = LocalYaac.current
    fun millis(d: String?) = d?.let { runCatching { java.time.LocalDate.parse(it).atStartOfDay(java.time.ZoneOffset.UTC).toInstant().toEpochMilli() }.getOrNull() }
    val min = millis(minDate)
    val state = androidx.compose.material3.rememberDatePickerState(initialSelectedDateMillis = millis(value),
        selectableDates = object : androidx.compose.material3.SelectableDates {
            override fun isSelectableDate(utcTimeMillis: Long) = min == null || utcTimeMillis >= min
        })
    val colors = androidx.compose.material3.DatePickerDefaults.colors(containerColor = c.raised, titleContentColor = c.muted, headlineContentColor = c.text,
        weekdayContentColor = c.muted, dayContentColor = c.text, selectedDayContainerColor = c.accent, selectedDayContentColor = c.onAccent,
        todayContentColor = c.text, todayDateBorderColor = c.focus, disabledDayContentColor = c.muted.copy(alpha = 0.4f), navigationContentColor = c.text,
        yearContentColor = c.text, selectedYearContainerColor = c.accent, selectedYearContentColor = c.onAccent, currentYearContentColor = c.text)
    androidx.compose.material3.DatePickerDialog(onDismissRequest = onDismiss, colors = colors,
        confirmButton = { TextAction("OK") {
            state.selectedDateMillis?.let { onPick(java.time.Instant.ofEpochMilli(it).atZone(java.time.ZoneOffset.UTC).toLocalDate().toString()) }
            onDismiss()
        } },
        dismissButton = { TextAction("Cancel", onClick = onDismiss) }) {
        androidx.compose.material3.DatePicker(state, colors = colors, showModeToggle = false)
    }
}
