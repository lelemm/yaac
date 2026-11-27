package dev.yaac.android.core

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * One saved hub (P404). [id] is the key it was saved under: the normalized address (`host:port`)
 * it was first added with. It names the running controller, the hub's notifications and its
 * phone-local keys (left-off marks), and does not change when the hub moves to another machine
 * (P412): [address] does. [slot] is a small number of its own, kept for as long as the hub is
 * saved: it names the hub's data directory and namespaces its notification ids. [legacy] marks
 * the hub an install from before P404 was attached to: its controller keeps the app's root data
 * directory, so nothing it stored is lost on the upgrade.
 */
data class Hub(val id: String, val label: String? = null, val slot: Int, val legacy: Boolean = false, val address: String = id)

/** `hub/moved` as the controller shows it (`shell.hub.movedTo`): where the hub now listens. */
data class HubMove(val hubId: String?, val address: String, val hostName: String?) {
    companion object {
        fun parse(e: kotlinx.serialization.json.JsonElement?): HubMove? {
            val address = e["address"].text?.trim()?.ifEmpty { null } ?: return null
            return HubMove(e["hubId"].text, address, e["hostName"].text)
        }
    }
}

/**
 * The saved hubs and the selected one: pure, so the app's rules (add, remove, select, store)
 * are tested off-device. The app keeps the encoded form in SharedPreferences.
 */
data class HubList(val hubs: List<Hub> = emptyList(), val selected: String? = null) {
    val current: Hub? get() = hubs.firstOrNull { it.id == selected } ?: hubs.firstOrNull()

    fun byId(id: String?): Hub? = hubs.firstOrNull { it.id == id }

    /** The saved hub that dials [address] (normalized), if any. */
    fun byAddress(address: String): Hub? = hubs.firstOrNull { it.address == address }

    /**
     * Adds [input] (normalized like the connection screen) and selects it; a hub already saved
     * at that address is selected, and takes [label] when one is given. Null when [input] is not
     * an address. A new hub's key is its address, unless a moved hub still holds that key.
     */
    fun add(input: String, label: String? = null): HubList? {
        val address = Connection.normalize(input) ?: return null
        val name = label?.trim()?.ifEmpty { null }
        val existing = byAddress(address)
        if (existing != null)
            return copy(hubs = hubs.map { if (it === existing && name != null) it.copy(label = name) else it }, selected = existing.id)
        val used = hubs.map { it.slot }.toHashSet()
        val slot = generateSequence(0) { it + 1 }.first { it !in used }
        val ids = hubs.map { it.id }.toHashSet()
        val id = if (address in ids) generateSequence(1) { it + 1 }.map { "$address#$it" }.first { it !in ids } else address
        return HubList(hubs + Hub(id, name, slot, address = address), id)
    }

    /** What [moved] does to the list: the new list, and the saved hubs it merged away. */
    data class Moved(val list: HubList, val merged: List<Hub>)

    /**
     * The hub saved as [id] moved to another machine (`hub/moved`, P412): it keeps its key, slot,
     * label and data directory (so its drafts and left-off marks stay) and dials [move]'s address
     * from now on. Another saved hub at that address, or answering with the same `hubId`
     * ([hubIdOf], from each controller's `shell.hub.id`), is the same hub: it is merged into this
     * one (its label is used when this one has none, the selection follows) and returned in
     * [Moved.merged] for the caller to stop. Null when nothing changes or the address is not one.
     */
    fun applyMoved(id: String, move: HubMove, hubIdOf: (Hub) -> String? = { null }): Moved? {
        val hub = byId(id) ?: return null
        val address = Connection.normalize(move.address) ?: return null
        val same = hubs.filter { it !== hub && (it.address == address || (move.hubId != null && hubIdOf(it) == move.hubId)) }
        if (address == hub.address && same.isEmpty()) return null
        val moved = hub.copy(address = address, label = hub.label ?: same.firstNotNullOfOrNull { it.label })
        val rest = hubs.filter { it !in same }.map { if (it === hub) moved else it }
        val sel = if (same.any { it.id == selected }) hub.id else selected
        return Moved(HubList(rest, sel), same)
    }

    /** Removes a hub; the selection moves to the next one (or the one before when it was last). */
    fun remove(id: String): HubList {
        val i = hubs.indexOfFirst { it.id == id }
        if (i < 0) return this
        val rest = hubs.filterIndexed { j, _ -> j != i }
        val sel = if (current?.id != id) current?.id else rest.getOrNull(i)?.id ?: rest.lastOrNull()?.id
        return HubList(rest, sel)
    }

    fun select(id: String): HubList = if (byId(id) == null) this else copy(selected = id)

    fun encode(): String = JsonObject(mapOf(
        "selected" to (selected?.let { JsonPrimitive(it) } ?: kotlinx.serialization.json.JsonNull),
        "hubs" to JsonArray(hubs.map { h ->
            JsonObject(buildMap {
                put("address", JsonPrimitive(h.address))
                if (h.id != h.address) put("key", JsonPrimitive(h.id))
                put("slot", JsonPrimitive(h.slot))
                h.label?.let { put("label", JsonPrimitive(it)) }
                if (h.legacy) put("legacy", JsonPrimitive(true))
            })
        }),
    )).toString()

    companion object {
        /** A stored list; an unreadable one reads as empty (the connection screen then asks). */
        fun decode(s: String?): HubList {
            if (s.isNullOrBlank()) return HubList()
            val o = runCatching { FrameJson.parseToJsonElement(s) as? JsonObject }.getOrNull() ?: return HubList()
            val ids = HashSet<String>()
            val addresses = HashSet<String>()
            val slots = HashSet<Int>()
            val hubs = o["hubs"].objects.mapNotNull { h ->
                val address = h["address"].text?.let(Connection::normalize) ?: return@mapNotNull null
                val id = h["key"].text ?: address
                val slot = h["slot"].int ?: return@mapNotNull null
                if (id in ids || address in addresses || slot in slots) return@mapNotNull null
                ids += id; addresses += address; slots += slot
                Hub(id, h["label"].text, slot, h["legacy"].flag(), address)
            }
            return HubList(hubs, o["selected"].text?.takeIf { id -> hubs.any { it.id == id } } ?: hubs.firstOrNull()?.id)
        }

        /** The single hub an install from before P404 saved becomes the first of the list. */
        fun migrate(lastHub: String?): HubList {
            val address = lastHub?.let(Connection::normalize) ?: return HubList()
            return HubList(listOf(Hub(address, null, 0, legacy = true)), address)
        }
    }
}

/** How a hub is named and marked in the badge, the hubs list and notifications. */
object HubNames {
    /**
     * The hub's name as the hub reports it (`shell.hub.name`), else the label given when it was
     * added, else the host: the first DNS label of a name, or the address itself.
     */
    fun display(hub: Hub, reported: String?): String =
        reported?.trim()?.ifEmpty { null } ?: hub.label ?: host(hub.address)

    fun host(address: String): String {
        val h = if (address.startsWith("[")) address.substringAfter('[').substringBefore(']')
                else address.substringBeforeLast(':')
        val ipv4 = h.all { it.isDigit() || it == '.' }
        return if (ipv4 || h.contains(':')) h else h.substringBefore('.')
    }

    /**
     * One mark per name, in order: its first letter; names whose first letters collide take two
     * (the first letters of its first two words, else its first two letters); marks that still
     * collide take a number instead of the second letter.
     */
    fun initials(names: List<String>): List<String> {
        val chars = names.map { n -> n.filter { it.isLetterOrDigit() }.uppercase().ifEmpty { "?" } }
        val one = chars.map { it.take(1) }
        val two = names.mapIndexed { i, n ->
            if (one.count { it == one[i] } < 2) return@mapIndexed one[i]
            val words = n.split(' ', '-', '_', '.').map { w -> w.filter { it.isLetterOrDigit() } }.filter { it.isNotEmpty() }
            if (words.size >= 2) (words[0].take(1) + words[1].take(1)).uppercase()
            else chars[i].take(2)
        }
        val counters = HashMap<String, Int>()
        return two.map { m ->
            if (two.count { it == m } < 2) m
            else m.take(1) + (counters.merge(m, 1, Int::plus)!!)
        }
    }
}

/** A hub's connection, as the badge's list says it. */
enum class HubState(val label: String) {
    Connecting("Connecting…"), Connected("Connected"), Unreachable("Not reachable"), Stopped("Stopped");

    companion object {
        /**
         * From its controller: [stopped] when it ended, [status] while the transport is still
         * coming up, then the frame's `shell.hub.reachable`.
         */
        fun of(stopped: String?, status: String?, haveShell: Boolean, reachable: Boolean): HubState = when {
            stopped != null -> Stopped
            !haveShell || status != null -> Connecting
            !reachable -> Unreachable
            else -> Connected
        }
    }
}

/**
 * Notification ids and PendingIntent request codes, namespaced per hub so two hubs' requests never
 * replace each other. Android tells PendingIntents apart by request code (not by extras), so the
 * codes are distinct as well.
 */
object HubNotifyIds {
    /** Slots above this share ids with lower ones; nobody saves 256 hubs. */
    const val MAX_SLOT = 255

    /** A request's notification: the hub in bits 22–29, the request's key hashed below them. */
    fun request(slot: Int, key: String): Int = ((slot.coerceIn(0, MAX_SLOT) + 1) shl 22) or (key.hashCode() and 0x3FFFFF)

    /** The "N requests need you" summary of one hub's group (below the request ids, above 40–42). */
    fun summary(slot: Int): Int = 100 + slot.coerceIn(0, MAX_SLOT)

    fun group(slot: Int): String = "yaac.requests.$slot"

    /**
     * A decision action (Allow once, Deny) of the notification [id]: its index in the top two bits,
     * which request ids leave clear. Three per notification at most.
     */
    fun decision(id: Int, index: Int): Int {
        require(index in 0..2)
        return ((index + 1) shl 30) xor id
    }
}

/** What the ongoing notification says for all hubs at once. */
object HubSummary {
    data class Entry(val name: String, val needs: Int, val tooltip: String?)

    /**
     * With one hub, that hub's own tray tooltip ("YAAC · 2 need you"); with more, the sum, and a
     * line per hub that needs you for the expanded text.
     */
    fun ongoing(entries: List<Entry>): Pair<String, List<String>> {
        if (entries.size <= 1) return (entries.firstOrNull()?.tooltip ?: "YAAC") to emptyList()
        val total = entries.sumOf { it.needs }
        val title = if (total > 0) "YAAC · $total need you" else "YAAC"
        val lines = entries.filter { it.needs > 0 }.map { "${it.name} · ${it.needs} need you" }
        return title to lines
    }
}
