package dev.yaac.android.ui.pane

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.longOrNull

/*
 * File and diff tabs' pure logic (A23): the assembled `filesDocument`
 * (crates/yaac-app/src/wpf/files.rs `WireDocument`) laid out as WPF's FileDocument.Layout does,
 * with unfolded regions, the selection → `files.add` data (`excerpt`/`diff_selection`), and
 * change navigation (`files.change` / `filesChange`).
 */

/** A syntax span or an intraline change, in UTF-16 columns of its line. */
data class CodeSpan(val start: Int, val end: Int, val scope: String)

data class SourceLine(val text: String, val spans: List<CodeSpan>, val words: List<IntRange>?)

/** A `CodeSource`: text split in lines, with styles by line index. */
class Source(val lines: List<SourceLine>) {
    operator fun get(i: Int): SourceLine? = lines.getOrNull(i)

    companion object {
        private fun JsonElement?.int(): Int? = (this as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull?.toInt()

        fun read(src: JsonObject?): Source {
            val text = (src?.get("text") as? JsonPrimitive)?.content ?: ""
            val styles = src?.get("styles") as? JsonObject
            return Source(text.split('\n').mapIndexed { i, line ->
                val style = styles?.get(i.toString()) as? JsonObject
                val spans = (style?.get("spans") as? JsonArray).orEmpty().mapNotNull { s ->
                    val o = s as? JsonObject ?: return@mapNotNull null
                    val a = o["start"].int() ?: return@mapNotNull null
                    val b = o["end"].int() ?: return@mapNotNull null
                    if (a < 0 || b < a || b > line.length) null else CodeSpan(a, b, (o["scope"] as? JsonPrimitive)?.content ?: "")
                }
                val words = (style?.get("words") as? JsonArray)?.mapNotNull { w ->
                    val pair = w as? JsonArray ?: return@mapNotNull null
                    val a = pair.getOrNull(0).int() ?: return@mapNotNull null
                    val b = pair.getOrNull(1).int() ?: return@mapNotNull null
                    if (a < 0 || b < a || b > line.length) null else a until b
                }
                SourceLine(line, spans, words)
            })
        }
    }
}

/** A diff row `[old?, new?, count]`: count > 0 is a fold of unchanged lines. */
data class DiffRowWire(val old: Int?, val new: Int?, val count: Int)

enum class LineKind { Context, Added, Removed, Fold, File }

/**
 * One displayed line. [row]/[offset] address it on the wire (a fold's lines share its row);
 * [old]/[new] are 0-based source lines (-1 when absent); [foldStart] marks a fold's first line
 * once unfolded (it carries the Fold again button).
 */
data class DocLine(
    val row: Int,
    val offset: Int,
    val old: Int,
    val new: Int,
    val kind: LineKind,
    val text: String,
    val source: SourceLine?,
    val foldCount: Int = 0,
    val foldStart: Boolean = false,
)

/** In side-by-side mode, the old and new halves of one display row. */
data class SidePair(val left: DocLine, val right: DocLine)

class DocumentModel private constructor(
    val file: Source?,
    val old: Source?,
    val new: Source?,
    val rows: List<DiffRowWire>,
    val sideBySide: Boolean,
) {
    val isDiff get() = file == null

    /** The rows that are folds, for "Unfold all". */
    val foldRows: List<Int> get() = rows.indices.filter { rows[it].count > 0 }

    /** The unified (or file) lines, folds closed unless their row is in [expanded]. */
    fun layout(expanded: Set<Int>): List<DocLine> {
        file?.let { f -> return f.lines.mapIndexed { i, l -> DocLine(i, 0, i, i, LineKind.File, l.text, l) } }
        val out = ArrayList<DocLine>()
        for ((i, r) in rows.withIndex()) {
            if (r.count > 0 && i !in expanded) {
                out += DocLine(i, 0, -1, -1, LineKind.Fold, "⋯ ${r.count} unchanged lines", null, r.count); continue
            }
            for (offset in 0 until maxOf(r.count, 1)) {
                val a = r.old?.plus(offset) ?: -1
                val b = r.new?.plus(offset) ?: -1
                val fold = r.count > 0 && offset == 0
                out += when {
                    a >= 0 && b >= 0 -> DocLine(i, offset, a, b, LineKind.Context, new?.get(b)?.text ?: "", new?.get(b), foldStart = fold)
                    a >= 0 -> DocLine(i, offset, a, -1, LineKind.Removed, old?.get(a)?.text ?: "", old?.get(a), foldStart = fold)
                    else -> DocLine(i, offset, -1, b, LineKind.Added, new?.get(b)?.text ?: "", new?.get(b), foldStart = fold)
                }
            }
        }
        return out
    }

    /** Side by side (the document must have been sent `sideBySide`): paired old/new halves. */
    fun pairs(expanded: Set<Int>): List<SidePair> {
        val out = ArrayList<SidePair>()
        for ((i, r) in rows.withIndex()) {
            if (r.count > 0 && i !in expanded) {
                val m = DocLine(i, 0, -1, -1, LineKind.Fold, "⋯ ${r.count} unchanged lines", null, r.count)
                out += SidePair(m, m); continue
            }
            for (offset in 0 until maxOf(r.count, 1)) {
                val a = r.old?.plus(offset) ?: -1
                val b = r.new?.plus(offset) ?: -1
                val fold = r.count > 0 && offset == 0
                val os = if (a >= 0) old?.get(a) else null
                val ns = if (b >= 0) new?.get(b) else null
                // A pair with intraline words is a replacement: both halves are marked.
                val changed = a < 0 || b < 0 || os?.words != null || ns?.words != null
                out += SidePair(
                    DocLine(i, offset, a, -1, if (a < 0) LineKind.Context else if (changed) LineKind.Removed else LineKind.Context, os?.text ?: "", os, foldStart = fold),
                    DocLine(i, offset, -1, b, if (b < 0) LineKind.Context else if (changed) LineKind.Added else LineKind.Context, ns?.text ?: "", ns, foldStart = fold),
                )
            }
        }
        return out
    }

    /** The display index of the first line of wire row [row] (where `filesChange` scrolls). */
    fun indexOfRow(lines: List<DocLine>, row: Int): Int = lines.indexOfFirst { it.row >= row }.let { if (it < 0) (lines.size - 1).coerceAtLeast(0) else it }

    companion object {
        private fun JsonElement?.int(): Int? = (this as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull?.toInt()

        fun read(doc: JsonObject): DocumentModel {
            val file = (doc["file"] as? JsonObject)?.let { Source.read(it) }
            val rows = (doc["rows"] as? JsonArray).orEmpty().mapNotNull { r ->
                val a = r as? JsonArray ?: return@mapNotNull null
                DiffRowWire(a.getOrNull(0).int(), a.getOrNull(1).int(), a.getOrNull(2).int() ?: 0)
            }
            val side = (doc["sideBySide"] as? JsonPrimitive)?.booleanOrNull ?: false
            return DocumentModel(file, if (file == null) Source.read(doc["old"] as? JsonObject) else null,
                if (file == null) Source.read(doc["new"] as? JsonObject) else null, rows, side)
        }
    }
}

/** A selection of displayed lines, with optional UTF-16 columns on its ends. */
data class LineSelection(val first: Int, val last: Int, val firstColumn: Int? = null, val lastColumn: Int? = null) {
    val count get() = last - first + 1

    /** Tap-to-extend: a tap starts a selection, a second tap extends it, a tap after that starts again. */
    fun tap(i: Int): LineSelection = if (first != last) LineSelection(i, i) else LineSelection(minOf(first, i), maxOf(first, i))

    companion object {
        fun start(i: Int) = LineSelection(i, i)
    }
}

object Excerpts {
    /**
     * The `data` of `files.add` for a selection of [lines] (display indices). Files send source
     * lines; diffs send wire rows with offsets, and the folds that stay closed in `folded`
     * (the controller skips them). [side] is "unified", "old" or "new". Null when the
     * selection holds nothing addable (only closed folds).
     */
    fun addData(document: Long, model: DocumentModel, lines: List<DocLine>, sel: LineSelection, side: String = "unified"): Map<String, Any?>? {
        val a = sel.first.coerceIn(0, lines.size - 1)
        val b = sel.last.coerceIn(a, lines.size - 1)
        if (!model.isDiff) {
            return linkedMapOf<String, Any?>("document" to document, "first" to lines[a].row, "last" to lines[b].row).apply {
                sel.firstColumn?.let { put("firstColumn", it) }; sel.lastColumn?.let { put("lastColumn", it) }
            }
        }
        // Closed folds at the ends add nothing: trim them so the offsets address real lines.
        var lo = a; var hi = b
        while (lo <= hi && lines[lo].kind == LineKind.Fold) lo++
        while (hi >= lo && lines[hi].kind == LineKind.Fold) hi--
        if (lo > hi) return null
        val folded = (lo..hi).filter { lines[it].kind == LineKind.Fold }.map { lines[it].row }.distinct()
        return linkedMapOf<String, Any?>(
            "document" to document, "side" to side,
            "first" to lines[lo].row, "firstOffset" to lines[lo].offset,
            "last" to lines[hi].row, "lastOffset" to lines[hi].offset,
            "folded" to folded,
        ).apply {
            if (lo == a) sel.firstColumn?.let { put("firstColumn", it) }
            if (hi == b) sel.lastColumn?.let { put("lastColumn", it) }
        }
    }

    /** The selected text, for Copy. */
    fun text(lines: List<DocLine>, sel: LineSelection): String =
        (sel.first..sel.last).mapNotNull { lines.getOrNull(it) }.filter { it.kind != LineKind.Fold }.joinToString("\n") { it.text }
}

object ChangeNav {
    /** The `data` of `files.change`: from the wire row of the line at the top (or the last change reached). */
    fun data(document: Long, lines: List<DocLine>, top: Int, forward: Boolean): Map<String, Any?> =
        mapOf("document" to document, "row" to lines.getOrNull(top)?.row, "forward" to forward)

    /** "Change 2 of 5", or "5 changes" before any navigation. */
    fun label(ordinal: Int?, count: Int): String = when {
        count <= 0 -> "No changes"
        ordinal != null && ordinal in 1..count -> "Change $ordinal of $count"
        count == 1 -> "1 change"
        else -> "$count changes"
    }
}
