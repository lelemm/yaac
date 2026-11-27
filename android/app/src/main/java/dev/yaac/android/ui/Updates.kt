package dev.yaac.android.ui

import android.app.Activity
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import kotlinx.serialization.json.JsonObject
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.yaac.android.core.AppUpdates
import dev.yaac.android.core.AppUpdates.State
import dev.yaac.android.core.Updates

/**
 * The phone and the hub run different builds (`shell.hub.mismatch`): offer the app of the hub's
 * build. Drawn over the list or the Update YAAC page, at the bottom.
 */
@Composable
fun HubBuildOffer(pcUpdate: JsonObject?, send: (String, Any?) -> Unit, modifier: Modifier = Modifier) {
    val hub = AppUpdates.hubBuild ?: return
    if (!AppUpdates.hubMismatch) return
    val c = LocalYaac.current
    val type = LocalType.current
    val drafts = remember { mutableStateMapOf<String, String>() }
    Column(modifier.fillMaxWidth().navigationBarsPadding().padding(12.dp).clip(RoundedCornerShape(8.dp)).background(c.raised).padding(16.dp)) {
        T("This phone runs YAAC ${AppUpdates.ownBuild ?: "?"}; the hub runs YAAC $hub.", type.ui.copy(fontSize = 15.sp))
        // The phone is newer: the PC can install its build (P403). The button is here because a
        // phone the hub does not attach may not reach Settings → About.
        if (pcUpdate != null) {
            SettingsRow(pcUpdate, "pc.", drafts, send)
            T("Or put this phone on the PC's build:", type.small, c.muted, modifier = Modifier.padding(top = 12.dp))
        }
        if (Updates.needsUninstall(hub, AppUpdates.ownBuild))
            T("That build is older than this app: Android installs it only after YAAC is uninstalled. Updating YAAC on the PC instead keeps this app.",
                type.small, c.muted, modifier = Modifier.padding(top = 4.dp))
        Box(Modifier.padding(top = 8.dp)) { Progress(hub, forHub = true) }
    }
}

/** Settings (below the pages): Check for updates, with the hub-matching build first. */
@Composable
fun UpdatesRow() {
    val c = LocalYaac.current
    val type = LocalType.current
    val s = AppUpdates.state
    val context = LocalContext.current.applicationContext
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp)) {
        Row(Modifier.fillMaxWidth().clickable(enabled = s !is State.Checking && s !is State.Downloading) { AppUpdates.check(forHub = AppUpdates.hubBuild != null) }
            .heightIn(min = 52.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                T("Check for updates", type.ui.copy(fontSize = 16.sp))
                T("This app ${AppUpdates.ownBuild ?: "?"}" + (AppUpdates.hubBuild?.let { " · hub $it" } ?: ""), type.small, c.muted)
            }
        }
        T("The phone must run the same build as the PC's hub. Update both together.", type.small, c.muted)
        if (s !is State.Idle) Box(Modifier.padding(top = 8.dp)) { Progress(AppUpdates.hubBuild, forHub = AppUpdates.hubBuild != null) }
        // With the hub's build found (or no hub), the latest release is offered too, with the warning.
        val latest = AppUpdates.latest
        val latestBuild = latest?.build
        if (latest != null && latestBuild != null && latestBuild != AppUpdates.hubBuild && latestBuild != AppUpdates.ownBuild && (s is State.Found && s.forHub || s is State.None)) {
            val apk = Updates.apk(latest, latestBuild)
            if (apk != null) Column(Modifier.padding(top = 10.dp)) {
                T("The latest release is YAAC $latestBuild." + (AppUpdates.hubBuild?.let { " It will not connect to the hub ($it) until the PC runs it too." } ?: ""),
                    type.small, if (AppUpdates.hubBuild != null) c.failure else c.muted)
                TextAction("Download YAAC $latestBuild anyway") {
                    AppUpdates.download(context, State.Found(latestBuild, latest, apk, forHub = false))
                }
            }
        }
        Box(Modifier.height(12.dp))
    }
}

/** What [AppUpdates.state] says, with its one next action. */
@Composable
private fun Progress(hub: dev.yaac.android.core.BuildLabel?, forHub: Boolean) {
    val c = LocalYaac.current
    val type = LocalType.current
    val context = LocalContext.current
    when (val s = AppUpdates.state) {
        State.Idle -> if (forHub && hub != null) PrimaryButton("Download YAAC $hub for this phone") { AppUpdates.check(forHub = true) }
        is State.Checking -> T(s.what, type.ui, c.muted)
        is State.Found -> Column {
            if (!s.forHub) {
                T("YAAC ${s.build} is the latest release." + if (s.build == AppUpdates.ownBuild) " This app is up to date." else "", type.ui)
                if (AppUpdates.hubBuild == null) T("Install it only if the PC's hub runs YAAC ${s.build} too.", type.small, c.muted)
            }
            if (s.build != AppUpdates.ownBuild)
                PrimaryButton("Download YAAC ${s.build} (${mb(s.apk.size)})", modifier = Modifier.padding(top = 6.dp)) { AppUpdates.download(context.applicationContext, s) }
        }
        is State.None -> Column {
            T(s.message, type.ui, c.muted)
            TextAction("Check again") { AppUpdates.check(forHub) }
        }
        is State.Downloading -> Column {
            T("Downloading YAAC ${s.build}… ${mb(s.done)}" + if (s.total > 0) " of ${mb(s.total)}" else "", type.ui)
            if (s.total > 0) Box(Modifier.fillMaxWidth().padding(top = 6.dp).height(6.dp).clip(RoundedCornerShape(3.dp)).background(c.rule)) {
                Box(Modifier.fillMaxWidth((s.done.toFloat() / s.total).coerceIn(0f, 1f)).height(6.dp).background(c.accent))
            }
        }
        is State.Ready -> Column {
            T("YAAC ${s.build} is downloaded.", type.ui)
            if (!AppUpdates.canInstall(context))
                T("Android asks once to allow YAAC to install apps: allow it, come back, and tap Install again.", type.small, c.muted)
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 6.dp)) {
                PrimaryButton("Install") { (context as? Activity)?.let { AppUpdates.install(it, s) } }
                TextAction("Later") { AppUpdates.dismiss() }
            }
        }
        is State.Failed -> Column {
            T(s.message, type.ui, c.failure)
            TextAction("Try again") { AppUpdates.check(forHub) }
        }
    }
}

private fun mb(bytes: Long): String = "%.1f MB".format(bytes / 1_048_576.0)
