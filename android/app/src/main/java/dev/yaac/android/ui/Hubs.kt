package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.CallManager
import dev.yaac.android.core.HubNames
import dev.yaac.android.core.HubRuntime
import dev.yaac.android.core.HubState
import dev.yaac.android.core.YaacRuntime
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.text

/** Opens Settings → Hubs… from anywhere (the badge's menu); provided by the activity. */
val LocalOpenHubs = staticCompositionLocalOf<() -> Unit> { {} }

/** Each running hub with its mark, in list order (P404). */
private fun marked(): List<Pair<HubRuntime, String>> {
    val hubs = YaacRuntime.ordered
    return hubs.zip(HubNames.initials(hubs.map { it.name }))
}

/**
 * The selected hub's mark beside the YAAC mark (list) or the title (thread, [subtle]), shown only
 * with more than one hub. A tap opens the hubs: name, address, connection, a check on the one
 * shown; picking one switches the whole window to it.
 */
@Composable
fun HubBadge(subtle: Boolean = false, size: Dp = if (subtle) 22.dp else 24.dp) {
    if (YaacRuntime.running.size < 2) return
    val c = LocalYaac.current
    val type = LocalType.current
    val context = LocalContext.current
    val marks = marked()
    val selected = YaacRuntime.selected
    val mark = marks.firstOrNull { it.first === selected }?.second ?: "?"
    var open by remember { mutableStateOf(false) }
    val openHubs = LocalOpenHubs.current
    val shape = RoundedCornerShape(6.dp)
    Box {
        Box(Modifier.size(size).clip(shape)
            .then(if (subtle) Modifier.border(1.dp, c.rule, shape) else Modifier.background(c.accent))
            .clickable(onClickLabel = "Hubs: ${selected?.name ?: ""}") { open = true },
            contentAlignment = Alignment.Center) {
            T(mark, type.small.copy(fontSize = if (mark.length > 1) 10.sp else 12.sp), if (subtle) c.muted else c.onAccent, weight = FontWeight.Bold, maxLines = 1)
        }
        DropdownMenu(open, { open = false }, containerColor = c.raised, tonalElevation = 0.dp, shape = RoundedCornerShape(8.dp)) {
            for ((rt, m) in marks) {
                HubMenuRow(rt, m, rt === selected) {
                    open = false
                    YaacRuntime.select(context.applicationContext, rt.key)
                }
            }
            Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).height(1.dp).background(c.rule))
            Row(Modifier.fillMaxWidth().clickable { open = false; openHubs() }.heightIn(min = 44.dp).padding(horizontal = 16.dp),
                verticalAlignment = Alignment.CenterVertically) {
                T("Hubs…", type.ui.copy(fontSize = 15.sp))
            }
        }
    }
}

@Composable
private fun HubMenuRow(rt: HubRuntime, mark: String, checked: Boolean, onClick: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    Row(Modifier.widthIn(min = 260.dp).clickable(onClick = onClick).heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(22.dp).clip(RoundedCornerShape(6.dp)).border(1.dp, c.rule, RoundedCornerShape(6.dp)), contentAlignment = Alignment.Center) {
            T(mark, type.small.copy(fontSize = if (mark.length > 1) 10.sp else 12.sp), c.text, weight = FontWeight.Bold, maxLines = 1)
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            T(rt.name, type.ui.copy(fontSize = 15.sp), maxLines = 1)
            HubDetail(rt)
        }
        Box(Modifier.width(28.dp), contentAlignment = Alignment.CenterEnd) { if (checked) Icon(YIcon.Check, c.text, size = 18.dp) }
    }
}

/** "100.64.0.2:47380 · Connected", its state in the state's colour, and its call if it has one. */
@Composable
private fun HubDetail(rt: HubRuntime) {
    val c = LocalYaac.current
    val type = LocalType.current
    val state = rt.state
    val color = when (state) { HubState.Connected -> c.muted; HubState.Connecting -> c.muted; else -> c.failure }
    val needs = rt.store.view("shell")["needs"].int ?: 0
    // Two saved addresses that answer with one hubId are one hub (14-android.md#several-hubs-p403).
    val id = rt.store.view("shell")["hub"]["id"].text
    val twin = id?.let { YaacRuntime.ordered.firstOrNull { o -> o !== rt && o.store.view("shell")["hub"]["id"].text == it } }
    val parts = listOfNotNull(rt.hub?.address, state.label, twin?.let { "Same hub as ${it.name}" }, needs.takeIf { it > 0 }?.let { "$it need you" },
        "In a call".takeIf { CallManager.phase.live && CallManager.hubKey == rt.key })
    T(parts.joinToString(" · "), type.small, color, maxLines = 1)
}

/**
 * Settings → Hubs…: the saved hubs, each with its name, address and connection; tap to show it,
 * Remove to forget it (its attach ends, never the hub). Add a hub opens the connection screen.
 * In the demo it offers to leave the demo instead.
 */
@Composable
fun HubsScreen(onAdd: () -> Unit, onLeaveDemo: () -> Unit, onClose: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val context = LocalContext.current
    var removing by remember { mutableStateOf<HubRuntime?>(null) }
    androidx.activity.compose.BackHandler(onBack = onClose)
    Column(Modifier.fillMaxSize().background(c.canvas).statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(YIcon.Back, "Back", onClick = onClose)
            T("Hubs", type.title.copy(fontSize = 19.sp), modifier = Modifier.weight(1f))
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(c.rule))
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            val demo = YaacRuntime.running.values.firstOrNull { it.hub == null }
            if (demo != null) {
                Column(Modifier.padding(20.dp)) {
                    T(if (demo.connection.mode == "demo") "Running the demo" else "Attached to a development controller", type.ui.copy(fontSize = 16.sp))
                    T("Leave it to attach to your hubs.", type.small, c.muted)
                    Spacer(Modifier.height(8.dp))
                    TextAction("Leave the demo", onClick = onLeaveDemo)
                }
            } else {
                val marks = marked()
                for ((rt, m) in marks) {
                    Row(Modifier.fillMaxWidth().clickable { YaacRuntime.select(context.applicationContext, rt.key); onClose() }
                        .heightIn(min = 64.dp).padding(start = 20.dp, end = 8.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(28.dp).clip(RoundedCornerShape(6.dp)).background(c.accent), contentAlignment = Alignment.Center) {
                            T(m, type.small.copy(fontSize = if (m.length > 1) 11.sp else 13.sp), c.onAccent, weight = FontWeight.Bold, maxLines = 1)
                        }
                        Spacer(Modifier.width(14.dp))
                        Column(Modifier.weight(1f)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                T(rt.name, type.ui.copy(fontSize = 16.sp), maxLines = 1)
                                if (rt === YaacRuntime.selected) { Spacer(Modifier.width(6.dp)); Icon(YIcon.Check, c.text, size = 16.dp) }
                            }
                            HubDetail(rt)
                        }
                        if (rt.state != HubState.Connected) TextAction("Retry") { YaacRuntime.retry(context.applicationContext, rt.key) }
                        TextAction("Remove", color = c.failure) { removing = rt }
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            Row(Modifier.padding(horizontal = 20.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                PrimaryButton("Add a hub", onClick = onAdd)
            }
            Spacer(Modifier.height(12.dp))
            T("Every hub stays attached while YAAC runs, so each one's requests reach you. Each costs a connection and some memory and battery; the one not shown streams no open thread.",
                type.small, c.muted, modifier = Modifier.padding(horizontal = 20.dp))
        }
    }
    removing?.let { rt ->
        AlertDialog(onDismissRequest = { removing = null }, containerColor = c.raised, tonalElevation = 0.dp,
            title = { T("Remove ${rt.name}?", type.heading) },
            text = { T(buildString {
                append("This phone stops attaching to ${rt.hub?.address}. The hub and its threads are not touched.")
                if (CallManager.phase.live && CallManager.hubKey == rt.key) append(" The call on it ends.")
            }, type.ui, c.muted) },
            confirmButton = { TextAction("Remove", color = c.failure) {
                YaacRuntime.removeHub(context.applicationContext, rt.key); removing = null
                if (YaacRuntime.running.isEmpty()) onAdd()
            } },
            dismissButton = { TextAction("Cancel") { removing = null } })
    }
}
