package dev.yaac.android.ui.transcript

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject

/*
 * "Unread from here" (docs/07-ui.md#unread-from-here-on-android, P402): the phone remembers,
 * per thread, the last item the user had seen when they left it, and on return draws a divider
 * above the first item that came after it. All of it is on the device; nothing goes to the hub.
 */

/** One transcript row as the marker sees it: its id, its item (`actionId`), its turn, its kind. */
data class UnreadRow(val id: String, val itemId: String?, val turnId: String?, val kind: String?) {
    /** An item of the conversation: rows of a turn. Artifact cards and pinned copies are not. */
    val isItem: Boolean get() = turnId != null && !id.startsWith("~copy:")
}

/**
 * Where the user stopped: the row last seen, its item and turn (to find it again when the row
 * folds into a group), and the prompt at or before it (to jump to an older page through
 * `transcript.prompt` when the row is not loaded).
 */
data class SeenPoint(val rowId: String, val itemId: String?, val turnId: String?, val promptId: String?)

object Unread {
    const val LABEL = "New since you left"

    fun label(count: Int) = "$LABEL · $count"

    /**
     * The seen point for the row at [index] (the end while it is followed, else the row at the
     * bottom of the viewport): that row, or the nearest item row before it. Null when no item
     * row is at or before [index].
     */
    fun seenAt(rows: List<UnreadRow>, index: Int): SeenPoint? {
        if (rows.isEmpty()) return null
        val start = index.coerceIn(0, rows.size - 1)
        val at = (start downTo 0).firstOrNull { rows[it].isItem } ?: return null
        val row = rows[at]
        val prompt = (at downTo 0).firstOrNull { rows[it].kind == "user" && rows[it].itemId != null }?.let { rows[it].itemId }
        return SeenPoint(row.id, row.itemId, row.turnId, prompt)
    }

    /** The index of the row standing for [seen] in [rows], or -1 when it is not loaded. */
    fun locate(rows: List<UnreadRow>, seen: SeenPoint): Int {
        rows.indexOfLast { it.id == seen.rowId }.takeIf { it >= 0 }?.let { return it }
        if (seen.itemId != null) rows.indexOfLast { it.isItem && it.itemId == seen.itemId }.takeIf { it >= 0 }?.let { return it }
        val turn = seen.turnId ?: return -1
        // Folded into its turn's "Worked for…" line; else the turn's first row (what came after
        // it in the turn is then marked new: marking too much beats hiding something new).
        rows.indexOfFirst { it.id == "~worked:$turn" }.takeIf { it >= 0 }?.let { return it }
        return rows.indexOfFirst { it.isItem && it.turnId == turn }
    }

    sealed interface Outcome {
        /** Nothing came after the seen point (or it cannot be found): behave as before. */
        data object Nothing : Outcome
        /** Draw the divider above row [beforeId]; [count] item rows came after the seen point. */
        data class Divider(val beforeId: String, val count: Int) : Outcome
        /** The seen point is on an older page: jump there through its prompt. */
        data class Jump(val promptId: String) : Outcome
    }

    /**
     * Where the divider goes: above the first item row after [seen]. When [seen] is not in
     * [rows], a jump to its prompt if older history exists, the prompt is in the thread's prompt
     * index and no jump was made yet; otherwise nothing.
     */
    fun resolve(rows: List<UnreadRow>, seen: SeenPoint, prompts: Set<String>, hasOlder: Boolean, jumped: Boolean): Outcome {
        val at = locate(rows, seen)
        if (at < 0) {
            val prompt = seen.promptId
            return if (!jumped && hasOlder && prompt != null && prompt in prompts) Outcome.Jump(prompt) else Outcome.Nothing
        }
        val after = (at + 1 until rows.size).filter { rows[it].isItem }
        val first = after.firstOrNull() ?: return Outcome.Nothing
        return Outcome.Divider(rows[first].id, after.size)
    }
}

/**
 * One visit to a thread: resolves the divider once, then keeps it in place. The composable feeds
 * it the rows and acts on what [step] returns; every scroll is returned once, never again.
 */
class UnreadVisit(private val seen: SeenPoint?, enabled: Boolean) {
    enum class Phase { Resolving, Jumping, Settled }

    sealed interface Effect {
        /** Scroll so row [id] (the divider) is at the top of the viewport. */
        data class ScrollTo(val id: String) : Effect
        /** Send `transcript.prompt` for [promptId]. */
        data class Jump(val promptId: String) : Effect
        /** Return to the latest page (a jump found nothing). */
        data object Latest : Effect
    }

    var phase: Phase = if (enabled && seen != null) Phase.Resolving else Phase.Settled
        private set
    /** The row the divider is drawn above, or null. */
    var dividerBefore: String? = null
        private set
    var count: Int = 0
        private set
    private var jumpPrompt: String? = null
    private var ids: List<String> = emptyList()

    /** The anchor and the end-following wait while this is true; the seen point is not tracked. */
    val busy: Boolean get() = phase != Phase.Settled

    /** A jump of this visit's own is under way: the focus it causes is this visit's to apply. */
    val jumping: Boolean get() = phase == Phase.Jumping

    /**
     * Called whenever the rows change. [ready]: the page is loaded and not loading. [focused]:
     * the controller is focusing an item (that focus wins, the divider is drawn without a scroll).
     */
    fun step(rows: List<UnreadRow>, ready: Boolean, prompts: Set<String>, hasOlder: Boolean, focused: Boolean): Effect? {
        val newIds = rows.map { it.id }
        val old = ids
        ids = newIds
        when (phase) {
            Phase.Settled -> {
                // Kept above the same row; when that row folds away, above the one that took its place.
                val before = dividerBefore ?: return null
                if (before !in newIds) {
                    dividerBefore = relocate(old, newIds, before)
                    if (dividerBefore == null) count = 0
                }
                return null
            }
            Phase.Jumping -> {
                val prompt = jumpPrompt
                if (!ready || rows.none { it.itemId == prompt }) return null
            }
            Phase.Resolving -> if (!ready || rows.isEmpty()) return null
        }
        val point = seen ?: return settle(null)
        return when (val o = Unread.resolve(rows, point, prompts, hasOlder, jumped = phase == Phase.Jumping)) {
            is Unread.Outcome.Jump -> { phase = Phase.Jumping; jumpPrompt = o.promptId; Effect.Jump(o.promptId) }
            is Unread.Outcome.Divider -> {
                dividerBefore = o.beforeId
                count = o.count
                // A focus from the controller wins (but not the one this visit's own jump caused).
                settle(if (focused && jumpPrompt == null) null else Effect.ScrollTo(o.beforeId))
            }
            Unread.Outcome.Nothing -> settle(if (jumpPrompt != null) Effect.Latest else null)
        }
    }

    /** The jump never arrived: give up and return to the latest page. */
    fun giveUp(): Effect? {
        if (phase != Phase.Jumping) return null
        return settle(Effect.Latest)
    }

    /** Scrolled to the end past it, or a message was sent: the divider goes for this visit. */
    fun clear() {
        dividerBefore = null
        count = 0
        if (phase == Phase.Resolving) phase = Phase.Settled
    }

    private fun settle(effect: Effect?): Effect? {
        phase = Phase.Settled
        return effect
    }
}

/**
 * The seen points of the last [capacity] threads, most recently used last. Plain data with a
 * JSON form for SharedPreferences (core/LeftOff.kt).
 */
class SeenStore(private val capacity: Int = 200) {
    private val map = LinkedHashMap<String, SeenPoint>(16, 0.75f, true)

    val size: Int get() = map.size

    operator fun get(threadId: String): SeenPoint? = map[threadId]

    /** Records [point]; returns true when it changed. */
    fun put(threadId: String, point: SeenPoint): Boolean {
        val changed = map[threadId] != point
        map[threadId] = point
        while (map.size > capacity) map.remove(map.keys.first())
        return changed
    }

    fun encode(): String = JsonArray(map.map { (thread, p) ->
        JsonObject(buildMap {
            put("t", JsonPrimitive(thread))
            put("r", JsonPrimitive(p.rowId))
            p.itemId?.let { put("i", JsonPrimitive(it)) }
            p.turnId?.let { put("u", JsonPrimitive(it)) }
            p.promptId?.let { put("p", JsonPrimitive(it)) }
        })
    }).toString()

    companion object {
        /** Reads [encode]'s form; anything unreadable is skipped. */
        fun decode(text: String?, capacity: Int = 200): SeenStore {
            val store = SeenStore(capacity)
            if (text.isNullOrEmpty()) return store
            val entries = runCatching { dev.yaac.android.core.FrameJson.parseToJsonElement(text).jsonArray }.getOrNull() ?: return store
            for (e in entries) {
                val o = runCatching { e.jsonObject }.getOrNull() ?: continue
                fun s(k: String) = (o[k] as? JsonPrimitive)?.contentOrNull
                val thread = s("t") ?: continue
                val row = s("r") ?: continue
                store.put(thread, SeenPoint(row, s("i"), s("u"), s("p")))
            }
            return store
        }
    }
}
