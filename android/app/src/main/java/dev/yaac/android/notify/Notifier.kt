package dev.yaac.android.notify

import android.Manifest
import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.compose.runtime.snapshotFlow
import dev.yaac.android.MainActivity
import dev.yaac.android.R
import dev.yaac.android.core.Controller
import dev.yaac.android.core.flag
import dev.yaac.android.core.get
import dev.yaac.android.core.int
import dev.yaac.android.core.objects
import dev.yaac.android.core.str
import dev.yaac.android.core.text
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject

/**
 * Notifications drawn from the frames (A21):
 *
 * - the ongoing notification of the foreground service: `shell.trayTooltip` ("YAAC · 2 need
 *   you"), Open and Quit; it runs while `shell.trayOnClose` (Keep running in the background) is on;
 * - one high-importance, sticky notification per thread that needs you (P355), with sound when
 *   the count grows, withdrawn as soon as the thread no longer needs you (answered anywhere);
 *   two or more are summarised as "N requests need you" with the three oldest previewed.
 *   Nothing is shown for the thread on screen while the app is in front.
 *
 * Allow once / Deny on a notification needs the request's id and decisions for threads that are
 * not open; the frames do not carry those yet, so a notification offers Open only.
 */
object Notifier {
    const val ONGOING_ID = 1
    private const val SUMMARY_ID = 2
    private const val CH_ONGOING = "yaac.background"
    private const val CH_REQUESTS = "yaac.requests"
    private const val GROUP = "yaac.requests"
    const val EXTRA_OPEN = "dev.yaac.android.OPEN_THREAD"

    private var scope: CoroutineScope? = null
    private var tooltip = "YAAC"
    private var needs = 0
    private val shown = LinkedHashMap<String, Int>()
    @Volatile var foreground = false
    private var serviceOn = false

    fun attach(context: Context, controller: Controller) {
        channels(context)
        val s = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        scope = s
        val store = controller.store
        s.launch {
            snapshotFlow { store.view("shell") }.distinctUntilChanged().collect { shell ->
                tooltip = shell["trayTooltip"].text ?: "YAAC"
                needs = shell["needs"].int ?: 0
                val keep = shell["trayOnClose"].flag(true)
                val service = Intent(context, YaacService::class.java)
                if (keep && shell != null && !serviceOn) {
                    // The service's start runs on the main thread; ask for it once that thread
                    // is idle, so a busy first frame never delays it into an ANR.
                    serviceOn = true
                    android.os.Looper.myQueue().addIdleHandler {
                        if (serviceOn) runCatching { context.startForegroundService(service) }.onFailure { serviceOn = false }
                        false
                    }
                }
                else if (!keep && serviceOn) { context.stopService(service); serviceOn = false }
                if (serviceOn && allowed(context)) manager(context).notify(ONGOING_ID, ongoing(context))
            }
        }
        s.launch {
            snapshotFlow { Triple(store.view("sidebar"), store.view("shell"), foreground) }.distinctUntilChanged()
                .collect { (sidebar, shell, front) ->
                    val feed = shell["requests"] as? JsonObject
                    // The hub's reminder list (P355): exact, nested workers included, without the
                    // cards on screen. Older controllers lack it; then the list's rows stand in.
                    if (feed != null) hubRequests(context, feed) else requests(context, sidebar, shell["threadId"].text, front)
                }
        }
    }

    fun detach(context: Context) {
        scope?.cancel(); scope = null; serviceOn = false
        val m = manager(context)
        shown.values.forEach { m.cancel(it) }; shown.clear()
        m.cancel(SUMMARY_ID); m.cancel(ONGOING_ID)
    }

    fun ongoing(context: Context): Notification = Notification.Builder(context, CH_ONGOING)
        .setSmallIcon(R.drawable.ic_stat_yaac)
        .setContentTitle(tooltip)
        .setContentText(if (needs > 0) "Tap to open the next one" else "Keeping your agents in reach")
        .setOngoing(true).setShowWhen(false).setNumber(needs)
        .setContentIntent(open(context, null, 0))
        .addAction(Notification.Action.Builder(null, "Quit", PendingIntent.getService(context, 1,
            Intent(context, YaacService::class.java).setAction(YaacService.ACTION_QUIT), PendingIntent.FLAG_IMMUTABLE)).build())
        .build()

    private fun requests(context: Context, sidebar: kotlinx.serialization.json.JsonElement?, open: String?, front: Boolean) {
        if (!allowed(context)) return
        val rows = sidebar["sections"].objects.flatMap { it["rows"].objects }
            .filter { it["tone"].text in setOf("needsYou", "needs", "needs you") }
            .distinctBy { it["id"].str }
            .filterNot { front && it["id"].str == open }
        val m = manager(context)
        val ids = rows.map { it["id"].str }.toSet()
        // Withdraw what no longer needs you, wherever it was answered.
        shown.keys.filter { it !in ids }.forEach { m.cancel(shown.remove(it)!!) }
        val grew = rows.any { it["id"].str !in shown }
        for (row in rows) {
            val id = row["id"].str
            val n = shown.getOrPut(id) { 1000 + (shown.size + id.hashCode().and(0xFFFF)) }
            m.notify(n, request(context, row, n, sound = grew && id !in shownSounded).also { shownSounded += id })
        }
        shownSounded.retainAll(ids)
        if (rows.size >= 2) {
            val style = Notification.InboxStyle().setBigContentTitle("${rows.size} requests need you")
            rows.take(3).forEach { style.addLine("${it["title"].str} — ${it["detail"].text ?: it["status"].str}") }
            m.notify(SUMMARY_ID, Notification.Builder(context, CH_REQUESTS).setSmallIcon(R.drawable.ic_stat_yaac)
                .setContentTitle("${rows.size} requests need you").setStyle(style).setGroup(GROUP).setGroupSummary(true)
                .setNumber(rows.size).setContentIntent(open(context, null, 0)).setOnlyAlertOnce(true).build())
        } else m.cancel(SUMMARY_ID)
    }

    private val shownSounded = HashSet<String>()
    private var lastCount = 0

    const val ACTION_DECIDE = "dev.yaac.android.DECIDE"
    const val EXTRA_THREAD = "threadId"
    const val EXTRA_REQUEST = "requestId"
    const val EXTRA_DECISION = "decision"
    const val EXTRA_REVISION = "revision"

    private fun hubRequests(context: Context, feed: JsonObject) {
        if (!allowed(context)) return
        val m = manager(context)
        val revision = feed["revision"].let { (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull() } ?: 0L
        val items = feed["items"].objects
        val keys = items.map { it["threadId"].str + "#" + it["requestId"].str }.toSet()
        shown.keys.filter { it !in keys }.forEach { m.cancel(shown.remove(it)!!) }
        // Sound only when the count grows (P355); updates are silent.
        val sound = items.size > lastCount
        lastCount = items.size
        for (item in items) {
            val key = item["threadId"].str + "#" + item["requestId"].str
            val n = shown.getOrPut(key) { 2000 + (key.hashCode() and 0xFFFFF) }
            m.notify(n, hubRequest(context, item, revision, n, sound))
        }
        if (items.size >= 2) {
            val style = Notification.InboxStyle().setBigContentTitle("${items.size} requests need you")
            items.take(3).forEach { style.addLine(it["detail"].text ?: it["title"].str) }
            m.notify(SUMMARY_ID, Notification.Builder(context, CH_REQUESTS).setSmallIcon(R.drawable.ic_stat_yaac)
                .setContentTitle("${items.size} requests need you").setStyle(style).setGroup(GROUP).setGroupSummary(true)
                .setNumber(items.size).setContentIntent(openRequest(context, items.first(), 0)).setOnlyAlertOnce(!sound).build())
        } else m.cancel(SUMMARY_ID)
    }

    private fun hubRequest(context: Context, item: JsonObject, revision: Long, code: Int, sound: Boolean): Notification {
        val b = Notification.Builder(context, CH_REQUESTS)
            .setSmallIcon(R.drawable.ic_stat_yaac)
            .setContentTitle(item["workerTitle"].text ?: item["title"].str)
            .setContentText(listOfNotNull(item["kind"].text, item["title"].text).joinToString(" · "))
            .setCategory(Notification.CATEGORY_REMINDER)
            .setGroup(GROUP).setAutoCancel(false).setOnlyAlertOnce(!sound)
            .setContentIntent(openRequest(context, item, code))
            .addAction(Notification.Action.Builder(null, "Open", openRequest(context, item, code)).build())
        item["createdAt"].let { (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull() }?.let { b.setWhen(it).setShowWhen(true) }
        // Allow once / Deny only where the hub offers them (a plain allow/deny approval).
        item["decisions"].objects.forEachIndexed { i, d ->
            val intent = Intent(context, DecisionReceiver::class.java).setAction(ACTION_DECIDE)
                .putExtra(EXTRA_THREAD, item["threadId"].str).putExtra(EXTRA_REQUEST, item["requestId"].str)
                .putExtra(EXTRA_DECISION, d["id"].str).putExtra(EXTRA_REVISION, revision)
            val pi = PendingIntent.getBroadcast(context, code * 8 + i + 1, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            b.addAction(Notification.Action.Builder(null, d["label"].str, pi).build())
        }
        return b.build()
    }

    private fun openRequest(context: Context, item: JsonObject, code: Int): PendingIntent = PendingIntent.getActivity(context, code,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
            .putExtra(EXTRA_THREAD, item["threadId"].str).putExtra(EXTRA_REQUEST, item["requestId"].str),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)

    private fun request(context: Context, row: JsonObject, code: Int, sound: Boolean): Notification {
        val where = listOfNotNull(row["providerName"].text, row["line2"]["branch"].text).joinToString(" · ")
        return Notification.Builder(context, CH_REQUESTS)
            .setSmallIcon(R.drawable.ic_stat_yaac)
            .setContentTitle(row["title"].str)
            .setContentText("Needs you" + (row["detail"].text?.let { " — $it" } ?: "") + if (where.isNotEmpty()) " · $where" else "")
            .setCategory(Notification.CATEGORY_REMINDER)
            .setGroup(GROUP).setOngoing(false).setAutoCancel(false).setOnlyAlertOnce(!sound)
            .setContentIntent(open(context, row["id"].str, code))
            .addAction(Notification.Action.Builder(null, "Open", open(context, row["id"].str, code)).build())
            .build()
    }

    private fun open(context: Context, thread: String?, code: Int): PendingIntent = PendingIntent.getActivity(context, code,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
            .apply { if (thread != null) putExtra(EXTRA_OPEN, thread) },
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)

    private fun channels(context: Context) {
        val m = manager(context)
        m.createNotificationChannel(NotificationChannel(CH_ONGOING, "Running in the background", NotificationManager.IMPORTANCE_LOW).apply {
            description = "YAAC keeps the attach to the hub while no window is open"; setShowBadge(true)
        })
        m.createNotificationChannel(NotificationChannel(CH_REQUESTS, "Requests that need you", NotificationManager.IMPORTANCE_HIGH).apply {
            description = "An agent asks for an approval, an answer or a login"; setShowBadge(true)
        })
    }

    @SuppressLint("InlinedApi")
    private fun allowed(context: Context) = context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    private fun manager(context: Context) = context.getSystemService(NotificationManager::class.java)
}
