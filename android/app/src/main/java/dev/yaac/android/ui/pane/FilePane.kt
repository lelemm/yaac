package dev.yaac.android.ui.pane

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import dev.yaac.android.ui.LocalType
import dev.yaac.android.ui.LocalYaac
import dev.yaac.android.ui.SidePaneState
import dev.yaac.android.ui.T
import dev.yaac.android.ui.TextAction
import dev.yaac.android.ui.YaacColors
import dev.yaac.android.ui.gitColor
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

private fun scopeColor(scope: String, c: YaacColors): Color? = when {
    scope.contains("comment") -> c["YaacCodeComment"]
    scope.contains("string") -> c["YaacCodeString"]
    scope.contains("keyword") || scope.contains("storage") -> c["YaacCodeKeyword"]
    scope.contains("number") || scope.contains("constant") -> c["YaacCodeNumber"]
    scope.contains("type") || scope.contains("entity") -> c["YaacCodeType"]
    else -> null
}

/** A line's text coloured by its syntax spans, with its intraline changes ([words]) marked. */
private fun styled(line: DocLine, c: YaacColors): AnnotatedString {
    val src = line.source
    if (line.kind == LineKind.Fold) return buildAnnotatedString { pushStyle(SpanStyle(color = c.muted)); append(line.text); pop() }
    if (src == null || (src.spans.isEmpty() && src.words == null)) return AnnotatedString(line.text)
    return buildAnnotatedString {
        append(line.text)
        for (s in src.spans) scopeColor(s.scope, c)?.let { if (s.end <= line.text.length) addStyle(SpanStyle(color = it), s.start, s.end) }
        val word = when (line.kind) { LineKind.Added -> c.success.copy(alpha = .32f); LineKind.Removed -> c.failure.copy(alpha = .32f); else -> null }
        if (word != null) for (w in src.words.orEmpty()) if (w.last < line.text.length) addStyle(SpanStyle(background = word), w.first, w.last + 1)
    }
}

private fun lineBackground(kind: LineKind, c: YaacColors): Color = when (kind) {
    LineKind.Added -> c["YaacDiffAdded"]
    LineKind.Removed -> c["YaacDiffRemoved"]
    else -> Color.Transparent
}

private fun sign(kind: LineKind) = when (kind) { LineKind.Added -> "+"; LineKind.Removed -> "−"; else -> " " }

/**
 * A file or a diff (A23, D03–D05), from the assembled `filesDocument`: line numbers, syntax
 * colours, selectable text (touch and hold), Reload; for diffs unified or side by side, intraline
 * marks, folds that open on a tap, the change count with Previous/Next; and a line range (tap a
 * line, tap another to extend) that Copy or Add to composer takes.
 */
@Composable
fun FilePane(view: JsonObject?, tab: JsonObject, threadId: String, tabId: Long?, side: SidePaneState, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val clipboard = LocalClipboardManager.current
    val docId = (view?.get("document") as? JsonPrimitive)?.content?.toLongOrNull()
    val doc = docId?.let { side.documents[it] }
    val diff = tab["kind"].str == "diff"
    fun cmd(name: String, data: Map<String, Any?>, extra: Map<String, Any?> = emptyMap()) =
        send("files.$name", mapOf("thread" to threadId, "tab" to tabId, "data" to data) + extra)
    val model = remember(doc) { doc?.let { DocumentModel.read(it) } }
    var expanded by remember(docId) { mutableStateOf<Set<Int>>(emptySet()) }
    var sel by remember(docId) { mutableStateOf<LineSelection?>(null) }
    // Side by side: which half the selection is on.
    var selSide by remember(docId) { mutableStateOf("old") }
    val sideBySide = diff && model?.sideBySide == true
    val lines = remember(model, expanded) { model?.layout(expanded).orEmpty() }
    val pairs = remember(model, expanded) { if (sideBySide) model?.pairs(expanded).orEmpty() else emptyList() }
    val rowCount = if (sideBySide) pairs.size else lines.size
    val listState = rememberLazyListState()
    val ordinal = side.change?.takeIf { it.id == docId }?.ordinal

    // The controller moved to a change: scroll to its first line.
    val moved = side.change
    LaunchedEffect(moved, model) {
        if (moved != null && moved.id == docId && model != null) {
            val target = if (sideBySide) pairs.indexOfFirst { it.left.row >= moved.row }.coerceAtLeast(0) else model.indexOfRow(lines, moved.row)
            listState.animateScrollToItem(target.coerceIn(0, (rowCount - 1).coerceAtLeast(0)))
        }
    }

    Column(Modifier.fillMaxSize()) {
        // The relative path, its git mark, and for diffs the change count.
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            T(tab["path"].str, type.mono, maxLines = 1, modifier = Modifier.weight(1f))
            tab["mark"].text?.let { T(it, type.mono, gitColor(it, c)) }
        }
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (diff) {
                val count = view?.get("changeCount").int ?: 0
                T(ChangeNav.label(ordinal, count), type.small, c.muted, modifier = Modifier.padding(horizontal = 8.dp))
                val can = view?.get("changes").flag() && docId != null
                ToolButton(PaneGlyph.Up, "Previous change", can) { cmd("change", ChangeNav.data(docId!!, if (sideBySide) pairs.map { it.left } else lines, listState.firstVisibleItemIndex, false)) }
                ToolButton(PaneGlyph.Down, "Next change", can) { cmd("change", ChangeNav.data(docId!!, if (sideBySide) pairs.map { it.left } else lines, listState.firstVisibleItemIndex, true)) }
                ToolButton(if (sideBySide) PaneGlyph.Rows else PaneGlyph.Split, if (sideBySide) "Unified" else "Side by side") {
                    sel = null; cmd("mode", mapOf("sideBySide" to !sideBySide))
                }
                if (model != null && model.foldRows.isNotEmpty()) {
                    val all = expanded.containsAll(model.foldRows)
                    TextAction(if (all) "Fold unchanged" else "Show unchanged", style = type.small) { expanded = if (all) emptySet() else model.foldRows.toSet() }
                }
            }
            ToolButton(PaneGlyph.Reload, "Reload") { sel = null; cmd("reload", emptyMap()) }
        }
        view?.get("status").text?.let { T(it, type.ui, c.muted, modifier = Modifier.padding(16.dp)) }
        if (model == null) { if (view?.get("status").text == null) T("Loading…", type.small, c.muted, modifier = Modifier.padding(16.dp)); return }

        // The selection's actions.
        sel?.let { r ->
            val selLines = if (sideBySide) pairs.map { if (selSide == "old") it.left else it.right } else lines
            Row(Modifier.fillMaxWidth().background(c.panel).horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                T("${r.count} ${if (r.count == 1) "line" else "lines"}", type.small, c.muted, modifier = Modifier.padding(horizontal = 8.dp))
                val data = docId?.let { Excerpts.addData(it, model, selLines, r, if (sideBySide) selSide else "unified") }
                TextAction("Add to composer", data != null, style = type.small) {
                    cmd("add", data!!, mapOf("caret" to side.caret())); sel = null
                }
                TextAction("Copy", style = type.small) { clipboard.setText(AnnotatedString(Excerpts.text(selLines, r))); sel = null }
                TextAction("Clear selection", style = type.small) { sel = null }
            }
        }

        val digits = remember(model) { (if (model.isDiff) maxOf(model.old?.lines?.size ?: 0, model.new?.lines?.size ?: 0) else model.file?.lines?.size ?: 0).toString().length }
        val code = type.mono.copy(fontSize = 12.sp, lineHeight = 17.sp)
        fun tapLine(i: Int, line: DocLine, half: String) {
            if (line.kind == LineKind.Fold) { expanded = expanded + line.row; return }
            val s = sel
            sel = if (s == null || half != selSide) LineSelection.start(i) else s.tap(i)
            selSide = half
        }
        if (sideBySide) {
            SelectionContainer {
                LazyColumn(Modifier.fillMaxSize(), state = listState, contentPadding = PaddingValues(bottom = 24.dp)) {
                    items(pairs.size) { i ->
                        val p = pairs[i]
                        Row(Modifier.fillMaxWidth()) {
                            for ((half, line) in listOf("old" to p.left, "new" to p.right)) {
                                val selected = selSide == half && sel?.let { i in it.first..it.last } == true
                                Row(Modifier.weight(1f).background(if (selected) c.selected else lineBackground(line.kind, c)).clickable { tapLine(i, line, half) }) {
                                    DisableSelection {
                                        val n = if (half == "old") line.old else line.new
                                        BasicText(if (n >= 0) (n + 1).toString().padStart(digits) else " ".repeat(digits), style = code.copy(color = c.muted), modifier = Modifier.padding(start = 4.dp, end = 4.dp))
                                    }
                                    BasicText(styled(line, c), style = code.copy(color = c.text), softWrap = true, modifier = Modifier.padding(end = 4.dp))
                                }
                                if (half == "old") Spacer(Modifier.width(1.dp).height(17.dp).background(c.rule))
                            }
                        }
                    }
                }
            }
        } else {
            // Long lines scroll sideways together, as an editor's.
            Box(Modifier.fillMaxSize().horizontalScroll(rememberScrollState())) {
                SelectionContainer {
                    LazyColumn(Modifier.widthIn(min = 400.dp, max = 2400.dp).fillMaxSize(), state = listState, contentPadding = PaddingValues(bottom = 24.dp)) {
                        items(lines.size) { i ->
                            val line = lines[i]
                            val selected = sel?.let { i in it.first..it.last } == true
                            Row(Modifier.fillMaxWidth().background(if (selected) c.selected else lineBackground(line.kind, c))
                                .clickable { tapLine(i, line, "unified") }.semantics { if (line.kind == LineKind.Fold) contentDescription = "${line.foldCount} unchanged lines, tap to show" }) {
                                DisableSelection {
                                    val gutter = when {
                                        !model.isDiff -> (line.row + 1).toString().padStart(digits)
                                        line.kind == LineKind.Fold -> " ".repeat(digits * 2 + 1)
                                        else -> (if (line.old >= 0) (line.old + 1).toString() else "").padStart(digits) + " " + (if (line.new >= 0) (line.new + 1).toString() else "").padStart(digits)
                                    }
                                    BasicText(gutter + if (model.isDiff) " " + sign(line.kind) + " " else "  ", style = code.copy(color = when (line.kind) { LineKind.Added -> c.success; LineKind.Removed -> c.failure; else -> c.muted }),
                                        modifier = Modifier.padding(start = 8.dp))
                                }
                                BasicText(styled(line, c), style = code.copy(color = c.text), softWrap = false, modifier = Modifier.padding(end = 8.dp))
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
internal fun ToolButton(glyph: PaneGlyph, description: String, enabled: Boolean = true, tint: Color = LocalYaac.current.text, onClick: () -> Unit) {
    Box(Modifier.padding(horizontal = 2.dp).width(40.dp).height(40.dp).clickable(enabled = enabled, onClickLabel = description, onClick = onClick)
        .semantics { contentDescription = description }, contentAlignment = Alignment.Center) {
        PaneIcon(glyph, tint.copy(alpha = if (enabled) tint.alpha else .4f), size = 18.dp, strokeWidth = 1.8f)
    }
}
