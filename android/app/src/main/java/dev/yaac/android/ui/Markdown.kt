package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.selection.DisableSelection
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.list
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import dev.yaac.android.ui.transcript.lineNumberGutter

/** Inline spans `{text, bold, italic, code, link}` as one annotated string. */
@Composable
fun spans(spans: JsonElement?, color: Color): AnnotatedString {
    val c = LocalYaac.current
    return buildAnnotatedString {
        for (s in spans.objects) {
            val text = s["text"].str
            val style = SpanStyle(
                fontWeight = if (s["bold"].flag()) FontWeight.Bold else null,
                fontStyle = if (s["italic"].flag()) FontStyle.Italic else null,
                fontFamily = if (s["code"].flag()) MonoFont else null,
                background = if (s["code"].flag()) c.input else Color.Unspecified,
                fontSize = if (s["code"].flag()) 0.9.em else androidx.compose.ui.unit.TextUnit.Unspecified,
            )
            val link = s["link"].text?.takeIf { it.startsWith("http://") || it.startsWith("https://") }
            if (link != null) {
                withLink(LinkAnnotation.Url(link, TextLinkStyles(SpanStyle(color = c["YaacLink"], textDecoration = TextDecoration.Underline)))) { withStyle(style) { append(text) } }
            } else withStyle(style) { append(text) }
        }
    }
}

private val Double.em get() = androidx.compose.ui.unit.TextUnit(this.toFloat(), androidx.compose.ui.unit.TextUnitType.Em)

/**
 * Markdown blocks as the controller parses them (`TranscriptText`): paragraph, heading, item,
 * quote, rule, code, diff, table. Falls back to [fallback] text when there are no blocks.
 */
@Composable
fun Blocks(blocks: JsonElement?, fallback: String?, style: TextStyle, color: Color, modifier: Modifier = Modifier) {
    val list = blocks.objects
    if (list.isEmpty()) {
        if (!fallback.isNullOrEmpty()) BasicText(fallback, modifier, style = style.copy(color = color))
        return
    }
    val c = LocalYaac.current
    Column(modifier) {
        list.forEachIndexed { i, b ->
            if (i > 0) Spacer(Modifier.height(8.dp))
            when (b["kind"].str) {
                "heading" -> {
                    val extra = when (b["level"].int) { 1 -> 5; 2 -> 3; else -> 1 }
                    BasicText(spans(b["spans"], color), style = style.copy(color = color, fontWeight = FontWeight.SemiBold, fontSize = (style.fontSize.value + extra).sp))
                }
                "item" -> Row(Modifier.padding(start = (6 + 18 * (b["depth"].int ?: 0)).dp)) {
                    BasicText(b["marker"].str + "  ", style = style.copy(color = c.muted))
                    BasicText(spans(b["spans"], color), style = style.copy(color = color))
                }
                "quote" -> Row {
                    Box(Modifier.width(3.dp).height(20.dp).background(c.rule))
                    Spacer(Modifier.width(10.dp))
                    BasicText(spans(b["spans"], color), style = style.copy(color = c.muted))
                }
                "rule" -> Box(Modifier.fillMaxWidth().height(1.dp).background(c.rule))
                "mermaid" -> MermaidBlock(b["text"].str)
                "code", "diff" -> CodeBlock(b["copyText"].text ?: b["text"].str, b["text"].str, b["language"].text, b["kind"].str == "diff", lineNumbers = b["kind"].str == "code")
                "table" -> Table(b, style, color)
                else -> BasicText(spans(b["spans"], color).takeIf { it.isNotEmpty() } ?: AnnotatedString(b["text"].str), style = style.copy(color = color))
            }
        }
    }
}

/**
 * A code surface: the language and **Copy** (never part of a text selection), then the code,
 * scrolling sideways. Fenced code shows line numbers in a gutter that is not selected or copied
 * (07-ui: "Copying text omits the line numbers").
 */
@Composable
fun CodeBlock(copy: String, text: String, language: String?, diff: Boolean = false, lineNumbers: Boolean = false) {
    val c = LocalYaac.current
    val clipboard = LocalClipboardManager.current
    val mono = LocalType.current.mono
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(c.input)) {
        DisableSelection {
            Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                T(language ?: "", LocalType.current.small, c.muted, modifier = Modifier.weight(1f))
                T("Copy", LocalType.current.small, c.muted, modifier = Modifier.clip(RoundedCornerShape(4.dp)).clickable { clipboard.setText(AnnotatedString(copy)) }.padding(horizontal = 8.dp, vertical = 6.dp))
            }
        }
        val shown = if (lineNumbers) text.removeSuffix("\n") else text
        val styled = if (!diff) AnnotatedString(shown) else buildAnnotatedString {
            shown.lines().forEachIndexed { i, line ->
                if (i > 0) append("\n")
                val col = when { line.startsWith("+") -> c.success; line.startsWith("-") -> c.failure; else -> c.text }
                withStyle(SpanStyle(color = col)) { append(line) }
            }
        }
        Row(Modifier.padding(bottom = 10.dp)) {
            if (lineNumbers) DisableSelection {
                BasicText(lineNumberGutter(shown), Modifier.padding(start = 10.dp, end = 2.dp), style = mono.copy(color = c.muted), softWrap = false)
            }
            Box(Modifier.horizontalScroll(rememberScrollState()).padding(start = if (lineNumbers) 8.dp else 12.dp, end = 12.dp)) {
                BasicText(styled, style = mono.copy(color = c.text), softWrap = false)
            }
        }
    }
}

@Composable
private fun Table(b: JsonElement, style: TextStyle, color: Color) {
    val c = LocalYaac.current
    val rows = (listOf(b["head"]) + b["rows"].list).filter { it is JsonArray && it.isNotEmpty() }
    Box(Modifier.horizontalScroll(rememberScrollState())) {
        Column {
            rows.forEachIndexed { r, row ->
                Row {
                    for (cell in row.list) {
                        BasicText(spans(cell, color), Modifier.widthIn(min = 60.dp, max = 260.dp).padding(horizontal = 6.dp, vertical = 4.dp),
                            style = style.copy(color = color, fontSize = 14.sp, fontWeight = if (r == 0 && b["head"].list.isNotEmpty()) FontWeight.SemiBold else null))
                    }
                }
                if (r == 0) Box(Modifier.height(1.dp).width(200.dp).background(c.rule))
            }
        }
    }
}
