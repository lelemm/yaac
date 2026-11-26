package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import android.content.res.Configuration
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/**
 * The Commander (A22, `shell.commander`): a query field and the controller's rows. Key caps are
 * shown only with a hardware keyboard. `commander.query`, `.pick`, `.back`, `.close`.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CommanderSheet(view: JsonObject, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val step = view["step"].text
    var query by remember(step) { mutableStateOf(TextFieldValue(view["query"].str, TextRange(view["query"].str.length))) }
    val focus = remember { FocusRequester() }
    val hardKeys = LocalConfiguration.current.keyboard == Configuration.KEYBOARD_QWERTY
    LaunchedEffect(step) { runCatching { focus.requestFocus() } }
    ModalBottomSheet(
        onDismissRequest = { send("commander.close", null) },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = c.raised, contentColor = c.text, tonalElevation = 0.dp,
    ) {
        Column(Modifier.fillMaxWidth().imePadding().navigationBarsPadding().padding(bottom = 8.dp)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                if (step != null) IconButton(YIcon.Back, "Back") { send("commander.back", null) }
                Row(Modifier.weight(1f).clip(RoundedCornerShape(6.dp)).background(c.input).padding(horizontal = 12.dp, vertical = 11.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(YIcon.Command, c.muted, size = 17.dp)
                    Spacer(Modifier.width(10.dp))
                    BasicTextField(query, { v ->
                        val changed = v.text != query.text
                        query = v
                        if (changed) send("commander.query", mapOf("text" to v.text))
                    }, singleLine = true, textStyle = type.ui.copy(fontSize = 16.sp, color = c.text), cursorBrush = SolidColor(c.text),
                        modifier = Modifier.weight(1f).focusRequester(focus))
                }
            }
            step?.let { T(it, type.small, c.muted, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp)) }
            val rows = view["rows"].objects
            LazyColumn(Modifier.fillMaxWidth().heightIn(max = 460.dp).padding(top = 6.dp)) {
                items(rows, key = { it["id"].str + "|" + it["label"].str }) { row ->
                    row["heading"].text?.let { T(it, type.small, c.muted, modifier = Modifier.padding(start = 20.dp, top = 10.dp, bottom = 2.dp)) }
                    val enabled = row["enabled"].flag()
                    Row(Modifier.fillMaxWidth().clickable(enabled = enabled) { send("commander.pick", mapOf("id" to row["id"].str, "query" to view["query"].str)) }
                        .heightIn(min = 48.dp).padding(horizontal = 20.dp, vertical = 6.dp).alpha(if (enabled) 1f else 0.5f), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f).padding(start = 0.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                if (row["checked"].flag()) { Icon(YIcon.Check, c.text, size = 16.dp); Spacer(Modifier.width(6.dp)) }
                                T(row["label"].str, type.ui.copy(fontSize = 15.sp))
                            }
                            row["detail"].text?.let { T(it, type.small, c.muted) }
                        }
                        if (hardKeys) row["key"].text?.let { T(it, type.monoSmall, c.muted) }
                    }
                }
            }
        }
    }
}

/** The header sheet (A06): the title and its Rename, the state and place, the links, Copy ID. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HeaderSheet(shell: JsonElement?, sidebar: JsonElement?, send: (String, Any?) -> Unit, onDismiss: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val clipboard = LocalClipboardManager.current
    val threadId = shell["threadId"].str
    val row = sidebar["sections"].objects.flatMap { it["rows"].objects }.firstOrNull { it["id"].str == threadId }
    val actions = ActionItem.list(row?.get("actions"))
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.raised, contentColor = c.text, tonalElevation = 0.dp) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 20.dp).padding(bottom = 16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                T(shell["title"].str, type.title.copy(fontSize = 20.sp), modifier = Modifier.weight(1f))
                actions.firstOrNull { it.id == "rename" }?.let { a -> TextAction(a.label, a.enabled) { onDismiss(); send("sidebar.action", mapOf("action" to "rename", "ids" to listOf(threadId))) } }
            }
            shell["tag"].text?.let { T(it, type.ui, c.muted) }
            Spacer(Modifier.heightIn(min = 10.dp))
            shell["location"].text?.let { Field("Where", it) }
            row?.get("providerName").text?.let { Field("Harness", it) }
            row?.get("line2")?.get("branch").text?.let { Field("Branch", it) }
            val links = shell["links"].objects
            if (links.isNotEmpty()) {
                T("Links", type.small, c.muted, modifier = Modifier.padding(top = 10.dp))
                for (l in links) {
                    val id = l["id"].text
                    T(l["label"].str, type.ui.copy(fontSize = 15.sp), if (id == null) c.muted else c.text,
                        modifier = Modifier.fillMaxWidth().clickable(enabled = id != null) { onDismiss(); send("sidebar.open", mapOf("id" to id)) }.padding(vertical = 8.dp))
                }
            }
            Spacer(Modifier.heightIn(min = 8.dp))
            Row(Modifier.fillMaxWidth().clickable { clipboard.setText(AnnotatedString(row?.get("copyId").text ?: threadId)); onDismiss() }.padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(YIcon.Copy, c.text, size = 17.dp); Spacer(Modifier.width(10.dp)); T("Copy thread ID", type.ui.copy(fontSize = 15.sp))
            }
        }
    }
}

@Composable
private fun Field(label: String, value: String) {
    val c = LocalYaac.current
    Row(Modifier.padding(vertical = 3.dp)) {
        T(label, LocalType.current.ui, c.muted, modifier = Modifier.width(80.dp))
        T(value, LocalType.current.ui)
    }
}

@Suppress("unused") private fun JsonElement?.n() = int
