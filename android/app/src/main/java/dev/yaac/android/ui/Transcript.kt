package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import dev.yaac.android.ui.transcript.peerHeaderRuns
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.list
import dev.yaac.android.core.objects
import dev.yaac.android.core.scalar
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** Local, per-row UI state kept across frames (WPF's `TranscriptRowState`). */
@Stable
class RowState {
    val expanded = mutableStateMapOf<String, Boolean>()
    val selected = mutableStateMapOf<String, Set<String>>()
    val other = mutableStateMapOf<String, String>()
    val fields = mutableStateMapOf<String, String>()
    /** An external dialog's entered values, by input id (P418); in memory only. */
    val dialog = mutableStateMapOf<String, Any?>()
    var showStamp by mutableStateOf(false)
}

@Stable
class TranscriptStates {
    private val map = HashMap<String, RowState>()
    fun of(thread: String, row: String) = map.getOrPut("$thread\u0000$row") { RowState() }
}

private val CardKinds = setOf("artifact", "request", "approval", "permission", "question", "form", "budget", "dialog")
private val ActivityKinds = setOf("call", "tool", "activity", "worked", "changes")

/**
 * One transcript row, drawn from its data as `TranscriptRowView` draws it. [send] carries a
 * `transcript.action` payload: `{id: actionId, action, threadId, answers?, fields?}`.
 */
@Composable
fun TranscriptRow(row: JsonObject, threadId: String, state: RowState, send: (Map<String, Any?>) -> Unit) {
    val kind = row["kind"].str
    val submit: (ActionItem, Boolean) -> Unit = { a, draft -> if (a.enabled && a.id.isNotEmpty()) send(payload(row, threadId, a, draft, state)) }
    when {
        kind == "shell" -> ShellRow(row, state, submit)
        kind == "user" -> UserRow(row, state, submit)
        kind in CardKinds -> CardRow(row, state, submit)
        kind in ActivityKinds -> ActivityRow(row, state, submit)
        kind == "handoff" -> HandoffRow(row)
        else -> ProseRow(row, kind, state, submit)
    }
}

private fun payload(row: JsonObject, threadId: String, a: ActionItem, includeDraft: Boolean, state: RowState): Map<String, Any?> {
    val out = linkedMapOf<String, Any?>("id" to (row["actionId"].text ?: row["id"].str), "action" to a.id, "threadId" to threadId)
    if (!includeDraft) return out
    // An external dialog's button (P418): the values entered go with it.
    if (a.id.startsWith("dialog:")) { out["values"] = dev.yaac.android.core.DialogValues.payload(row["dialog"]["layout"], state.dialog); return out }
    val answers = linkedMapOf<String, Any?>()
    for (q in row["questions"].objects) {
        val qid = q["id"].text ?: continue
        val sel = state.selected[qid].orEmpty().sorted()
        val hasOther = q["allowOther"].flag() || q["options"].list.isEmpty()
        val other = state.other[qid]?.takeIf { hasOther && it.isNotEmpty() }
        if (sel.isEmpty() && other == null && qid !in state.selected && qid !in state.other) continue
        answers[qid] = mapOf("selected" to sel, "other" to other)
    }
    val values = linkedMapOf<String, Long>()
    for (f in row["fields"].objects) {
        val fid = f["id"].text ?: continue
        var v = (state.fields[fid] ?: f["value"].scalar ?: "").toLongOrNull() ?: continue
        f["min"].scalar?.toLongOrNull()?.let { v = maxOf(v, it) }
        f["max"].scalar?.toLongOrNull()?.let { v = minOf(v, it) }
        values[fid] = v
    }
    if (answers.isNotEmpty()) out["answers"] = answers
    if (values.isNotEmpty()) out["fields"] = values
    return out
}

@Composable
private fun UserRow(row: JsonObject, state: RowState, submit: (ActionItem, Boolean) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    BoxWithConstraints(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        val max = maxWidth * 0.8f
        Column(Modifier.align(Alignment.TopEnd).widthIn(max = max), horizontalAlignment = Alignment.End) {
            Column(Modifier.clip(RoundedCornerShape(8.dp)).background(c.user).clickable { state.showStamp = !state.showStamp }.padding(horizontal = 14.dp, vertical = 10.dp)) {
                // Selectable as part of the transcript's one selection (T05, ThreadScreen).
                Column { Body(row, "user", type.prose); Parts(row, state, submit) }
                Attachments(row, submit)
            }
            val status = row["status"].text
            val stamp = if (status != null && row["actions"].list.isEmpty()) status else row["stamp"].text?.takeIf { state.showStamp }
            DisableSelection {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (state.showStamp) CopyMessage(row)
                    stamp?.let { T(it, type.small, if (status != null) c.tone(row["tone"].text, c.muted) else c.muted, modifier = Modifier.padding(top = 3.dp, end = 2.dp)) }
                }
            }
            Actions(row, submit, stacked = false)
        }
    }
}

@Composable
private fun Body(row: JsonObject, kind: String, style: androidx.compose.ui.text.TextStyle, color: Color = LocalYaac.current.text) {
    val markdown = row["markdown"].flag(kind == "agent" || kind == "task" || kind == "handoff")
    val text = row["text"].text
    if (markdown || row["textBlocks"].list.isNotEmpty()) Blocks(row["textBlocks"], text, style, color)
    else if (text != null) T(text, style, color)
}

@Composable
private fun ProseRow(row: JsonObject, kind: String, state: RowState, submit: (ActionItem, Boolean) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val task = kind == "task"
    var m = Modifier.fillMaxWidth().padding(vertical = 6.dp)
    if (task) m = m.clip(RoundedCornerShape(8.dp)).background(c.card).padding(horizontal = 12.dp, vertical = 8.dp)
    val tone = c.tone(row["tone"].text, if (kind in setOf("notice", "file", "mesh")) c.muted else c.text)
    Column(m.clickable(enabled = row["stamp"].text != null) { state.showStamp = !state.showStamp }) {
        row["taskPeer"].obj()?.let { peer ->
            // One flowing text, so a long title and the target wrap at spaces together (never
            // letter by letter in a leftover sliver); the title keeps to two lines.
            val a = ActionItem.of(peer)
            val runs = peerHeaderRuns(peer["heading"].text, peer["label"].str, peer["machine"].text)
            val text = buildAnnotatedString {
                for (r in runs) {
                    if (r.name) withStyle(SpanStyle(color = c.text, fontWeight = FontWeight.SemiBold)) { append(r.text) }
                    else withStyle(SpanStyle(color = c.muted)) { append(r.text) }
                }
            }
            BasicText(
                text,
                (if (a?.enabled == true) Modifier.clickable { submit(a, false) } else Modifier).padding(bottom = 5.dp),
                style = type.ui, maxLines = 3, overflow = TextOverflow.Ellipsis,
            )
        }
        row["title"].text?.takeIf { kind != "agent" && !task }?.let { T(it, type.ui, tone, weight = FontWeight.SemiBold) }
        // Touch and hold starts the transcript's selection, which runs across rows (T05, A11e).
        Body(row, kind, if (kind == "agent" || task) type.prose else type.ui, if (kind == "agent" || task) c.text else tone)
        if (row["streaming"].flag()) DisableSelection { T("▌", type.prose, c.muted) }
        Details(row, state, submit)
        Attachments(row, submit)
        row["status"].text?.let { T(it, type.small, c.tone(row["tone"].text, c.muted)) }
        if (state.showStamp) DisableSelection {
            Row(verticalAlignment = Alignment.CenterVertically) {
                row["stamp"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(top = 2.dp, end = 4.dp)) }
                CopyMessage(row)
            }
        }
        Actions(row, submit, stacked = false)
    }
}

@Composable
private fun HandoffRow(row: JsonObject) {
    val c = LocalYaac.current
    Column(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.weight(1f).height(1.dp).background(c.rule))
            row["divider"].text?.let { T(it, LocalType.current.small, c.muted, modifier = Modifier.padding(horizontal = 9.dp)) }
            Box(Modifier.weight(1f).height(1.dp).background(c.rule))
        }
        Body(row, "handoff", LocalType.current.ui, c.muted)
    }
}

/** A request card: the faint fill and the 3 dp amber edge; its offered decisions stacked. */
@Composable
private fun CardRow(row: JsonObject, state: RowState, submit: (ActionItem, Boolean) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val edge = if (row["kind"].str == "artifact") c.rule else c.tone(row["tone"].text, c.warning)
    Row(Modifier.fillMaxWidth().padding(vertical = 8.dp).clip(RoundedCornerShape(6.dp)).background(c.card).height(IntrinsicSize.Min)) {
        Box(Modifier.width(3.dp).fillMaxHeight().background(edge))
        Column(Modifier.weight(1f).padding(horizontal = 14.dp, vertical = 12.dp)) {
            // A worker's request mirrored here (P354): who asked, and Go to the original.
            row["taskPeer"].obj()?.let { peer ->
                Row(Modifier.padding(bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    T(peer["heading"].str, type.small, c.muted, modifier = Modifier.weight(1f), maxLines = 2)
                    ActionItem.of(peer)?.takeIf { it.enabled }?.let { a -> TextAction(a.label, style = type.small) { submit(a, false) } }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                T(row["title"].str, type.title.copy(fontSize = 18.sp), modifier = Modifier.weight(1f))
                row["publishedAt"].text?.let { T(it, type.small, c.muted) }
            }
            row["mermaidSource"].text?.let { Spacer(Modifier.height(6.dp)); MermaidBlock(it) }
            row["text"].text?.let { text ->
                Spacer(Modifier.height(6.dp))
                val lines = text.split('\n', limit = 2)
                // The first line of an approval's explanation is its label (P357), drawn muted.
                if (lines.size == 2 && row["kind"].str == "approval" && row["markdown"].flag().not()) {
                    T(lines[0], type.ui.copy(fontSize = 15.sp), c.muted); T(lines[1], type.ui.copy(fontSize = 15.sp))
                } else Body(row, row["kind"].str, type.ui.copy(fontSize = 15.sp))
            }
            Details(row, state, submit)
            Attachments(row, submit)
            DisableSelection { Questions(row, state); Fields(row, state) }
            row["dialog"].obj()?.let { d ->
                Spacer(Modifier.height(4.dp))
                DialogLayout(d, state.dialog) { id -> submit(ActionItem("dialog:$id", id, true, null, false, null, emptyList(), false), true) }
            }
            row["status"].text?.let { Spacer(Modifier.height(6.dp)); T(it, type.small, c.tone(row["tone"].text, c.muted)) }
            Spacer(Modifier.height(4.dp))
            // Decisions on a request stack full width; an artifact's actions share one row.
            Actions(row, submit, stacked = row["kind"].str != "artifact")
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ActivityRow(row: JsonObject, state: RowState, submit: (ActionItem, Boolean) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val mark = row["mark"].str
    val markerTone = c.tone(row["tone"].text, c.muted)
    val tone = if (mark == "×") c.muted else markerTone
    val pills = row["pills"].objects
    val group = ActionItem.of(row["groupToggle"])
    val toggle = ActionItem.of(row["activityToggle"])
    val call = row["kind"].str == "call"
    val canExpand = group?.enabled == true || toggle?.enabled == true || pills.isNotEmpty() ||
        row["parts"].objects.any { it["kind"].str == "image" || it["text"].text != null } ||
        listOf("detail", "message", "result", "receipt").any { row[it].text != null }
    val open = if (group?.enabled == true) row["groupOpen"].flag()
        else row["activityOpen"].let { if (it is kotlinx.serialization.json.JsonPrimitive && !it.isString && it !is kotlinx.serialization.json.JsonNull) it.flag() else null }
            ?: state.expanded["activity"] ?: row["defaultExpanded"].flag()
    val lineStyle = type.ui.copy(fontSize = 15.sp)
    Column(Modifier.fillMaxWidth()) {
        // One line, as on desktop: the mark, the lead, the pills, the preview, then the tail.
        Row(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(4.dp))
                .background(if (row["highlighted"].flag()) c.selected else Color.Transparent)
                .clickable(enabled = canExpand) {
                    when {
                        group?.enabled == true -> submit(group, false)
                        toggle?.enabled == true -> submit(toggle, false)
                        else -> state.expanded["activity"] = !open
                    }
                }.heightIn(min = 38.dp).padding(vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(22.dp), contentAlignment = Alignment.Center) {
                // A mesh mark (⑂ ↗ ↙ ⌛) says what the call did, so it stays; plain lines show the caret.
                if (canExpand && (mark.isEmpty() || mark == "•")) Icon(if (open) YIcon.CaretDown else YIcon.CaretRight, markerTone, size = 12.dp)
                else when (mark) {
                    "✓" -> Icon(YIcon.Check, markerTone, size = 16.dp)
                    "×" -> Icon(YIcon.Close, c.failure, size = 16.dp)
                    "⌛", "⧗" -> Icon(YIcon.Hourglass, markerTone, size = 16.dp, strokeWidth = 1.8f)
                    "•" -> Box(Modifier.size(5.dp).clip(RoundedCornerShape(3.dp)).background(markerTone))
                    else -> androidx.compose.foundation.text.BasicText(mark, style = lineStyle.copy(color = markerTone), softWrap = false)
                }
            }
            row["lead"].text?.let { T(it, lineStyle, tone, maxLines = 1) }
            for (p in pills.take(if (call) 2 else 3)) Box(Modifier.weight(1f, fill = false)) { Pill(p, submit) }
            if (pills.size > (if (call) 2 else 3)) T("+${pills.size - (if (call) 2 else 3)}", lineStyle, tone)
            row["after"].text?.let { T(it, lineStyle, tone, maxLines = 1) }
            if (call) row["preview"].text?.let { T("“$it”", lineStyle, tone, maxLines = 1, modifier = Modifier.weight(1f, fill = false)) }
            row["added"].text?.let { T(it, lineStyle, c.success) }
            row["removed"].text?.let { T(it, lineStyle, c.failure) }
            row["tail"].text?.let { T("· " + it.substringBefore(" · "), lineStyle, tone, maxLines = 1, modifier = Modifier.weight(1f, fill = false)) }
        }
        row["groupError"].text?.let { T(it, type.small, c.failure, modifier = Modifier.padding(start = 24.dp)) }
        if (canExpand && open && group?.enabled != true) {
            Column(Modifier.fillMaxWidth().padding(start = 24.dp, bottom = 6.dp)) {
                if (call && pills.isNotEmpty()) {
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                        row["heading"].text?.let { T(it, type.small, tone) }
                        for (p in pills) Pill(p, submit)
                    }
                }
                row["message"].text?.let { Blocks(row["messageBlocks"], it, type.ui, c.text) }
                row["receipt"].text?.let { T(it, type.small, c.muted) }
                row["result"].text?.let { Blocks(row["resultBlocks"], it, type.ui, c.text) }
                ActionItem.of(row["restAction"])?.takeIf { it.enabled }?.let { a -> TextAction(a.label) { submit(a, false) } }
                Details(row, state, submit, activity = true)
            }
        }
    }
}

/** A peer pill: harness logo and name, on the faint fill. Tapping opens the peer when enabled. */
@Composable
fun Pill(p: JsonObject, submit: (ActionItem, Boolean) -> Unit) {
    val c = LocalYaac.current
    val enabled = p["enabled"].flag()
    val label = p["label"].str + (p["machine"].text?.let { " · $it" } ?: "")
    Row(
        Modifier.clip(RoundedCornerShape(12.dp)).background(c.input).clickable(enabled = enabled) { ActionItem.of(p)?.let { submit(it, false) } }
            .padding(horizontal = 9.dp, vertical = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        p["logo"].text?.let { Logo(it, parseColor(p["logoColor"].text) ?: c.muted, size = 13.dp); Spacer(Modifier.width(5.dp)) }
        T(label, LocalType.current.ui, if (enabled) c.text else c.muted, maxLines = 1, modifier = Modifier.widthIn(max = 200.dp))
    }
}

/** `detail`/`detailBlocks` and the row's parts, as `UpdateDetails`/`UpdatePart`. */
@Composable
private fun Details(row: JsonObject, state: RowState, submit: (ActionItem, Boolean) -> Unit, activity: Boolean = false) {
    val c = LocalYaac.current
    val type = LocalType.current
    val detail = row["detail"].text
    if (detail == null && row["parts"].list.isEmpty()) return
    var label = row["disclosureLabel"].text
    if (label == null && !activity && detail != null && row["kind"].str in setOf("file", "mesh")) label = "Details"
    val body: @Composable () -> Unit = {
        if (detail != null) Blocks(row["detailBlocks"], detail, if (activity) type.small else type.ui, c.muted)
        Parts(row, state, submit)
    }
    if (label == null) body() else Disclosure(label, null, state, "details", row["expanded"].flag(), body)
}

@Composable
private fun Parts(row: JsonObject, state: RowState, submit: (ActionItem, Boolean) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    row["parts"].objects.forEachIndexed { i, part ->
        val kind = part["kind"].str
        val pid = part["id"].text ?: i.toString()
        val text = part["text"].str
        var label = part["label"].text
        val color = c.tone(part["tone"].text, if (kind == "detail") c.muted else c.text)
        Spacer(Modifier.height(4.dp))
        if (kind == "image") { ImagePart(part) { a -> submit(a, false) }; return@forEachIndexed }
        if (kind == "pasted") label = label ?: (text.count { it == '\n' } + 1).let { "Pasted text · $it ${if (it == 1) "line" else "lines"}" }
        if (kind == "detail") label = label ?: "Details"
        if (kind == "code") label = label ?: part["language"].text
        val show: @Composable () -> Unit = {
            when (kind) {
                "code", "diff" -> CodeBlock(text, text, part["language"].text, kind == "diff", lineNumbers = kind == "code")
                "command" -> T(text.replace("`", ""), type.mono, color)
                "markdown", "pasted" -> Blocks(part["blocks"], text, if (row["kind"].str == "user") type.prose else type.ui, color)
                else -> T(text, type.ui, color)
            }
        }
        if (label == null || kind == "text" || kind == "markdown" || kind == "command") {
            show()
            if (label != null && kind == "command") T(label, type.small, c.muted)
        } else Disclosure(label, part["preview"].text, state, "part:$pid", part["expanded"].flag(kind == "code"), show)
    }
}

@Composable
private fun Disclosure(label: String, preview: String?, state: RowState, key: String, default: Boolean, content: @Composable () -> Unit) {
    val c = LocalYaac.current
    val open = state.expanded[key] ?: default
    DisableSelection { Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(4.dp)).clickable { state.expanded[key] = !open }.heightIn(min = 36.dp).padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(if (open) YIcon.CaretDown else YIcon.CaretRight, c.text, size = 12.dp)
        Spacer(Modifier.width(10.dp))
        T(if (open && preview != null) "Hide command" else label, LocalType.current.ui.copy(fontSize = 15.sp))
        if (!open && preview != null) { Spacer(Modifier.width(10.dp)); T(preview, LocalType.current.mono, c.muted, maxLines = 1) }
    } }
    if (open) content()
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Attachments(row: JsonObject, submit: (ActionItem, Boolean) -> Unit) {
    val list = row["attachments"].objects.filter { it["label"].text != null }
    if (list.isEmpty()) return
    val c = LocalYaac.current
    DisableSelection { FlowRow(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        for (a in list) {
            val missing = a["missing"].flag()
            val action = ActionItem.of(a["action"])
            val label = a["label"].str.substringAfterLast('/') + if (missing) " · no longer on the target" else ""
            Row(Modifier.clip(RoundedCornerShape(5.dp)).background(c.raised).clickable(enabled = action?.enabled == true) { action?.let { submit(it, false) } }
                .padding(horizontal = 10.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(if (a["image"].flag()) YIcon.Image else YIcon.File, c.muted, size = 16.dp, strokeWidth = 1.6f)
                Spacer(Modifier.width(8.dp))
                T(label, LocalType.current.mono, if (missing) c.muted else c.text, maxLines = 1)
            }
        }
    } }
}

@Composable
private fun Questions(row: JsonObject, state: RowState) {
    val c = LocalYaac.current
    val type = LocalType.current
    for (q in row["questions"].objects) {
        val qid = q["id"].text ?: continue
        q["header"].text?.let { T(it, type.ui, c.muted, weight = FontWeight.SemiBold, modifier = Modifier.padding(top = 6.dp)) }
        T(q["text"].str, type.ui.copy(fontSize = 15.sp), modifier = Modifier.padding(top = 3.dp, bottom = 4.dp))
        val multi = q["multiSelect"].flag()
        val options = q["options"].list
        for (o in options) {
            val label = o["label"].text ?: o.text ?: continue
            val oid = o["id"].text ?: label
            val enabled = o["enabled"].flag(true)
            val checked = oid in state.selected[qid].orEmpty()
            Row(Modifier.fillMaxWidth().clickable(enabled = enabled) {
                val cur = state.selected[qid].orEmpty()
                state.selected[qid] = if (multi) (if (checked) cur - oid else cur + oid) else setOf(oid)
            }.heightIn(min = 40.dp).alpha(if (enabled) 1f else 0.5f), verticalAlignment = Alignment.CenterVertically) {
                val shape = RoundedCornerShape(if (multi) 4.dp else 10.dp)
                Box(Modifier.size(20.dp).clip(shape).border(1.5.dp, if (checked) c.text else c.muted, shape), contentAlignment = Alignment.Center) {
                    if (checked) Box(Modifier.size(10.dp).clip(RoundedCornerShape(if (multi) 2.dp else 5.dp)).background(c.text))
                }
                Spacer(Modifier.width(10.dp))
                Column { T(label); o["detail"].text?.let { T(it, type.small, c.muted) } }
            }
        }
        if (q["allowOther"].flag() || options.isEmpty()) {
            T(if (options.isEmpty()) "Your answer" else "Something else", type.small, c.muted, modifier = Modifier.padding(top = 4.dp))
            InputField(state.other[qid] ?: "", { state.other[qid] = it }, secret = q["secret"].flag())
        }
    }
}

@Composable
private fun Fields(row: JsonObject, state: RowState) {
    val c = LocalYaac.current
    val fields = row["fields"].objects
    if (fields.isEmpty()) return
    Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        for (f in fields) {
            val fid = f["id"].text ?: continue
            Column(Modifier.weight(1f)) {
                T(f["label"].str, LocalType.current.small, c.muted)
                InputField(state.fields[fid] ?: f["value"].scalar ?: "", { state.fields[fid] = it.filter { ch -> ch.isDigit() } }, number = true)
            }
        }
    }
}

@Composable
fun InputField(value: String, onChange: (String) -> Unit, secret: Boolean = false, number: Boolean = false) {
    val c = LocalYaac.current
    BasicTextField(
        value, onChange, singleLine = true,
        textStyle = LocalType.current.ui.copy(color = c.text, fontSize = 15.sp), cursorBrush = SolidColor(c.text),
        visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(keyboardType = when { secret -> KeyboardType.Password; number -> KeyboardType.Number; else -> KeyboardType.Text }),
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp).clip(RoundedCornerShape(6.dp)).background(c.input).padding(horizontal = 10.dp, vertical = 10.dp),
    )
}

/** The offered decisions. On a card they stack full width, the primary one filled. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Actions(row: JsonObject, submit: (ActionItem, Boolean) -> Unit, stacked: Boolean) {
    val actions = ActionItem.list(row["actions"])
    if (actions.isEmpty()) return
    var menu by remember { mutableStateOf<ActionItem?>(null) }
    val click: (ActionItem) -> Unit = { a -> if (a.children.isNotEmpty()) menu = a else submit(a, true) }
    // Buttons are not text: a selection across rows leaves them out (T05).
    DisableSelection { if (stacked) {
        Column(Modifier.fillMaxWidth().padding(top = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            for (a in actions) {
                if (a.primary) PrimaryButton(a.label, a.enabled, Modifier.fillMaxWidth()) { click(a) }
                else TextAction(a.label, a.enabled, modifier = Modifier.fillMaxWidth()) { click(a) }
                a.detail?.takeIf { !a.enabled }?.let { T(it, LocalType.current.small, LocalYaac.current.muted, modifier = Modifier.align(Alignment.CenterHorizontally)) }
            }
        }
    } else {
        FlowRow(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp), itemVerticalAlignment = Alignment.CenterVertically) {
            for (a in actions) {
                if (a.primary) PrimaryButton(a.label, a.enabled) { click(a) }
                else TextAction(a.label, a.enabled, style = LocalType.current.ui) { click(a) }
            }
        }
    } }
    menu?.let { m -> ActionSheet(m.label, m.children, { menu = null }) { submit(it, true) } }
}

/**
 * A `!command` the user ran (P346, WPF `TranscriptShell`): full width on the canvas, the command
 * in mono with its status badge and Stop, the output on a code surface folded to `shown`.
 */
@Composable
private fun ShellRow(row: JsonObject, state: RowState, submit: (ActionItem, Boolean) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val running = row["running"].flag()
    val ok = row["ok"].let { if (it is kotlinx.serialization.json.JsonPrimitive && !it.isString && it !is kotlinx.serialization.json.JsonNull) it.flag() else null }
    val badge = if (running) "running" else row["badge"].text
    Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            T(">_", type.mono, c["YaacShell"])
            Spacer(Modifier.width(8.dp))
            T(row["command"].str, type.mono.copy(fontSize = 14.sp), modifier = Modifier.weight(1f), maxLines = 2)
            badge?.let { T(it, type.small, when { running -> c.muted; ok == false -> c.failure; ok == true -> c.success; else -> c.muted }) }
            ActionItem.of(row["stop"])?.let { a -> TextAction(a.label, a.enabled, style = type.small) { submit(a, false) } }
        }
        row["cwd"].text?.let { T(it, type.small, c.muted) }
        val all = row["output"].str
        val folded = row["shown"].text
        val open = state.expanded["shell:open"] ?: false
        val text = if (folded != null && !open) folded else all
        if (text.isNotEmpty()) {
            Spacer(Modifier.height(4.dp))
            CodeBlock(all, text.trimEnd('\n'), null)
        }
        row["showAll"].text?.let { label -> TextAction(if (open) "Show less" else label, style = type.small) { state.expanded["shell:open"] = !open } }
        if (row["truncated"].flag()) T("The output was cut; the full log is on the target.", type.small, c.muted)
        row["status"].text?.let { T(it, type.small, c.tone(row["tone"].text, c.muted)) }
        Actions(row, submit, stacked = false)
    }
}

/** **Copy message** beside a message's time (T05, WPF's context-menu item): its whole text. */
@Composable
private fun CopyMessage(row: JsonObject) {
    val text = dev.yaac.android.ui.transcript.copyMessage(row) ?: return
    val clipboard = androidx.compose.ui.platform.LocalClipboardManager.current
    TextAction("Copy message", style = LocalType.current.small, color = LocalYaac.current.muted) { clipboard.setText(androidx.compose.ui.text.AnnotatedString(text)) }
}
