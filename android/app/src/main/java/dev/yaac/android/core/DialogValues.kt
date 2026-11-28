package dev.yaac.android.core

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/**
 * The pure part of an external dialog (docs/16-external-dialogs.md) on the phone: the values a
 * layout starts with, and what is sent with a button. The controller validates and answers; the
 * phone only keeps what the user entered (in memory, secret values too) and sends it with the
 * button pressed, as WPF's `DialogLayoutView` does.
 */
object DialogValues {
    /** Input node types; anything else carries no value. */
    val INPUTS = setOf("textInput", "number", "checkbox", "toggle", "select", "radio", "multiSelect", "date", "slider")

    /** Every node of a layout, parents first (bounded like the daemon's limit). */
    fun nodes(layout: JsonElement?): List<JsonObject> {
        val out = ArrayList<JsonObject>()
        fun walk(n: JsonElement?, depth: Int) {
            val o = n as? JsonObject ?: return
            if (depth > 16 || out.size > 600) return
            out += o
            for (child in o["children"].list) walk(child, depth + 1)
        }
        walk(layout, 0)
        return out
    }

    /** Each input's starting value: its default, never one for a secret input. Number, slider
     * and date values are text, as typed. */
    fun initial(layout: JsonElement?): Map<String, Any?> {
        val out = LinkedHashMap<String, Any?>()
        for (n in nodes(layout)) {
            val type = n["type"].str
            val id = n["id"].text ?: continue
            if (type !in INPUTS) continue
            out[id] = when (type) {
                "textInput" -> if (n["secret"].flag()) "" else n["default"].str
                "number", "date" -> n["default"].scalar ?: ""
                "slider" -> n["default"].scalar ?: n["min"].scalar ?: "0"
                "checkbox", "toggle" -> n["default"].flag()
                "select", "radio" -> n["default"].text
                "multiSelect" -> n["default"].list.mapNotNull { it.text }
                else -> null
            }
        }
        return out
    }

    /** The `transcript.action` / `inbox.action` values for a press: inputs of this layout only. */
    fun payload(layout: JsonElement?, values: Map<String, Any?>): Map<String, Any?> {
        val ids = nodes(layout).filter { it["type"].str in INPUTS }.mapNotNull { it["id"].text }.toSet()
        return values.filterKeys { it in ids }
    }

    /** Whether a frame's inbox location names a target inbox rather than a thread. */
    fun isInbox(threadAddress: String?): Boolean = threadAddress?.substringAfter('/', "") == "~inbox"
}
