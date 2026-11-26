package dev.yaac.android.core

import android.content.Context
import android.content.Intent
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

        fun saved(context: Context): Connection? {
            val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            return when (p.getString("mode", null)) {
                "hub" -> p.getString("hub", null)?.let { Connection("hub", hub = it) }
                "demo" -> Connection("demo")
                else -> null
            }
        }

        fun lastHub(context: Context): String? = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("hub", null)

        fun save(context: Context, c: Connection?) {
            val e = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            if (c == null) e.remove("mode") else { e.putString("mode", c.mode); c.hub?.let { e.putString("hub", it) } }
            e.apply()
        }
    }
}

/**
 * The one controller of the process. It outlives the activity, so the foreground service keeps
 * the attach (and every request notification) alive while no window is open (A21). The UI and
 * the service read the same [ViewStore].
 */
object YaacRuntime {
    @Volatile var controller: Controller? = null
        private set
    @Volatile var connection: Connection? = null
        private set

    /** Starts a controller for [c], or returns the running one when it is already that. */
    @Synchronized
    fun start(context: Context, c: Connection): Controller {
        controller?.let { if (connection == c) return it; stopController(context) }
        val store = ViewStore()
        val native = c.mode != "tcp" && NativeController.packaged()
        val transport = when {
            c.mode == "tcp" || !native -> TcpTransport(c.tcpHost ?: BuildConfig.DEV_HOST, c.tcpPort)
            c.mode == "hub" -> NativeTransport(jsonOf("mode" to "hub", "hubAddress" to c.hub, "dataDir" to PlatformBridge.dataDir, "theme" to (c.theme ?: "system")).toString())
            // `crates/yaac-android`: demo runs a scenario without a hub.
            else -> NativeTransport(jsonOf("mode" to "demo", "scenario" to c.scenario, "theme" to (c.theme ?: "dark")).toString())
        }
        if (android.util.Log.isLoggable("yaac-dump", android.util.Log.DEBUG))
            store.dump = java.io.File(context.cacheDir, "frames.ndjson").apply { delete() }
        val ctl = Controller(transport, store)
        controller = ctl
        connection = c
        ctl.start()
        // The foreground service is this client's tray (P375), so "Keep running in the
        // background" (trayOnClose) applies.
        ctl.send("shell.ready", mapOf("tray" to true))
        dev.yaac.android.notify.Notifier.attach(context.applicationContext, ctl)
        return ctl
    }

    private fun stopController(context: Context) {
        val c = controller ?: return
        controller = null
        connection = null
        dev.yaac.android.notify.Notifier.detach(context.applicationContext)
        context.stopService(Intent(context, dev.yaac.android.notify.YaacService::class.java))
        c.stop()
    }

    /** Change hub: ends this attach (never the hub) and leaves the app on the connection screen. */
    @Synchronized
    fun disconnect(context: Context) = stopController(context)

    /** Quit: ends this client (never the hub). The activity and the service close themselves. */
    @Synchronized
    fun quit(context: Context) {
        if (controller == null) return
        stopController(context)
        quitListeners.forEach { it() }
    }

    val quitListeners = java.util.concurrent.CopyOnWriteArrayList<() -> Unit>()
}
