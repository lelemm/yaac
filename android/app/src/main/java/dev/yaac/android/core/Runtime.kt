package dev.yaac.android.core

import android.content.Context
import android.content.Intent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.yaac.android.BuildConfig

/**
 * What the app attaches to: a hub over Tailscale, the demo, or (development) a controller over
 * TCP. Saved once chosen; intent extras override the saved choice.
 */
data class Connection(val mode: String, val hub: String? = null, val scenario: String = "fleet", val theme: String? = null,
                      val tcpHost: String? = null, val tcpPort: Int = BuildConfig.DEV_PORT) {
    companion object {
        const val DEFAULT_PORT = 47380
        private const val PREFS = "yaac.connection"

        /** `host` or `host:port` (IPv6 in brackets); the hub's attach port when none is given. */
        fun normalize(input: String): String? {
            val s = input.trim()
            if (s.isEmpty() || s.any { it.isWhitespace() }) return null
            val hasPort = if (s.startsWith("[")) s.contains("]:") else s.count { it == ':' } == 1
            if (!hasPort) return if (s.count { it == ':' } > 1 && !s.startsWith("[")) "[$s]:$DEFAULT_PORT" else "$s:$DEFAULT_PORT"
            val port = s.substringAfterLast(':').toIntOrNull() ?: return null
            return if (port in 1..65535) s else null
        }

        fun fromIntent(intent: Intent?): Connection? {
            intent ?: return null
            val theme = intent.getStringExtra("theme")
            if (intent.getStringExtra("transport") == "tcp")
                return Connection("tcp", theme = theme, tcpHost = intent.getStringExtra("host") ?: BuildConfig.DEV_HOST, tcpPort = intent.getIntExtra("port", BuildConfig.DEV_PORT))
            intent.getStringExtra("hubAddress")?.let { return Connection("hub", hub = it, theme = theme) }
            intent.getStringExtra("scenario")?.let { return Connection("demo", scenario = it, theme = theme) }
            return null
        }

        /** The saved mode: "hub" (the hubs list, P404), "demo", or null (the connection screen). */
        fun savedMode(context: Context): String? = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("mode", null)

        /** The one hub an install from before P404 saved (migrated into the hubs list once). */
        fun lastHub(context: Context): String? = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("hub", null)

        fun saveMode(context: Context, mode: String?) {
            val e = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            if (mode == null) e.remove("mode") else e.putString("mode", mode)
            e.apply()
        }
    }
}

/**
 * One attached hub (or the demo, or the development TCP controller): its controller and store.
 * [key] is the hub's id ([Hub.id]), or "demo" / "tcp".
 */
class HubRuntime(val key: String, val hub: Hub?, val connection: Connection, val controller: Controller) {
    val store: ViewStore get() = controller.store
    /** The hub's name: `shell.hub.name` from its hello, else its label, else its host. */
    val name: String get() = hub?.let { HubNames.display(it, store.view("shell")["hub"]["name"].text) } ?: "Demo"
    /** The prefix of this hub's phone-local keys (left-off marks); "" for the pre-P404 hub. */
    val localKey: String get() = if (hub == null || hub.legacy) "" else hub.id + "|"
    val state: HubState get() {
        val shell = store.view("shell")
        return HubState.of(store.stopped, store.status, shell != null, shell["hub"]["reachable"].flag(true))
    }
}

/**
 * The controllers of the process (P404): one per saved hub, all attached at once, or the demo
 * alone. They outlive the activity, so the foreground service keeps every attach (and every
 * request notification) alive while no window is open (A21). The UI shows the [selected] one;
 * the others run hidden (`shell.visible` hidden), so their hubs stream no open thread to them.
 * Main thread only.
 */
object YaacRuntime {
    private const val PREFS = "yaac.hubs"

    /** The saved hubs and the selected one (empty in the demo). */
    var hubs by mutableStateOf(HubList()); private set
    /** The running controllers by key, in the hubs list's order. */
    val running = mutableStateMapOf<String, HubRuntime>()
    var selectedKey by mutableStateOf<String?>(null); private set
    /** Whether the activity is in front (the selected hub is then `shell.visible` shown). */
    var shown = false; private set

    val selected: HubRuntime? get() = selectedKey?.let { running[it] } ?: running.values.firstOrNull()
    /** The selected hub's controller (calls, the browser host and settings use this). */
    val controller: Controller? get() = selected?.controller
    val connection: Connection? get() = selected?.connection
    /** The running hubs in list order. */
    val ordered: List<HubRuntime> get() = hubs.hubs.mapNotNull { running[it.id] }.ifEmpty { running.values.toList() }

    /**
     * The hubs list from SharedPreferences. An install from before P404 that was attached to a
     * hub ([migrate]) brings that hub in once, as the first of the list.
     */
    fun load(context: Context, migrate: Boolean): HubList {
        val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val stored = p.getString("list", null)
        hubs = when {
            stored != null -> HubList.decode(stored)
            migrate -> HubList.migrate(Connection.lastHub(context)).also { save(context, it) }
            else -> HubList()
        }
        return hubs
    }

    private fun save(context: Context, list: HubList) {
        hubs = list
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString("list", list.encode()).apply()
    }

    /** Starts every saved hub that is not running yet, and selects the saved selection. */
    fun startHubs(context: Context) {
        if (running.values.any { it.hub == null }) stopAll(context)
        for (h in hubs.hubs) if (h.id !in running) startOne(context, h.id, h, Connection("hub", hub = h.address))
        select(context, hubs.current?.id ?: return)
    }

    /** The demo or the development controller, alone: every hub's attach ends first. */
    fun startSingle(context: Context, c: Connection): Controller {
        running[c.mode]?.let { if (it.connection == c) { select(context, c.mode); return it.controller } }
        stopAll(context)
        val rt = startOne(context, c.mode, null, c)
        select(context, c.mode)
        return rt.controller
    }

    /** Adds (or selects) a hub, saves the list, attaches to it and selects it. */
    fun addHub(context: Context, address: String, label: String?): Hub? {
        val next = hubs.add(address, label) ?: return null
        save(context, next)
        Connection.saveMode(context, "hub")
        startHubs(context)
        return next.current
    }

    /** Removes a hub: its attach ends (never the hub), its notifications and data go. */
    fun removeHub(context: Context, id: String) {
        val hub = hubs.byId(id) ?: return
        stopOne(context, id)
        save(context, hubs.remove(id))
        if (!hub.legacy) runCatching { java.io.File(dataDir(hub)).deleteRecursively() }
        hubs.current?.let { select(context, it.id) } ?: run { selectedKey = null }
    }

    /**
     * The hub saved as [key] said it moved to another machine (`hub/moved`, P412; its controller
     * shows it as `shell.hub.movedTo`). The saved entry keeps its key, slot, label and data
     * directory and takes the new address; another saved entry that is the same hub (same address
     * or same `hubId`) is merged into it and stops. Then the hub's controller is started again on
     * the new address; it attaches with the normal reattach backoff once the new hub is up. Its
     * notifications keep their ids and routing (both by key and slot).
     */
    fun hubMoved(context: Context, key: String, move: HubMove) {
        val rt = running[key] ?: return
        val result = hubs.applyMoved(key, move) { h -> running[h.id]?.store?.view("shell")["hub"]["id"].text } ?: return
        val wasSelected = selectedKey == key || result.merged.any { it.id == selectedKey }
        save(context, result.list)
        for (gone in result.merged) {
            stopOne(context, gone.id)
            if (!gone.legacy) runCatching { java.io.File(dataDir(gone)).deleteRecursively() }
        }
        val hub = hubs.byId(key) ?: return
        stopOne(context, key)
        startOne(context, key, hub, rt.connection.copy(hub = hub.address))
        if (wasSelected) select(context, key)
    }

    /** Shows [key]'s hub; the one shown before runs on hidden. Remembered. */
    fun select(context: Context, key: String) {
        val rt = running[key] ?: return
        val previous = selected
        selectedKey = key
        if (rt.hub != null && hubs.selected != key) save(context, hubs.select(key))
        if (previous != null && previous !== rt) {
            previous.controller.send("shell.visibleRequests", mapOf("cards" to emptyList<Any>()))
            previous.controller.send("shell.visible", mapOf("shown" to false, "hidden" to true))
        }
        if (shown) rt.controller.send("shell.visible", mapOf("shown" to true, "hidden" to false))
    }

    /** The activity came to the front or left it: only the selected hub is ever shown. */
    fun visible(on: Boolean) {
        shown = on
        selected?.controller?.send("shell.visible", mapOf("shown" to on, "hidden" to !on))
    }

    /** Sends to every running controller (the system theme, for one). */
    fun broadcast(command: String, payload: Any?) = running.values.forEach { it.controller.send(command, payload) }

    /** Retry: a fresh attach to the same hub (a new controller; nothing is re-sent). */
    fun retry(context: Context, key: String) {
        val rt = running[key] ?: return
        val wasSelected = selectedKey == key
        stopOne(context, key)
        startOne(context, key, rt.hub, rt.connection)
        if (wasSelected) select(context, key)
    }

    /** Leaves the demo (or every hub) for the connection screen; the hubs list stays saved. */
    fun disconnect(context: Context) {
        stopAll(context)
        Connection.saveMode(context, null)
    }

    /** Quit: ends this client (never a hub). The activity and the service close themselves. */
    fun quit(context: Context) {
        if (running.isEmpty()) return
        stopAll(context)
        quitListeners.forEach { it() }
    }

    val quitListeners = java.util.concurrent.CopyOnWriteArrayList<() -> Unit>()

    /** A hub's own data directory: distinct per live controller (the controller refuses a shared one). */
    fun dataDir(hub: Hub): String =
        if (hub.legacy) PlatformBridge.dataDir else java.io.File(PlatformBridge.dataDir, "hubs/${hub.slot}").apply { mkdirs() }.absolutePath

    private fun startOne(context: Context, key: String, hub: Hub?, c: Connection): HubRuntime {
        val store = ViewStore()
        val native = c.mode != "tcp" && NativeController.packaged()
        val transport = when {
            c.mode == "tcp" || !native -> TcpTransport(c.tcpHost ?: BuildConfig.DEV_HOST, c.tcpPort)
            c.mode == "hub" -> NativeTransport(jsonOf("mode" to "hub", "hubAddress" to c.hub, "dataDir" to dataDir(hub!!), "theme" to (c.theme ?: "system")).toString())
            // `crates/yaac-android`: demo runs a scenario without a hub.
            else -> NativeTransport(jsonOf("mode" to "demo", "scenario" to c.scenario, "theme" to (c.theme ?: "dark")).toString())
        }
        if (android.util.Log.isLoggable("yaac-dump", android.util.Log.DEBUG))
            store.dump = java.io.File(context.cacheDir, "frames-${hub?.slot ?: key}.ndjson").apply { delete() }
        val ctl = Controller(transport, store)
        val rt = HubRuntime(key, hub, c, ctl)
        running[key] = rt
        if (hub != null) {
            // `hub/moved`: acted on once, after this frame is applied (listeners run on the main thread).
            var moved = false
            store.listeners += { _ ->
                val move = if (moved) null else HubMove.parse(store.view("shell")["hub"]["movedTo"])
                if (move != null) {
                    moved = true
                    android.os.Handler(android.os.Looper.getMainLooper()).post { if (running[key] === rt) hubMoved(context.applicationContext, key, move) }
                }
            }
        }
        ctl.start()
        // The foreground service is this client's tray (P375), so "Keep running in the
        // background" (trayOnClose) applies. Every hub starts hidden until it is selected.
        ctl.send("shell.ready", mapOf("tray" to true))
        ctl.send("shell.visible", mapOf("shown" to false, "hidden" to true))
        systemLight?.let { ctl.send("shell.systemTheme", mapOf("light" to it)) }
        dev.yaac.android.notify.Notifier.attach(context.applicationContext, rt)
        return rt
    }

    /** The system theme as the activity last reported it, for controllers started later. */
    var systemLight: Boolean? = null

    private fun stopOne(context: Context, key: String) {
        val rt = running.remove(key) ?: return
        // A voice call ends with its controller (`call.end` goes before the controller stops).
        CallManager.controllerStopping(rt.controller)
        dev.yaac.android.notify.Notifier.detach(context.applicationContext, rt)
        if (selectedKey == key) selectedKey = null
        rt.controller.stop()
    }

    private fun stopAll(context: Context) {
        running.keys.toList().forEach { stopOne(context, it) }
        selectedKey = null
        context.stopService(Intent(context, dev.yaac.android.notify.YaacService::class.java))
    }
}

/** The selected hub's prefix for phone-local keys (left-off marks), provided by the activity. */
val LocalHubKey = androidx.compose.runtime.staticCompositionLocalOf { "" }
