package dev.yaac.android.ui.transcript

import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import dev.yaac.android.ui.LocalType
import dev.yaac.android.ui.LocalYaac
import dev.yaac.android.ui.T
import dev.yaac.android.ui.TextAction
import kotlinx.serialization.json.JsonElement

/**
 * The header's budget text: the controller's reason (`budgetReason`, sent whenever the
 * thread's budget ran out), else WPF's words while `budget` holds; null otherwise.
 */
fun budgetReason(shell: JsonElement?): String? =
    shell["budgetReason"].text ?: if (shell["budget"].flag()) "Agent budget reached" else null

/**
 * The header's budget state (T01, WPF `HeaderBudget`): why the orchestration is paused and
 * **Continue** (`header.action` `continue`). It sits outside the title's weighted column, so
 * the title truncates first and the reason stays visible.
 */
@Composable
fun HeaderBudget(shell: JsonElement?, send: (String, Any?) -> Unit) {
    val reason = budgetReason(shell) ?: return
    val c = LocalYaac.current
    val type = LocalType.current
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 4.dp)) {
        T(reason, type.small, c.warning, maxLines = 2, modifier = Modifier.widthIn(max = 120.dp))
        // Continue only where the controller can act on it (`budget`, header::command).
        if (shell["budget"].flag()) TextAction("Continue", style = type.small) { send("header.action", mapOf("id" to "continue", "threadId" to shell["threadId"].str)) }
    }
}
