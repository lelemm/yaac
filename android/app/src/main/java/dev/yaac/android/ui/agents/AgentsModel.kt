package dev.yaac.android.ui.agents

import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.list
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/*
 * The agents view (`crates/yaac-app/src/wpf/agents.rs`) read into plain values, with no Compose in
 * sight, so the ordering, folding, overflow and time-limit rules are unit-tested on the JVM. The
 * rules are those of `tools/wpf-preview/Views/AgentsView.xaml.cs` (BuildPill, ReadGroups,
 * OpenBudgetTimeMenu) and `AgentsItems.cs` (AgentsData).
 */

/** All rows the agents view knows, by id: the groups' rows first, then siblings not already in a group. */
fun agentRows(view: JsonElement?): Map<String, JsonObject> {
    val out = LinkedHashMap<String, JsonObject>()
    for (g in view["groups"].objects) for (r in g["rows"].objects) out.putIfAbsent(r["id"].str, r)
    for (r in view["siblings"].objects) out.putIfAbsent(r["id"].str, r)
    return out
}

/** The default heading and glyph of a cluster when the frame leaves them out (WPF ReadGroups). */
fun clusterDefaults(id: String): Pair<String, String> = when (id) {
    "spawnedBy" -> "Spawned by" to "↑"
    "workers" -> "Workers" to "⑂"
    "contacts" -> "In contact with" to "⇄"
    else -> "" to ""
}

/** A pill row's marks, as AgentsData reads them. */
data class AgentMarks(
    val needsYou: Boolean,
    val unseen: Boolean,
    val finished: Boolean,
    /** "↑" sending, "↓" receiving, "" when no message is in flight. */
    val flow: String,
    val transfer: String,
)

fun marksOf(r: JsonObject): AgentMarks {
    val (flow, transfer) = when (r["flow"].text) {
        "sending" -> "↑" to "sending"
        "receiving" -> "↓" to "receiving"
        else -> "" to ""
    }
    return AgentMarks(
        needsYou = r["attention"].flag() || r["attention"].text != null || r["mark"].text == "needsYou",
        unseen = r["unseen"].flag(),
        finished = r["finished"].flag(),
        flow = flow,
        transfer = transfer,
    )
}

/** One entry of the 44 dp pill, top to bottom. */
sealed interface PillEntry {
    /** The budget ran out: "Ⅱ" in the failure colour. */
    data object Paused : PillEntry
    /** The hairline between clusters (and before "more"). */
    data object Rule : PillEntry
    /** A cluster's direction glyph: ↑ spawned by, ⑂/↓ workers, ⇄/↔ in contact. */
    data class Cluster(val id: String, val glyph: String, val label: String) : PillEntry
    data class Agent(val id: String, val row: JsonObject, val marks: AgentMarks) : PillEntry
    /** "more (23)" with "4 alive"; [needsYou] when a hidden agent needs you. */
    data class More(val hidden: Int, val alive: Int, val needsYou: Boolean, val firstHiddenId: String?) : PillEntry {
        val lines: List<String> get() = listOfNotNull("more", "($hidden)", if (alive > 0) "$alive alive" else null)
    }
    /** Only a budget to show: "⇄" opens the sheet. */
    data object Related : PillEntry
}

/** The pill as WPF's BuildPill builds it: clusters in group order, each in the pill's own order. */
fun pillEntries(view: JsonElement?): List<PillEntry> {
    val out = mutableListOf<PillEntry>()
    val budget = view["budget"]
    val exhausted = budget["exhausted"].flag()
    val hasBudget = budget["turns"].text != null
    if (exhausted) out += PillEntry.Paused
    val pill = view["pill"]
    val groups = view["groups"].objects
    val all = groups.flatMap { it["rows"].objects }
    val explicit = pill["rowIds"] is kotlinx.serialization.json.JsonArray
    val order = (if (explicit) pill["rowIds"].list.map { it.str } else all.take(7).map { it["id"].str })
        .distinct().withIndex().associate { (i, id) -> id to i }
    var hasGroup = false
    val seen = HashSet<String>()
    for (g in groups) {
        val id = g["id"].str
        val visible = g["rows"].objects.filter { it["id"].str in order && seen.add(it["id"].str) }.sortedBy { order[it["id"].str] }
        if (visible.isEmpty()) continue
        if (hasGroup) out += PillEntry.Rule
        hasGroup = true
        val (heading, glyph) = clusterDefaults(id)
        val shownGlyph = g["glyph"].text ?: glyph
        if (shownGlyph.isNotEmpty()) out += PillEntry.Cluster(id, shownGlyph, g["label"].text ?: heading)
        for (r in visible) out += PillEntry.Agent(r["id"].str, r, marksOf(r))
    }
    val hidden = pill["hidden"].int ?: all.count { it["id"].str !in order }
    val alive = pill["alive"].int ?: 0
    val firstHidden = pill["firstHiddenId"].text ?: all.firstOrNull { it["id"].str !in order }?.get("id").text
    if (hidden > 0) {
        if (hasGroup) out += PillEntry.Rule
        out += PillEntry.More(hidden, alive, (pill["needsYou"].int ?: 0) > 0, firstHidden)
    }
    if (out.isEmpty() && hasBudget) out += PillEntry.Related
    return out
}

/** An entry of the sheet: a heading, a row, or a "› N finished" fold toggle. */
sealed interface SheetEntry {
    data class Heading(val key: String, val label: String) : SheetEntry
    data class Agent(val group: String, val id: String, val row: JsonObject) : SheetEntry
    data class Fold(val group: String, val label: String, val open: Boolean) : SheetEntry
    /** "root: <title>", opening the orchestration's root when it is known (**Open root**). */
    data class Root(val id: String?, val title: String, val canOpen: Boolean) : SheetEntry
    /** A line of text, e.g. "started by you". */
    data class Info(val text: String) : SheetEntry
}

/**
 * The sheet's rows, the docked order (A08b): *Siblings* (self marked), each cluster in its docked
 * order with inactive workers folded behind "› N finished" unless [unfolded] holds the cluster,
 * then *Spawned by* at the foot (WPF BuildDock and BuildDockParent).
 */
fun sheetEntries(view: JsonElement?, unfolded: Set<String>): List<SheetEntry> {
    val out = mutableListOf<SheetEntry>()
    val siblings = view["siblings"].objects
    if (siblings.isNotEmpty()) {
        out += SheetEntry.Heading("siblings", "Siblings (${siblings.size})")
        siblings.forEach { out += SheetEntry.Agent("siblings", it["id"].str, it) }
    }
    var parent: JsonObject? = null
    for (g in view["groups"].objects) {
        val gid = g["id"].str
        val rows = g["rows"].objects.associateBy { it["id"].str }
        if (gid == "spawnedBy") { parent = parent ?: g["rows"].objects.firstOrNull(); continue }
        if (rows.isEmpty()) continue
        val docked = g["docked"]
        val explicit = docked["rowIds"] is kotlinx.serialization.json.JsonArray
        val foldIds = docked["foldIds"].list.map { it.str }.filter { it in rows }.distinct()
        val order = (if (explicit) docked["rowIds"].list.map { it.str } else rows.keys.toList()).filter { it in rows && it !in foldIds }
        if (order.isEmpty() && foldIds.isEmpty()) continue
        val label = g["label"].text ?: clusterDefaults(gid).first
        val count = docked["count"].int ?: g["count"].int
        out += SheetEntry.Heading(gid, if (count != null) "$label ($count)" else label)
        order.forEach { out += SheetEntry.Agent(gid, it, rows.getValue(it)) }
        if (foldIds.isNotEmpty()) {
            val open = gid in unfolded
            out += SheetEntry.Fold(gid, docked["foldLabel"].text ?: "${foldIds.size} inactive", open)
            if (open) foldIds.forEach { out += SheetEntry.Agent(gid, it, rows.getValue(it)) }
        }
    }
    // *Spawned by* at the foot (BuildDockParent): the root, then the parent or "started by you".
    val root = view["root"].asObj()?.takeIf { it["title"].text != null }
    if (parent != null || root != null) {
        out += SheetEntry.Heading("spawnedBy", "Spawned by")
        root?.let { out += SheetEntry.Root(it["id"].text, "root: ${it["title"].str}", it["canOpen"].flag() && it["id"].text != null) }
        out += parent?.let { SheetEntry.Agent("spawnedBy", it["id"].str, it) } ?: SheetEntry.Info("started by you")
    }
    return out
}

/** The group a hidden agent sits in when it is folded, so opening at it unfolds that group first. */
fun foldedGroupOf(view: JsonElement?, id: String?): String? =
    id?.let { view["groups"].objects.firstOrNull { g -> g["docked"]["foldIds"].list.any { it.str == id } }?.get("id").text }

/** The second line of a row: provider · model · target. */
fun rowMeta(r: JsonObject): String = listOfNotNull(r["provider"].text, r["model"].text, r["target"].text).joinToString(" · ")

/** The last line: what it did last (or is doing) · its age. */
fun rowActivity(r: JsonObject): String = listOfNotNull(r["activity"].text ?: r["lastAction"].text, r["age"].text).joinToString(" · ")

/** The words after the state mark: the state, in-flight direction, attention, unread, read-only. */
fun rowInfo(r: JsonObject): String {
    val m = marksOf(r)
    return listOfNotNull(
        r["markLabel"].text, m.transfer.takeIf { it.isNotEmpty() }, r["attention"].text,
        "unread".takeIf { m.unseen }, "read-only".takeIf { r["readOnly"].flag() },
    ).joinToString(" · ")
}

/** The budget line (P352): both limits, which ran out, Continue. */
data class Budget(
    val root: String,
    val turns: String?,
    val time: String?,
    /** "turns", "time", "unknown" or null: the limit drawn in the failure colour. */
    val ranOut: String?,
    val exhausted: Boolean,
    val reached: String?,
    val timeMenu: TimeMenu?,
) {
    fun failing(limit: String) = ranOut == limit || ranOut == "unknown"
    companion object {
        fun of(e: JsonElement?): Budget? {
            val b = e.asObj() ?: return null
            return Budget(b["id"].str, b["turns"].text, b["time"].text, b["ranOut"].text, b["exhausted"].flag(), b["reached"].text, TimeMenu.of(b["timeMenu"]))
        }
    }
}

/** The Set / Extend time limit menu (A08d): presets, any number of hours, Remove, or why not. */
data class TimeMenu(val title: String, val unavailable: String?, val choices: List<Choice>, val remove: Choice?, val customDefault: Int) {
    data class Choice(val index: Int, val label: String, val detail: String?)

    companion object {
        const val MAX_HOURS = 10_000

        fun of(e: JsonElement?): TimeMenu? {
            val m = e.asObj() ?: return null
            val all = m["choices"].objects
            val choice = { o: JsonObject -> Choice(o["index"].int ?: 0, o["label"].str, o["detail"].text) }
            return TimeMenu(
                m["title"].str, m["unavailable"].text,
                all.filterNot { it["remove"].flag() }.map(choice),
                all.firstOrNull { it["remove"].flag() }?.let(choice),
                m["customDefault"].int ?: 2,
            )
        }

        /** Hours typed into the custom field: digits only, 1 to 10 000, or null (WPF's Submit). */
        fun hours(text: String): Int? {
            val t = text.trim()
            if (t.isEmpty() || !t.all { it in '0'..'9' }) return null
            return t.toIntOrNull()?.takeIf { it in 1..MAX_HOURS }
        }
    }
}

/** The `agents.timeLimit` payload for a preset or Remove. */
fun timeLimitChoice(root: String, choice: TimeMenu.Choice): Map<String, Any?> = mapOf("id" to root, "choice" to choice.index)

/** The `agents.timeLimit` payload for custom hours, or null when the text is not a valid count. */
fun timeLimitHours(root: String, text: String): Map<String, Any?>? = TimeMenu.hours(text)?.let { mapOf("id" to root, "hours" to it) }

/** Vertical bounds of a row in the sheet, in one shared coordinate space. */
data class RowBounds(val id: String, val top: Float, val bottom: Float)

/** The row under [y] while a held finger slides (A08c), or null between and outside rows. */
fun rowAt(y: Float, rows: Collection<RowBounds>): String? = rows.firstOrNull { y >= it.top && y < it.bottom }?.id

private fun JsonElement?.asObj(): JsonObject? = this as? JsonObject
