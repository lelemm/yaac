package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
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
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** All rows the agents view knows, by id (groups carry them; the pill and card name them by id). */
private fun agentRows(view: JsonElement?): Map<String, JsonObject> =
    (view["groups"].objects.flatMap { it["rows"].objects } + view["siblings"].objects + listOfNotNull(view["root"].obj()))
        .associateBy { it["id"].str }

/** The 44 dp pill on the right edge (A07): a harness mark per related agent, clustered. */
@Composable
fun AgentsPill(view: JsonElement?, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    if (!view["visible"].flag()) return
    val pill = view["pill"].obj() ?: return
    val rows = agentRows(view)
    val ids = pill["rowIds"].list.mapNotNull { it.text }
    if (ids.isEmpty()) return
    val c = LocalYaac.current
    Column(
        modifier.width(44.dp).clip(RoundedCornerShape(10.dp)).background(c.raised).clickable(onClick = onOpen).padding(vertical = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        var lastGroup: String? = null
        val groupOf = view["groups"].objects.flatMap { g -> g["rows"].objects.map { it["id"].str to g["id"].str } }.toMap()
        for (id in ids.take(8)) {
            val r = rows[id] ?: continue
            val g = groupOf[id]
            if (lastGroup != null && g != lastGroup) Box(Modifier.padding(vertical = 4.dp).width(22.dp).height(1.dp).background(c.rule))
            lastGroup = g
            Box(Modifier.padding(vertical = 4.dp).alpha(if (r["finished"].flag()) 0.45f else 1f), contentAlignment = Alignment.Center) {
                r["logo"].text?.let { Logo(it, parseColor(r["logoColor"].text) ?: c.text, size = 20.dp) }
                if (r["attention"].text != null || r["mark"].text == "needsYou") T("!", LocalType.current.small, c.failure, weight = FontWeight.Bold, modifier = Modifier.align(Alignment.BottomEnd).padding(start = 18.dp))
                // Unseen completion: the cornflower ring (D114); a message in flight: its arrow.
                if (r["unseen"].flag()) Box(Modifier.size(28.dp).border(1.5.dp, c.unread, RoundedCornerShape(14.dp)))
                when (r["flow"].text) {
                    "sending" -> T("↑", LocalType.current.small, c.muted, modifier = Modifier.align(Alignment.TopEnd))
                    "receiving" -> T("↓", LocalType.current.small, c.muted, modifier = Modifier.align(Alignment.TopEnd))
                }
            }
        }
        val hidden = pill["hidden"].int ?: 0
        if (hidden > 0) T("+$hidden", LocalType.current.small, c.muted)
        val alive = pill["alive"].int ?: 0
        if (alive > 0) T("$alive alive", LocalType.current.small.copy(fontSize = 10.sp), c.muted)
    }
}

/** Related agents as a sheet (A08): budget, the groups, each row with its menu. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AgentsSheet(view: JsonElement?, send: (String, Any?) -> Unit, onDismiss: () -> Unit, onOpenThread: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val rows = agentRows(view)
    var menu by remember { mutableStateOf<JsonObject?>(null) }
    val unfolded = remember { androidx.compose.runtime.mutableStateMapOf<String, Boolean>() }
    val clipboard = LocalClipboardManager.current
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(), containerColor = c.raised, contentColor = c.text, tonalElevation = 0.dp) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(bottom = 16.dp)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                T("Related agents", type.title.copy(fontSize = 19.sp), modifier = Modifier.weight(1f))
                if (view["canMarkRead"].flag() || view["canMarkReadCard"].flag()) TextAction("Mark all as read") { send("agents.markRead", mapOf("floating" to false)) }
            }
            view["budget"].obj()?.let { b ->
                Column(Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) {
                    b["turns"].text?.let { T(it, type.ui, c.muted) }
                    b["time"].text?.let { T(it, type.ui, c.muted) }
                    ActionItem.of(b["action"])?.let { a -> PrimaryButton(a.label, a.enabled) { send("header.action", mapOf("id" to a.id, "threadId" to b["id"].str)) } }
                }
            }
            if (view["siblings"].objects.isNotEmpty()) Group("Siblings (${view["siblings"].objects.size})", view["siblings"].objects, send, { menu = it }, onOpenThread)
            for (g in view["groups"].objects) {
                // The docked order, with finished agents folded behind "› N finished" (A05).
                val docked = g["docked"].obj()
                val order = docked?.get("rowIds").list.mapNotNull { rows[it.str] }.ifEmpty { g["rows"].objects }
                val folded = docked?.get("foldIds").list.map { it.str }.toSet()
                val gid = g["id"].str
                val open = unfolded[gid] == true
                Group("${g["label"].str} (${g["count"].int ?: order.size})", order.filter { open || it["id"].str !in folded }, send, { menu = it }, onOpenThread)
                docked?.get("foldLabel").text?.takeIf { folded.isNotEmpty() }?.let { label ->
                    TextAction(if (open) "Hide finished" else "› $label", modifier = Modifier.padding(start = 8.dp)) { unfolded[gid] = !open }
                }
            }
            view["root"].obj()?.let { Group("Spawned by", listOf(it), send, { r -> menu = r }, onOpenThread) }
        }
    }
    menu?.let { r ->
        ActionSheet(r["title"].str, ActionItem.list(r["actions"]), { menu = null }) { a ->
            if (a.id == "copyId") clipboard.setText(AnnotatedString(r["id"].str)) else send("agents.action", mapOf("id" to r["id"].str, "action" to a.id))
        }
    }
}

@Composable
private fun Group(label: String, rows: List<JsonObject>, send: (String, Any?) -> Unit, onMenu: (JsonObject) -> Unit, onOpenThread: () -> Unit) {
    if (rows.isEmpty()) return
    val c = LocalYaac.current
    T(label, LocalType.current.ui.copy(fontSize = 15.sp), c.muted, modifier = Modifier.padding(start = 16.dp, top = 14.dp, bottom = 4.dp))
    for (r in rows) AgentRow(r, { if (r["canOpen"].flag(true)) { send("agents.open", mapOf("id" to r["id"].str)); onOpenThread() } }, { onMenu(r) }, send)
}

@Composable
private fun AgentRow(r: JsonObject, onOpen: () -> Unit, onMenu: () -> Unit, send: (String, Any?) -> Unit = { _, _ -> }) {
    val c = LocalYaac.current
    val type = LocalType.current
    val self = r["self"].flag()
    val mono = type.mono.copy(fontSize = 13.sp)
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 2.dp).clip(RoundedCornerShape(8.dp))
            .background(if (self) c.selected else Color.Transparent).tapAndHold(onOpen, onMenu)
            .padding(start = 8.dp, top = 8.dp, bottom = 8.dp).alpha(if (r["finished"].flag()) 0.6f else 1f),
    ) {
        Box(Modifier.width(36.dp).padding(top = 2.dp)) { r["logo"].text?.let { Logo(it, parseColor(r["logoColor"].text) ?: c.text, size = 24.dp) } }
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                T(r["title"].str, type.ui.copy(fontSize = 17.sp), maxLines = 1, modifier = Modifier.weight(1f))
                if (r["ultrafast"].flag()) { Spacer(Modifier.width(4.dp)); Icon(YIcon.Bolt, c["YaacUltrafast"], size = 14.dp) }
                else if (r["fast"].flag()) { Spacer(Modifier.width(4.dp)); Icon(YIcon.Bolt, c["YaacFast"], size = 14.dp) }
                if (self) { Spacer(Modifier.width(6.dp)); T("self", type.small, c.muted, modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(c.input).padding(horizontal = 6.dp, vertical = 1.dp)) }
                Spacer(Modifier.width(8.dp))
                val mark = r["mark"].text
                StateMark(mark, size = 14.dp)
                Spacer(Modifier.width(6.dp))
                T(r["markLabel"].str.replaceFirstChar { it.uppercase() }, type.ui, when (mark) { "needsYou", "needs", "failed" -> c.failure; "done" -> c.success; else -> c.text }, maxLines = 1)
            }
            r["git"].obj()?.takeIf { it["branch"].text != null }?.let { g ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(YIcon.Branch, c.muted, size = 13.dp); Spacer(Modifier.width(5.dp))
                    T(g["branch"].str, mono)
                    r["badge"].obj()?.let { b -> Spacer(Modifier.width(6.dp)); PrBadge(b, { action, n -> send("pr.action", mapOf("id" to r["id"].str, "action" to action, "number" to n)) }, {}, {}) }
                    g["changes"].text?.let { T("  $it", mono, c.warning) }
                    (g["ahead"].int ?: 0).takeIf { it > 0 }?.let { T("  ↑$it", mono, c.muted) }
                    (g["behind"].int ?: 0).takeIf { it > 0 }?.let { T("  ↓$it", mono, c.muted) }
                }
            }
            T(listOfNotNull(r["provider"].text, r["model"].text, r["target"].text).joinToString(" · "), mono, maxLines = 1)
            T(listOfNotNull(r["activity"].text ?: r["lastAction"].text, r["age"].text).joinToString(" · "), mono, c.muted, maxLines = 1)
        }
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).clickable(onClick = onMenu), contentAlignment = Alignment.Center) { Icon(YIcon.More, c.text, size = 20.dp, strokeWidth = 3f) }
    }
}
