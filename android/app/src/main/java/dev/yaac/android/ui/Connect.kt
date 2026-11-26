package dev.yaac.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.Connection

/**
 * The first launch (and Change hub): choose the hub to attach to over Tailscale, or the demo.
 * The Android equivalent of choosing the window's attach target; no pairing (P373).
 */
@Composable
fun ConnectScreen(lastHub: String?, onConnect: (Connection) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    var host by rememberSaveable { mutableStateOf(lastHub?.removeSuffix(":${Connection.DEFAULT_PORT}") ?: "") }
    val address = Connection.normalize(host)
    val connect = { address?.let { onConnect(Connection("hub", hub = it)) }; Unit }
    Column(Modifier.fillMaxSize().background(c.canvas).statusBarsPadding().navigationBarsPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp),
        horizontalAlignment = Alignment.Start) {
        Spacer(Modifier.height(32.dp))
        T("YAAC", type.title.copy(fontSize = 28.sp), weight = FontWeight.Bold)
        Spacer(Modifier.height(24.dp))
        T("Connect to your YAAC hub", type.title.copy(fontSize = 20.sp))
        Spacer(Modifier.height(8.dp))
        T("Turn on Settings → Window → Android on the PC first. The phone must be on the same Tailscale account.", type.ui.copy(fontSize = 15.sp), c.muted)
        Spacer(Modifier.height(20.dp))
        T("Hub", type.small, c.muted)
        Spacer(Modifier.height(4.dp))
        val interaction = remember { MutableInteractionSource() }
        val focused by interaction.collectIsFocusedAsState()
        val shape = RoundedCornerShape(6.dp)
        Column(Modifier.fillMaxWidth().widthIn(max = 520.dp).clip(shape).background(c.input).then(if (focused) Modifier.border(2.dp, c.focus, shape) else Modifier).padding(14.dp)) {
            if (host.isEmpty()) T("my-pc or 100.x.y.z", type.ui.copy(fontSize = 16.sp), c.muted)
            BasicTextField(host, { host = it }, singleLine = true, interactionSource = interaction,
                textStyle = type.mono.copy(fontSize = 16.sp, color = c.text), cursorBrush = SolidColor(c.text),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, autoCorrectEnabled = false),
                keyboardActions = KeyboardActions(onGo = { connect() }), modifier = Modifier.fillMaxWidth())
        }
        Spacer(Modifier.height(4.dp))
        T(when {
            host.isBlank() -> "The Tailscale name (MagicDNS) or 100.x address of the PC. Port ${Connection.DEFAULT_PORT} unless you add :port."
            address == null -> "That is not a host or host:port."
            else -> "Connects to $address"
        }, type.small, if (host.isNotBlank() && address == null) c.failure else c.muted)
        Spacer(Modifier.height(20.dp))
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            PrimaryButton("Connect", enabled = address != null, onClick = connect)
            TextAction("Try the demo") { onConnect(Connection("demo")) }
        }
    }
}

/** The attach ended: the controller's reason, Retry, and Change hub. */
@Composable
fun ConnectionFailed(connection: Connection?, error: String, onRetry: () -> Unit, onChange: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    Column(Modifier.fillMaxSize().background(c.canvas).statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(24.dp)) {
        Spacer(Modifier.height(32.dp))
        T("YAAC", type.title.copy(fontSize = 28.sp), weight = FontWeight.Bold)
        Spacer(Modifier.height(24.dp))
        T(if (connection?.mode == "hub") "Could not stay connected to ${connection.hub}" else "The demo stopped", type.title.copy(fontSize = 20.sp))
        Spacer(Modifier.height(8.dp))
        T(error.ifEmpty { "The connection ended." }, type.ui.copy(fontSize = 15.sp), c.failure)
        Spacer(Modifier.height(20.dp))
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            PrimaryButton("Retry", onClick = onRetry)
            TextAction("Change hub", onClick = onChange)
        }
    }
}
