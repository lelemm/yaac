package dev.yaac.android.ui.transcript

/** One run of the "From <agent> on <target>" header: [name] is the bold one, the rest are muted. */
data class HeaderRun(val text: String, val name: Boolean)

/**
 * The runs of a message bubble's source line. They are drawn as one flowing text so the line wraps
 * at spaces, never letter by letter in a narrow column left over by a long title. A blank [machine]
 * leaves out the "on" part.
 */
fun peerHeaderRuns(heading: String?, label: String, machine: String?): List<HeaderRun> = buildList {
    add(HeaderRun((heading?.takeIf { it.isNotBlank() } ?: "From") + " ", false))
    add(HeaderRun(label, true))
    machine?.takeIf { it.isNotBlank() }?.let { add(HeaderRun(" on $it", false)) }
}
