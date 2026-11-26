package dev.yaac.android

import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.text
import dev.yaac.android.ui.CommanderSheet
import dev.yaac.android.ui.ShellDialog
import dev.yaac.android.ui.obj
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipboardManager
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.layout.navigationBarsPadding
import dev.yaac.android.core.Controller
import dev.yaac.android.core.FrameEffect
import dev.yaac.android.core.ViewStore
import dev.yaac.android.core.YaacRuntime
import dev.yaac.android.core.Connection
import dev.yaac.android.ui.ConnectScreen
import dev.yaac.android.ui.ConnectionFailed
import dev.yaac.android.ui.ListScreen
import dev.yaac.android.ui.LocalType
import dev.yaac.android.ui.LocalYaac
import dev.yaac.android.ui.T
import dev.yaac.android.ui.ThreadScreen
import dev.yaac.android.ui.TranscriptStates
import dev.yaac.android.ui.YaacTheme

class MainActivity : ComponentActivity() {
    /** The running controller, or none while the connection screen is up. */
    private val active = mutableStateOf<Controller?>(null)
    private val controller: Controller? get() = active.value
    /** A thread a notification asked to open; the UI shows it and clears this. */
    private val openRequest = mutableStateOf<String?>(null)
    private val quit: () -> Unit = { runOnUiThread { finishAndRemoveTask() } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Before the controller starts: its directories, and the activity the pickers use (A2).
        dev.yaac.android.core.PlatformBridge.init(this)
        // What to attach to: intent extras (`--es hubAddress`, `--es scenario`, `--es transport
        // tcp`) override the saved choice; with neither, the connection screen asks. The
        // controller belongs to the process (YaacRuntime), so it survives this activity.
        val chosen = Connection.fromIntent(intent) ?: YaacRuntime.connection ?: Connection.saved(this)
        if (chosen != null) connect(chosen, save = false)
        YaacRuntime.quitListeners += quit
        enableEdgeToEdge(SystemBarStyle.dark(android.graphics.Color.TRANSPARENT), SystemBarStyle.dark(android.graphics.Color.TRANSPARENT))
        setContent {
            val ctl = active.value
            val failed = ctl?.store?.stopped
            when {
                ctl == null -> YaacTheme(kotlinx.serialization.json.JsonObject(mapOf("theme" to kotlinx.serialization.json.JsonPrimitive("system")))) {
                    ConnectScreen(Connection.lastHub(this)) { connect(it, save = true) }
                }
                failed != null -> YaacTheme(ctl.store.view("shell")) {
                    ConnectionFailed(YaacRuntime.connection, failed,
                        onRetry = ::retry,
                        onChange = ::changeHub)
                }
                else -> YaacApp(ctl.store, ctl::send, openRequest, onChangeHub = ::changeHub, onRetry = ::retry)
            }
        }
        handleOpen(intent)
        if (android.os.Build.VERSION.SDK_INT >= 33 && checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED)
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
    }

    private fun connect(c: Connection, save: Boolean) {
        if (save) Connection.save(this, c)
        val ctl = YaacRuntime.start(applicationContext, c)
        ctl.send("shell.systemTheme", mapOf("light" to !isDark()))
        ctl.send("shell.visible", mapOf("shown" to true, "hidden" to false))
        active.value = ctl
    }

    /** Retry: a fresh attach to the same hub (a new controller; nothing is re-sent). */
    private fun retry() {
        val c = YaacRuntime.connection ?: return
        YaacRuntime.disconnect(applicationContext)
        connect(c, save = false)
    }

    /** Change hub: end this attach (never the hub), forget the choice, show the connection screen. */
    private fun changeHub() {
        YaacRuntime.disconnect(applicationContext)
        Connection.save(this, null)
        active.value = null
    }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        handleOpen(intent)
    }

    private fun handleOpen(intent: android.content.Intent?) {
        val ctl = controller ?: return
        val n = dev.yaac.android.notify.Notifier
        // A request notification: open that request (the exact card, or its open ancestor's copy).
        val request = intent?.getStringExtra(n.EXTRA_REQUEST)
        val requestThread = intent?.getStringExtra(n.EXTRA_THREAD)
        if (request != null && requestThread != null) {
            intent.removeExtra(n.EXTRA_REQUEST)
            ctl.send("shell.requestOpen", mapOf("threadId" to requestThread, "requestId" to request))
            openRequest.value = requestThread
            return
        }
        val id = intent?.getStringExtra(dev.yaac.android.notify.Notifier.EXTRA_OPEN) ?: return
        intent.removeExtra(dev.yaac.android.notify.Notifier.EXTRA_OPEN)
        ctl.send("sidebar.open", mapOf("id" to id))
        openRequest.value = id
    }

    // The activity handles uiMode itself (configChanges), so the system theme is reported here.
    override fun onConfigurationChanged(newConfig: android.content.res.Configuration) {
        super.onConfigurationChanged(newConfig)
        controller?.send("shell.systemTheme", mapOf("light" to !isDark()))
    }

    private fun isDark() = (resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK) == android.content.res.Configuration.UI_MODE_NIGHT_YES

    override fun onStart() { super.onStart(); controller?.send("shell.visible", mapOf("shown" to true, "hidden" to false)) }
    override fun onResume() { super.onResume(); dev.yaac.android.notify.Notifier.foreground = true }
    override fun onPause() {
        dev.yaac.android.notify.Notifier.foreground = false
        // Not focused: no card is on screen, so every request may be notified.
        controller?.send("shell.visibleRequests", mapOf("cards" to emptyList<Any>()))
        super.onPause()
    }
    override fun onStop() { controller?.send("shell.visible", mapOf("shown" to false, "hidden" to true)); super.onStop() }
    override fun onDestroy() {
        YaacRuntime.quitListeners -= quit
        // Without "Keep running in the background", closing the last window ends the client.
        val keep = controller?.store?.view("shell")["trayOnClose"].flag(true)
        if (isFinishing && !keep) YaacRuntime.quit(applicationContext)
        super.onDestroy()
    }
}

private enum class Screen { List, Thread }

@Composable
private fun YaacApp(store: ViewStore, send: (String, Any?) -> Unit, openRequest: androidx.compose.runtime.MutableState<String?>, onChangeHub: () -> Unit, onRetry: () -> Unit) {
    val shell = store.view("shell")
    YaacTheme(shell) {
        val clipboard: ClipboardManager = LocalClipboardManager.current
        val side = remember(store) { dev.yaac.android.ui.SidePaneState(send) }
        var viewer by remember { mutableStateOf<kotlinx.serialization.json.JsonObject?>(null) }
        val context = androidx.compose.ui.platform.LocalContext.current
        store.effects = { fx ->
            when (fx) {
                is FrameEffect.Copy -> clipboard.setText(AnnotatedString(fx.text))
                is FrameEffect.Error -> Toast.makeText(context, fx.text, Toast.LENGTH_LONG).show()
                is FrameEffect.Window -> {}
                is FrameEffect.FilesDocument -> side.document(fx.frame)
                is FrameEffect.Terminal -> side.terminal(fx.frame)
                is FrameEffect.ImageViewer -> viewer = fx.frame
            }
        }
        var screen by rememberSaveable { mutableStateOf(Screen.List) }
        // A command that navigates (Needs you, a Commander pick, a link) shows the thread it opens.
        var openOnChange by remember { mutableStateOf(false) }
        // What is open: a thread, or a draft (which has no thread id, only its composer key).
        val threadId = shell["threadId"].text
        val draftKey = store.view("composer")["key"].text
        val opened = threadId ?: draftKey
        var lastOpened by remember { mutableStateOf(opened) }
        LaunchedEffect(opened) {
            if (opened != lastOpened && openOnChange && opened != null) { screen = Screen.Thread; openOnChange = false }
            lastOpened = opened
        }
        // A navigating command opens what it opens soon, or not at all (Start a thread may first
        // ask for a project); the intent expires so a later, unrelated change never navigates.
        LaunchedEffect(openOnChange) { if (openOnChange) { kotlinx.coroutines.delay(20_000); openOnChange = false } }
        val navigate: (String, Any?) -> Unit = { cmd, payload ->
            val starts = cmd == "settings.action" && (payload as? Map<*, *>)?.get("id") == "first.start"
            if (cmd == "commander.pick" || cmd == "shell.needs" || starts) openOnChange = true
            send(cmd, payload)
        }
        LaunchedEffect(openRequest.value) { if (openRequest.value != null) { screen = Screen.Thread; openRequest.value = null } }
        val states = remember { TranscriptStates() }
        val c = LocalYaac.current
        // Landscape puts the navigation bar (and a cutout) at the side: keep content clear of it.
        val sides = androidx.compose.foundation.layout.WindowInsetsSides.Horizontal
        Box(Modifier.fillMaxSize().background(c.canvas)
            .windowInsetsPadding(androidx.compose.foundation.layout.WindowInsets.navigationBars.only(sides))
            .windowInsetsPadding(androidx.compose.foundation.layout.WindowInsets.displayCutout.only(sides))) {
            val thread: @Composable (Modifier, Boolean) -> Unit = { m, compact ->
                Box(m) {
                    ThreadScreen(shell, store.view("transcript"), store.view("composer"), store.view("agents"), store.view("sidebar"), store.view("composerMessageHistory"), states, navigate, browser = store.view("browser"), side = side,
                        onBack = { screen = Screen.List }, onNeeds = { navigate("shell.needs", null) }, showBack = compact)
                }
            }
            androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize()) {
                val wide = maxWidth
                if (wide >= 600.dp) {
                    // Medium and expanded width (A04): the list and the thread side by side, as the
                    // desktop does in a narrow window; one hairline between the regions.
                    if (shell["canBack"].flag()) BackHandler { send("shell.back", null) }
                    androidx.compose.foundation.layout.Row(Modifier.fillMaxSize()) {
                        // Expanded width shows the full list; medium width, the strip (desktop rule).
                        if (wide >= 840.dp) Box(Modifier.width(340.dp).fillMaxHeight()) {
                            ListScreen(store.view("sidebar"), shell, navigate, openThread = { }, onNeeds = { navigate("shell.needs", null) })
                        } else dev.yaac.android.ui.StripScreen(store.view("sidebar"), shell, navigate)
                        Box(Modifier.width(1.dp).fillMaxHeight().background(c.rule))
                        thread(Modifier.weight(1f).fillMaxHeight(), false)
                    }
                } else when (screen) {
                    Screen.List -> ListScreen(store.view("sidebar"), shell, navigate, openThread = { screen = Screen.Thread }, onNeeds = { navigate("shell.needs", null) })
                    Screen.Thread -> {
                        // System Back walks YAAC's history (shell.back) and ends at the list (P375).
                        BackHandler { if (shell["canBack"].flag()) send("shell.back", null) else screen = Screen.List }
                        thread(Modifier.fillMaxSize(), true)
                    }
                }
            }
            // Settings covers the list and the thread while it is open (`shell.settings`), and
            // first run or a required update takes the whole window.
            val settingsView = store.view("settings")
            val settingsMode = settingsView["mode"].text
            if (shell["settings"].flag() || settingsMode == "firstRun" || settingsMode == "updateNeeded")
                dev.yaac.android.ui.SettingsScreen(settingsView, navigate, onChangeHub)
            // On a phone the WSL cards and the status notice sit over the list; the thread shows
            // the status as a banner and the stopped distro in its composer.
            if (screen == Screen.List) ShellNotices(shell, send)
            // Attached to a hub that does not answer (or refused the hello, e.g. another build):
            // say which hub, why, and offer Retry and Change hub.
            val conn = YaacRuntime.connection
            if (conn?.mode == "hub" && shell != null && !shell["hub"]["reachable"].flag(true))
                HubDown(conn.hub ?: "", shell["status"]["text"].text, onRetry, onChangeHub)
            store.status?.let { s ->
                Box(Modifier.fillMaxWidth().statusBarsPadding().padding(top = 60.dp).align(Alignment.TopCenter), contentAlignment = Alignment.Center) {
                    T(s, LocalType.current.small, c.muted)
                }
            }
        }
        shell["commander"].obj()?.let { CommanderSheet(it, navigate) }
        viewer?.let { v -> dev.yaac.android.ui.ImageViewer(v) { viewer = null; send("image.close", null) } }
        shell["dialog"].obj()?.takeIf { it["id"].text != null }?.let { ShellDialog(it, navigate) }
    }
}

/**
 * Window-level notices (WPF's status notice and WSL toasts): `shell.status` with Dismiss
 * (`shell.dismissStatus`), and a card per stopped WSL distro with Start it (`shell.startWsl`).
 */
@Composable
private fun ShellNotices(shell: kotlinx.serialization.json.JsonElement?, send: (String, Any?) -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    val status = shell["status"].obj()
    val stopped = shell["stoppedWsl"].let { (it as? kotlinx.serialization.json.JsonArray)?.mapNotNull { e -> e as? kotlinx.serialization.json.JsonObject } }.orEmpty()
    if (status?.get("text").text == null && stopped.isEmpty()) return
    androidx.compose.foundation.layout.Column(Modifier.fillMaxSize().navigationBarsPadding().padding(start = 12.dp, end = 12.dp, bottom = 96.dp),
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.Bottom) {
        for (w in stopped) {
            androidx.compose.foundation.layout.Column(Modifier.fillMaxWidth().padding(top = 6.dp).clip(androidx.compose.foundation.shape.RoundedCornerShape(8.dp)).background(c.raised).padding(14.dp)) {
                T(w["label"].text ?: "", type.ui.copy(fontSize = 15.sp))
                dev.yaac.android.ui.TextAction("Start it") { send("shell.startWsl", mapOf("id" to w["id"].text)) }
            }
        }
        status?.get("text").text?.let { text ->
            androidx.compose.foundation.layout.Row(Modifier.fillMaxWidth().padding(top = 6.dp).clip(androidx.compose.foundation.shape.RoundedCornerShape(8.dp)).background(c.input).padding(start = 14.dp),
                verticalAlignment = Alignment.CenterVertically) {
                T(text, type.ui, c.failure, modifier = Modifier.weight(1f))
                dev.yaac.android.ui.TextAction("Dismiss") { status["id"]?.let { send("shell.dismissStatus", mapOf("id" to (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull())) } }
            }
        }
    }
}

@Composable
private fun HubDown(hub: String, reason: String?, onRetry: () -> Unit, onChange: () -> Unit) {
    val c = LocalYaac.current
    val type = LocalType.current
    androidx.compose.foundation.layout.Column(Modifier.fillMaxWidth().statusBarsPadding().padding(start = 12.dp, end = 12.dp, top = 64.dp)
        .clip(androidx.compose.foundation.shape.RoundedCornerShape(8.dp)).background(c.raised).padding(16.dp)) {
        T("Not connected to $hub", type.heading)
        T(reason ?: "The hub does not answer. Check that the PC is on, on the same Tailscale account, with Settings → Window → Android turned on.",
            type.ui, if (reason != null) c.failure else c.muted, modifier = Modifier.padding(top = 4.dp))
        androidx.compose.foundation.layout.Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            dev.yaac.android.ui.PrimaryButton("Retry", onClick = onRetry)
            dev.yaac.android.ui.TextAction("Change hub", onClick = onChange)
        }
    }
}
