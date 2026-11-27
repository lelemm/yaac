package dev.yaac.android.core

/** Something shared into YAAC from another app (the Android share sheet, C09). */
data class Share(val text: String?, val paths: List<String>) {
    val isEmpty get() = text.isNullOrBlank() && paths.isEmpty()
}

/**
 * The commands that put a share into a composer, as WPF's drop and paste do: files through the
 * composer's `attachPaths` (the controller uploads them), text appended to the draft.
 */
fun shareCommands(key: String, draft: String, share: Share): List<Pair<String, Map<String, Any?>>> {
    val out = ArrayList<Pair<String, Map<String, Any?>>>()
    if (share.paths.isNotEmpty())
        out += "composer.action" to mapOf("key" to key, "id" to "attachPaths", "paths" to share.paths)
    share.text?.trim()?.takeIf { it.isNotEmpty() }?.let { t ->
        val text = if (draft.isBlank()) t else draft.trimEnd() + "\n\n" + t
        out += "composer.edit" to mapOf("key" to key, "text" to text, "caret" to text.length)
    }
    return out
}
