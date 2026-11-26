package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.layout.layout
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.list
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * The strip (A04, D103): the desktop's collapsed sidebar for a narrow window, used at medium
 * width. The mark, New, Back and Forward; per project a vertical name; per thread its harness
 * logo with the state badge (tap opens, touch and hold shows the row card); the rate-limit bars
 * and Settings at the foot.
 */
@Composable
fun StripScreen(sidebar: JsonElement?, shell: JsonElement?, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    var card by remember { mutableStateOf<JsonObject?>(null) }
    Column(Modifier.width(64.dp).fillMaxHeight().background(c.panel).statusBarsPadding().navigationBarsPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
        val needs = (shell["needs"] as? JsonPrimitive)?.content?.toIntOrNull() ?: 0
        if (needs > 0) Box(Modifier.clip(RoundedCornerShape(6.dp)).clickable { send("shell.needs", null) }.padding(6.dp)) { T("!$needs", type.ui, c.failure, weight = FontWeight.Bold) }
        else T("Y", type.title.copy(fontSize = 20.sp), weight = FontWeight.Bold, modifier = Modifier.padding(6.dp))
        IconButton(YIcon.Plus, "New") { send("sidebar.new", null) }
        Row {
            Box(Modifier.size(30.dp).clip(RoundedCornerShape(6.dp)).clickable(enabled = shell["canBack"].flag()) { send("shell.back", null) }.alpha(if (shell["canBack"].flag()) 1f else 0.35f), contentAlignment = Alignment.Center) { Icon(YIcon.Back, c.text, size = 16.dp) }
            Box(Modifier.size(30.dp).clip(RoundedCornerShape(6.dp)).clickable(enabled = shell["canForward"].flag()) { send("shell.forward", null) }.alpha(if (shell["canForward"].flag()) 1f else 0.35f), contentAlignment = Alignment.Center) { Icon(YIcon.Back, c.text, size = 16.dp, modifier = Modifier.rotate(180f)) }
        }
        Box(Modifier.width(32.dp).height(1.dp).background(c.rule))
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
            for (section in sidebar["sections"].objects) {
                val header = section["header"].obj()
                val rows = if (section["collapsed"].flag()) emptyList() else section["rows"].objects
                item("h:" + section["id"].str) {
                    // The project name, written up the strip.
                    T(header?.get("label").str.take(14), type.small.copy(fontStyle = if (header?.get("temporary").flag()) FontStyle.Italic else FontStyle.Normal), c.muted,
                        maxLines = 1, modifier = Modifier.padding(vertical = 6.dp).upward())
                }
                items(rows, key = { "r:" + it["id"].str }) { row -> StripIcon(row, onOpen = { send("sidebar.open", mapOf("id" to row["id"].str)) }, onHold = { card = row }) }
            }
        }
        // The rate-limit bars, one per group, as small vertical meters.
        Row(Modifier.padding(vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            for (group in sidebar["usage"].objects) {
                val seg = group["bars"].objects.maxByOrNull { b -> b["segments"].list.sumOf { (it as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0 } }?.get("segments").list
                    .map { (it as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0 }
                Column(verticalArrangement = Arrangement.spacedBy(2.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    group["logo"].text?.let { Logo(it, parseColor(group["logoColor"].text) ?: c.text, size = 12.dp) }
                    for (s in seg.reversed()) Box(Modifier.width(10.dp).height(5.dp).clip(RoundedCornerShape(1.dp)).background(if (s > 0) c.accent.copy(alpha = 0.4f + 0.6f * s.toFloat()) else c.rule))
                }
            }
        }
        IconButton(YIcon.Settings, "Settings", size = 18.dp, strokeWidth = 1.7f) { send("shell.settings", null) }
    }
    card?.let { row ->
        ActionSheet(row["title"].str, listOf(ActionItem("open", "Open", true, listOfNotNull(row["status"].text, row["right"].text, row["line2"]["branch"].text).joinToString(" · "), false, null, emptyList(), false)) +
            listOf(ActionItem("", "", false, null, false, null, emptyList(), false)) + ActionItem.list(row["actions"]), { card = null }) { a ->
            if (a.id == "open") send("sidebar.open", mapOf("id" to row["id"].str))
            else if (a.id != "copyId") send("sidebar.action", mapOf("action" to a.id, "ids" to listOf(row["id"].str)))
        }
    }
}

@Composable
private fun StripIcon(row: JsonObject, onOpen: () -> Unit, onHold: () -> Unit) {
    val c = LocalYaac.current
    val logo = row["logo"].text ?: row["line2"]["logo"].text
    val tint = parseColor(row["logoColor"].text ?: row["line2"]["logoColor"].text) ?: c.text
    val tone = row["tone"].text
    val shape = RoundedCornerShape(8.dp)
    var m = Modifier.padding(vertical = 3.dp).size(44.dp).clip(shape)
    if (row["selected"].flag()) m = m.background(c.selected)
    if (row["unseen"].flag()) m = m.border(1.5.dp, c.unread, shape)
    Box(m.tapAndHold(onOpen, onHold).alpha(if (row["muted"].flag() || tone == "offline") 0.5f else 1f), contentAlignment = Alignment.Center) {
        if (logo != null) Logo(logo, tint, size = 22.dp) else T(row["title"].str.take(1), LocalType.current.ui)
        // The state badge at the corner: the same marks as the list.
        if (tone != null && tone != "idle") Box(Modifier.align(Alignment.BottomEnd).padding(2.dp).size(16.dp).clip(RoundedCornerShape(8.dp)).background(c.panel), contentAlignment = Alignment.Center) {
            StateMark(tone, size = 11.dp, fast = row["fast"].flag(), ultrafast = row["ultrafast"].flag())
        }
    }
}

/** Lays text out rotated a quarter turn counter-clockwise, taking the rotated size. */
private fun Modifier.upward(): Modifier = this
    .layout { measurable, constraints ->
        val placeable = measurable.measure(androidx.compose.ui.unit.Constraints(maxWidth = if (constraints.hasBoundedHeight) constraints.maxHeight else androidx.compose.ui.unit.Constraints.Infinity))
        layout(placeable.height, placeable.width) { placeable.place(-(placeable.width - placeable.height) / 2, (placeable.width - placeable.height) / 2) }
    }
    .rotate(-90f)
