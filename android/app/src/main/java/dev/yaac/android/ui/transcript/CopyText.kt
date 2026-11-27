package dev.yaac.android.ui.transcript

import dev.yaac.android.core.get
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonObject

/**
 * A row's **Copy message** text, as `TranscriptRowView.CopyMessage`: the row's own `copyText`,
 * else for a message (user, agent, task) its text and its text-like parts, one per line.
 */
fun copyMessage(row: JsonObject): String? {
    row["copyText"].text?.let { return it }
    if (row["kind"].str !in setOf("user", "agent", "task")) return null
    val parts = row["parts"].objects.filter { it["kind"].str in setOf("text", "markdown", "code", "command", "pasted") }.map { it["text"].str }
    return (listOf(row["text"].str) + parts).filter { it.isNotEmpty() }.joinToString("\n").ifEmpty { null }
}

/** The lines of a code block as drawn (a final newline does not start a line). */
fun codeLines(text: String): Int = if (text.isEmpty()) 1 else text.removeSuffix("\n").count { it == '\n' } + 1

/** The line-number gutter for [text], right-aligned, one number per line (never copied). */
fun lineNumberGutter(text: String): String {
    val n = codeLines(text)
    val width = n.toString().length
    return (1..n).joinToString("\n") { it.toString().padStart(width) }
}
