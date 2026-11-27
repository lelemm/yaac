package dev.yaac.android.ui.pane

import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.longOrNull

/*
 * The side pane's tab strip (B02, E02) as data: each tab's icon, title, secondary text and
 * marks, from the `browser` view's `tabs` (terminal tabs are the controller's `describe`:
 * title, shell, cwd, failed, unread, bell, exitCode, error).
 */

enum class TabIcon { Chat, Files, File, Diff, Terminal, Browser }

enum class TabMark { None, Loading, Bell, Unseen, Exited, Failed }

data class TabInfo(
    val id: Long?,
    val kind: String,
    val icon: TabIcon,
    /** The strip's label (ellipsized there). */
    val title: String,
    /** Secondary text: a terminal's folder, a file's directory. */
    val detail: String?,
    val mark: TabMark,
    /** A file tab's git mark (M, A, D, R, ?, U, !). */
    val git: String?,
    val exitCode: Int?,
    val error: String?,
)

object Tabs {
    private fun JsonObject.s(k: String): String? = (this[k] as? JsonPrimitive)?.takeIf { it.isString }?.content?.takeIf { it.isNotEmpty() }
    private fun JsonObject.l(k: String): Long? = (this[k] as? JsonPrimitive)?.takeIf { !it.isString && it !is JsonNull }?.longOrNull
    private fun JsonObject.b(k: String): Boolean = (this[k] as? JsonPrimitive)?.takeIf { !it.isString }?.booleanOrNull ?: false

    /** The last path segment of a folder, either separator, trailing ones ignored. */
    fun folder(path: String?): String? = path?.trimEnd('/', '\\')?.split('/', '\\')?.lastOrNull()?.takeIf { it.isNotEmpty() }

    fun info(t: JsonObject): TabInfo {
        val kind = (t["kind"] as? JsonPrimitive)?.content ?: "browser"
        val exit = t.l("exitCode")?.toInt()
        return when (kind) {
            "terminal" -> {
                val bell = (t.l("bell") ?: 0L) > 0 || t.b("bell")
                val mark = when {
                    t.b("failed") -> TabMark.Failed
                    exit != null -> TabMark.Exited
                    bell -> TabMark.Bell
                    t.b("unread") -> TabMark.Unseen
                    else -> TabMark.None
                }
                // The user's name or the shell's title (OSC 0/2), else the shell's name.
                TabInfo(t.l("id"), kind, TabIcon.Terminal, t.s("title") ?: t.s("shell") ?: "Terminal", folder(t.s("cwd")), mark, null, exit, t.s("error"))
            }
            "files" -> TabInfo(t.l("id"), kind, TabIcon.Files, "Files", null, TabMark.None, null, null, t.s("error"))
            "file", "diff" -> {
                val path = t.s("path") ?: ""
                val name = path.substringAfterLast('/')
                val dir = path.substringBeforeLast('/', "").takeIf { it.isNotEmpty() }
                TabInfo(t.l("id"), kind, if (kind == "diff") TabIcon.Diff else TabIcon.File, name.ifEmpty { if (kind == "diff") "Diff" else "File" }, dir, TabMark.None, t.s("mark"), null, t.s("error"))
            }
            else -> TabInfo(t.l("id"), kind, TabIcon.Browser, t.s("title") ?: t.s("url") ?: "New tab", null,
                if (t.b("loading")) TabMark.Loading else TabMark.None, null, null, t.s("error"))
        }
    }

    /** What the terminal's status bar says (WPF TerminalPane.Bind): the exit, or the error, or nothing. */
    fun terminalStatus(info: TabInfo): String? = info.exitCode?.let { "Process exited with code $it" } ?: info.error

    /** Whether Restart is offered: after the shell exits or fails to start. */
    fun canRestart(info: TabInfo): Boolean = info.mark == TabMark.Exited || info.mark == TabMark.Failed

    /** A spoken description of a tab, for TalkBack (K04). */
    fun describe(info: TabInfo, selected: Boolean): String = buildString {
        append(when (info.icon) { TabIcon.Terminal -> "Terminal"; TabIcon.Diff -> "Changes"; TabIcon.File -> "File"; TabIcon.Files -> "Files"; TabIcon.Browser -> "Browser"; TabIcon.Chat -> "Chat" })
        if (info.icon != TabIcon.Files && info.icon != TabIcon.Chat) append(", ").append(info.title)
        info.detail?.let { append(", in ").append(it) }
        when (info.mark) {
            TabMark.Loading -> append(", loading"); TabMark.Bell -> append(", bell"); TabMark.Unseen -> append(", new output")
            TabMark.Exited -> append(", exited with code ${info.exitCode}"); TabMark.Failed -> append(", failed"); TabMark.None -> {}
        }
        if (selected) append(", selected")
    }
}

/** Inline rename (P343): Enter keeps it, Esc cancels, an empty name restores the shell's title. */
object Rename {
    /** The `data` of `terminal.rename`, or null when nothing changed (cancel). */
    fun data(original: String, edited: String): Map<String, Any?>? {
        val name = edited.trim()
        return if (name == original.trim()) null else mapOf("name" to name)
    }
}
