package dev.yaac.android.core

import android.content.Context
import dev.yaac.android.ui.transcript.SeenPoint
import dev.yaac.android.ui.transcript.SeenStore

/*
 * "Unread from here" on this phone (docs/07-ui.md#unread-from-here-on-android, P402): the toggle
 * (Settings → Mark where I left off, on by default) and the last seen item of the last 200
 * threads, both in SharedPreferences, never in the hub. Seen points change in memory as the user
 * reads and are written when a thread is left or the app stops.
 */
object LeftOff {
    private const val PREFS = "yaac.leftOff"
    private const val ENABLED = "enabled"
    private const val SEEN = "seen"

    private var store: SeenStore? = null
    private var dirty = false

    fun enabled(context: Context): Boolean = prefs(context).getBoolean(ENABLED, true)

    fun setEnabled(context: Context, on: Boolean) {
        prefs(context).edit().putBoolean(ENABLED, on).apply()
    }

    @Synchronized
    fun seen(context: Context, threadId: String): SeenPoint? = load(context)[threadId]

    @Synchronized
    fun record(context: Context, threadId: String, point: SeenPoint) {
        if (load(context).put(threadId, point)) dirty = true
    }

    /** Writes the seen points if they changed. */
    @Synchronized
    fun flush(context: Context) {
        val s = store ?: return
        if (!dirty) return
        dirty = false
        prefs(context).edit().putString(SEEN, s.encode()).apply()
    }

    private fun load(context: Context): SeenStore =
        store ?: SeenStore.decode(prefs(context).getString(SEEN, null)).also { store = it }

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
