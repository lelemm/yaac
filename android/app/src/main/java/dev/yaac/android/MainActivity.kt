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
import dev.yaac.android.core.str
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
    /** The selected hub's controller, or none while the connection screen is up (P404). */
    private val controller: Controller? get() = YaacRuntime.controller
    /** Something shared into YAAC, waiting for the user to choose its thread. */
    private val share = mutableStateOf<dev.yaac.android.core.Share?>(null)
    /** A thread a notification asked to open; the UI shows it and clears this. */
    private val openRequest = mutableStateOf<String?>(null)
    /** Settings → Hubs… is open; Add a hub is open. */
    private val hubsOpen = mutableStateOf(false)
    private val adding = mutableStateOf(false)
    private val quit: () -> Unit = { runOnUiThread { finishAndRemoveTask() } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Before the controllers start: their directories, and the activity the pickers use (A2).
        dev.yaac.android.core.PlatformBridge.init(this)
        YaacRuntime.systemLight = !isDark()
        // What to attach to: intent extras (`--es hubAddress` adds and selects a hub, `--es
        // scenario`, `--es transport tcp`) override the saved choice; with neither, the
        // connection screen asks. The controllers belong to the process (YaacRuntime), so they
        // survive this activity.
        if (YaacRuntime.running.isEmpty()) {
            val mode = Connection.savedMode(this)
            YaacRuntime.load(this, migrate = mode == "hub")
            val extra = Connection.fromIntent(intent)
            when {
                extra?.mode == "hub" -> YaacRuntime.addHub(applicationContext, extra.hub!!, null)
                extra != null -> YaacRuntime.startSingle(applicationContext, extra)
                mode == "demo" -> YaacRuntime.startSingle(applicationContext, Connection("demo"))
                mode == "hub" -> YaacRuntime.startHubs(applicationContext)
            }
        }
        YaacRuntime.quitListeners += quit
        enableEdgeToEdge(SystemBarStyle.dark(android.graphics.Color.TRANSPARENT), SystemBarStyle.dark(android.graphics.Color.TRANSPARENT))
        setContent {
            val sel = YaacRuntime.selected
            val failed = sel?.store?.stopped
            val plain = kotlinx.serialization.json.JsonObject(mapOf("theme" to kotlinx.serialization.json.JsonPrimitive("system")))
            when {
                sel == null || adding.value -> YaacTheme(sel?.store?.view("shell") ?: plain) {
                    val first = sel == null
                    ConnectScreen(if (first) YaacRuntime.hubs.current?.address ?: Connection.lastHub(this) else null, adding = !first || YaacRuntime.hubs.hubs.isNotEmpty(),
                        onConnect = { address, label -> addHub(address, label) },
                        onDemo = if (first) ({ connectDemo() }) else null,
                        onHubs = if (first && YaacRuntime.hubs.hubs.isNotEmpty()) ({ Connection.saveMode(this, "hub"); YaacRuntime.startHubs(applicationContext) }) else null,
                        onCancel = if (!first) ({ adding.value = false }) else null)
                }
                hubsOpen.value -> YaacTheme(sel.store.view("shell")) {
                    dev.yaac.android.ui.HubsScreen(onAdd = { adding.value = true }, onLeaveDemo = ::leaveDemo, onClose = { hubsOpen.value = false })
                }
                failed != null -> YaacTheme(sel.store.view("shell")) {
                    ConnectionFailed(sel.connection.takeIf { sel.hub == null } ?: Connection("hub", hub = sel.name), failed,
                        onRetry = ::retry,
                        onChange = ::openHubs)
                }
                // Each hub has its own window state (screen, panes, scroll); switching hubs
                // switches all of it, and its theme comes from its own shell.
                else -> androidx.compose.runtime.key(sel.key) {
                    val ctl = sel.controller
                    androidx.compose.runtime.CompositionLocalProvider(
                        dev.yaac.android.core.LocalHubKey provides sel.localKey,
                        dev.yaac.android.ui.LocalOpenHubs provides ::openHubs,
                    ) {
                        YaacApp(ctl.store, { name, payload -> dev.yaac.android.ui.pane.ComposerCaret.observe(name, payload); ctl.send(name, payload) }, openRequest, onHubs = ::openHubs, onRetry = ::retry, share = share)
                    }
                }
            }
        }
        handleOpen(intent)
        handleShare(intent)
        if (android.os.Build.VERSION.SDK_INT >= 33 && checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED)
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
    }

    private fun addHub(address: String, label: String?) {
        YaacRuntime.addHub(applicationContext, address, label)
        adding.value = false
        hubsOpen.value = false
    }

    private fun connectDemo() {
        Connection.saveMode(this, "demo")
        YaacRuntime.startSingle(applicationContext, Connection("demo"))
    }

    /** Leave the demo: its controller ends and the connection screen asks (the hubs stay saved). */
    private fun leaveDemo() {
        YaacRuntime.disconnect(applicationContext)
        hubsOpen.value = false
    }

    /** Retry: a fresh attach to the selected hub (a new controller; nothing is re-sent). */
    private fun retry() {
        val key = YaacRuntime.selected?.key ?: return
        YaacRuntime.retry(applicationContext, key)
    }

    private fun openHubs() { hubsOpen.value = true }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        handleOpen(intent)
        handleShare(intent)
    }

    /**
     * The share sheet (C09): text and files from another app. Files are copied into the app's
     * cache off the UI thread (a content URI cannot be read later), then the user picks a thread.
     */
    private fun handleShare(intent: android.content.Intent?) {
        val action = intent?.action ?: return
        if (action != android.content.Intent.ACTION_SEND && action != android.content.Intent.ACTION_SEND_MULTIPLE) return
        intent.action = null
        val text = intent.getStringExtra(android.content.Intent.EXTRA_TEXT)
        @Suppress("DEPRECATION")
        val uris: List<android.net.Uri> = if (action == android.content.Intent.ACTION_SEND_MULTIPLE)
            intent.getParcelableArrayListExtra<android.net.Uri>(android.content.Intent.EXTRA_STREAM).orEmpty()
        else listOfNotNull(intent.getParcelableExtra(android.content.Intent.EXTRA_STREAM))
        Thread({
            val dir = java.io.File(cacheDir, "shared/" + java.util.UUID.randomUUID()).apply { mkdirs() }
            val paths = uris.mapNotNull { uri ->
                runCatching {
                    val name = contentResolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                        if (c.moveToFirst()) c.getString(0) else null
                    }?.replace('/', '_')?.ifBlank { null } ?: "shared"
                    val file = java.io.File(dir, name)
                    contentResolver.openInputStream(uri)?.use { input -> file.outputStream().use { input.copyTo(it) } } ?: return@runCatching null
                    file.absolutePath
                }.getOrNull()
            }
            val s = dev.yaac.android.core.Share(text, paths)
            if (!s.isEmpty) runOnUiThread { share.value = s }
        }, "yaac-share").start()
    }

    private fun handleOpen(intent: android.content.Intent?) {
        val n = dev.yaac.android.notify.Notifier
        // A notification names its hub (P404): show that hub, then its thread.
        intent?.getStringExtra(n.EXTRA_HUB)?.let { hub ->
            intent.removeExtra(n.EXTRA_HUB)
            if (YaacRuntime.running[hub] != null) {
                YaacRuntime.select(applicationContext, hub)
                hubsOpen.value = false; adding.value = false
            }
        }
        val ctl = controller ?: return
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
        YaacRuntime.systemLight = !isDark()
        YaacRuntime.broadcast("shell.systemTheme", mapOf("light" to !isDark()))
    }

    private fun isDark() = (resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK) == android.content.res.Configuration.UI_MODE_NIGHT_YES

    override fun onStart() { super.onStart(); YaacRuntime.visible(true) }
    override fun onResume() { super.onResume(); dev.yaac.android.notify.Notifier.foreground = true }
    override fun onPause() {
        dev.yaac.android.notify.Notifier.foreground = false
        // Not focused: no card is on screen, so every request may be notified.
        controller?.send("shell.visibleRequests", mapOf("cards" to emptyList<Any>()))
        super.onPause()
    }
    override fun onStop() { YaacRuntime.visible(false); super.onStop() }
    override fun onDestroy() {
        YaacRuntime.quitListeners -= quit
        // Without "Keep running in the background", closing the last window ends the client.
        val keep = YaacRuntime.running.values.any { it.store.view("shell")["trayOnClose"].flag(true) }
        if (isFinishing && !keep) YaacRuntime.quit(applicationContext)
        super.onDestroy()
    }
}

private enum class Screen { List, Thread }

@Composable
private fun YaacApp(store: ViewStore, send: (String, Any?) -> Unit, openRequest: androidx.compose.runtime.MutableState<String?>, onHubs: () -> Unit, onRetry: () -> Unit,
                    share: androidx.compose.runtime.MutableState<dev.yaac.android.core.Share?> = mutableStateOf(null)) {
    val shell = store.view("shell")
    YaacTheme(shell) {
        val clipboard: ClipboardManager = LocalClipboardManager.current
        val side = remember(store) { dev.yaac.android.ui.SidePaneState(send) }
        side.shortcuts(store.view("shortcuts"))
        side.composerKey = store.view("composer")["key"].text
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
                is FrameEffect.FilesChange -> side.change(fx.frame)
                is FrameEffect.PasteConfirm -> side.pasteConfirm = fx.frame
                is FrameEffect.ReadPaste -> side.readPaste(fx.frame)
                is FrameEffect.TerminalNotice -> side.notice = fx.text
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
                dev.yaac.android.ui.SettingsScreen(settingsView, navigate, onHubs)
            // On a phone the WSL cards and the status notice sit over the list; the thread shows
            // the status as a banner and the stopped distro in its composer.
            if (screen == Screen.List) ShellNotices(shell, send)
            // Attached to a hub that does not answer (or refused the hello, e.g. another build):
            // say which hub, why, and offer Retry and Hubs….
            val conn = YaacRuntime.connection
            // A build mismatch is not "not connected": the card at the bottom explains it.
            if (conn?.mode == "hub" && shell != null && !shell["hub"]["reachable"].flag(true) && !shell["hub"]["mismatch"].flag())
                HubDown(YaacRuntime.selected?.let { if (YaacRuntime.running.size > 1) "${it.name} (${conn.hub})" else conn.hub } ?: "", shell["status"]["text"].text, onRetry, onHubs)
            // Another build than the hub's: offer the hub's app (core/AppUpdates.kt).
            LaunchedEffect(shell) { dev.yaac.android.core.AppUpdates.observe(shell, BuildConfig.VERSION_NAME) }
            if (conn?.mode == "hub") dev.yaac.android.ui.HubBuildOffer(shell["hub"]["pcUpdate"].obj(), navigate, Modifier.align(Alignment.BottomCenter))
            store.status?.let { s ->
                Box(Modifier.fillMaxWidth().statusBarsPadding().padding(top = 60.dp).align(Alignment.TopCenter), contentAlignment = Alignment.Center) {
                    T(s, LocalType.current.small, c.muted)
                }
            }
        }
        shell["commander"].obj()?.let { CommanderSheet(it, navigate) }
        // Shared from another app: choose the thread (or a new one), then attach and append.
        share.value?.let { sh ->
            var target by remember(sh) { mutableStateOf<String?>(null) }
            val composer = store.view("composer")
            val composerKey = composer["key"].text
            val draftText = composer["localText"]?.str ?: composer["text"].str
            if (target == null) dev.yaac.android.ui.ShareTargetSheet(store.view("sidebar"), sh, onDismiss = { share.value = null }) { id ->
                if (id == null) send("sidebar.new", null) else send("sidebar.open", mapOf("id" to id))
                target = id ?: "draft"
            } else LaunchedEffect(composerKey) {
                // Once the chosen composer is the open one, hand it the share.
                val ready = composerKey != null && (if (target == "draft") composerKey.startsWith("draft") else composerKey == "thread:$target")
                if (ready) {
                    dev.yaac.android.core.shareCommands(composerKey!!, draftText, sh).forEach { (cmd, payload) -> send(cmd, payload) }
                    share.value = null
                    screen = Screen.Thread
                }
            }
        }
        viewer?.let { v -> dev.yaac.android.ui.ImageViewer(v) { viewer = null; send("image.close", null) } }
        side.pasteConfirm?.let { p -> dev.yaac.android.ui.PasteConfirm(p, send) { side.pasteConfirm = null } }
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
        T(reason ?: "The hub does not answer. Check that the PC is on, on the same Tailscale account, with Settings → Android turned on.",
            type.ui, if (reason != null) c.failure else c.muted, modifier = Modifier.padding(top = 4.dp))
        androidx.compose.foundation.layout.Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            dev.yaac.android.ui.PrimaryButton("Retry", onClick = onRetry)
            dev.yaac.android.ui.TextAction("Hubs…", onClick = onChange)
        }
    }
}
