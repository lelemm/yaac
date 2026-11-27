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
import dev.yaac.android.core.HubNotifyIds
import dev.yaac.android.core.HubRuntime
import dev.yaac.android.core.HubSummary
import dev.yaac.android.core.YaacRuntime
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
 *
 * Every hub's controller is followed at once (P404). Each hub's notifications carry its id
 * ([EXTRA_HUB]), its own ids and group ([HubNotifyIds]) and, with more than one hub, its name
 * as the subtext; the ongoing notification sums them.
 */

object Notifier {
    const val ONGOING_ID = 1
    private const val CH_ONGOING = "yaac.background"
    private const val CH_REQUESTS = "yaac.requests"
    const val EXTRA_OPEN = "dev.yaac.android.OPEN_THREAD"
    const val EXTRA_HUB = "dev.yaac.android.HUB"

    /** What one hub has on screen in the notification shade. */
    private class Feed(val rt: HubRuntime, val scope: CoroutineScope) {
        val slot get() = rt.hub?.slot ?: 0
        var tooltip = "YAAC"
        var needs = 0
        var keep = true
        val shown = LinkedHashMap<String, Int>()
        val sounded = HashSet<String>()
        var lastCount = 0
    }

    private val feeds = LinkedHashMap<String, Feed>()
    @Volatile var foreground = false
    private var serviceOn = false

    fun attach(context: Context, rt: HubRuntime) {
        channels(context)
        feeds.remove(rt.key)?.let { cancelAll(context, it) }
        val s = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        val feed = Feed(rt, s)
        feeds[rt.key] = feed
        val store = rt.store
        s.launch {
            snapshotFlow { store.view("shell") }.distinctUntilChanged().collect { shell ->
                feed.tooltip = shell["trayTooltip"].text ?: "YAAC"
                feed.needs = shell["needs"].int ?: 0
                feed.keep = shell["trayOnClose"].flag(true)
                if (shell != null) service(context)
            }
        }
        s.launch {
            // The hub's name is in its shell; `foreground` and the selection decide what is on screen.
            snapshotFlow { listOf(store.view("sidebar"), store.view("shell"), foreground, YaacRuntime.selectedKey, YaacRuntime.running.size) }.distinctUntilChanged()
                .collect { _ ->
                    val shell = store.view("shell")
                    val feedObj = shell["requests"] as? JsonObject
                    // The hub's reminder list (P355): exact, nested workers included, without the
                    // cards on screen. Older controllers lack it; then the list's rows stand in.
                    val onScreen = foreground && YaacRuntime.selectedKey == rt.key
                    if (feedObj != null) hubRequests(context, feed, feedObj) else requests(context, feed, store.view("sidebar"), shell["threadId"].text, onScreen)
                }
        }
    }

    fun detach(context: Context, rt: HubRuntime) {
        val feed = feeds[rt.key]?.takeIf { it.rt === rt } ?: return
        feeds.remove(rt.key)
        cancelAll(context, feed)
        service(context)
    }

    private fun cancelAll(context: Context, feed: Feed) {
        feed.scope.cancel()
        val m = manager(context)
        feed.shown.values.forEach { m.cancel(it) }; feed.shown.clear()
        m.cancel(HubNotifyIds.summary(feed.slot))
    }

    /** The foreground service runs while any hub keeps "Keep running in the background" on. */
    private fun service(context: Context) {
        val keep = feeds.values.any { it.keep }
        val service = Intent(context, YaacService::class.java)
        if (keep && !serviceOn) {
            // The service's start runs on the main thread; ask for it once that thread
            // is idle, so a busy first frame never delays it into an ANR.
            serviceOn = true
            android.os.Looper.myQueue().addIdleHandler {
                if (serviceOn) runCatching { context.startForegroundService(service) }.onFailure { serviceOn = false }
                false
            }
        } else if (!keep && serviceOn) { context.stopService(service); serviceOn = false; manager(context).cancel(ONGOING_ID) }
        if (serviceOn && allowed(context)) manager(context).notify(ONGOING_ID, ongoing(context))
    }

    /** Whether the phone has more than one hub: then each notification names its hub. */
    private fun many() = YaacRuntime.running.size > 1

    fun ongoing(context: Context): Notification {
        val (title, lines) = HubSummary.ongoing(feeds.values.map { HubSummary.Entry(it.rt.name, it.needs, it.tooltip) })
        val needs = feeds.values.sumOf { it.needs }
        val b = Notification.Builder(context, CH_ONGOING)
            .setSmallIcon(R.drawable.ic_stat_yaac)
            .setContentTitle(title)
            .setContentText(if (needs > 0) "Tap to open the next one" else "Keeping your agents in reach")
            .setOngoing(true).setShowWhen(false).setNumber(needs)
            .setContentIntent(open(context, null, null, 0))
            .addAction(Notification.Action.Builder(null, "Quit", PendingIntent.getService(context, 1,
                Intent(context, YaacService::class.java).setAction(YaacService.ACTION_QUIT), PendingIntent.FLAG_IMMUTABLE)).build())
        if (lines.isNotEmpty()) b.setStyle(Notification.InboxStyle().setBigContentTitle(title).also { st -> lines.forEach { st.addLine(it) } })
        return b.build()
    }

    private fun requests(context: Context, feed: Feed, sidebar: kotlinx.serialization.json.JsonElement?, open: String?, front: Boolean) {
        if (!allowed(context)) return
        val rows = sidebar["sections"].objects.flatMap { it["rows"].objects }
            .filter { it["tone"].text in setOf("needsYou", "needs", "needs you") }
            .distinctBy { it["id"].str }
            .filterNot { front && it["id"].str == open }
        val m = manager(context)
        val ids = rows.map { it["id"].str }.toSet()
        // Withdraw what no longer needs you, wherever it was answered.
        feed.shown.keys.filter { it !in ids }.forEach { m.cancel(feed.shown.remove(it)!!) }
        val grew = rows.any { it["id"].str !in feed.shown }
        for (row in rows) {
            val id = row["id"].str
            val n = feed.shown.getOrPut(id) { HubNotifyIds.request(feed.slot, id) }
            m.notify(n, request(context, feed, row, n, sound = grew && id !in feed.sounded).also { feed.sounded += id })
        }
        feed.sounded.retainAll(ids)
        summary(context, feed, rows.size, rows.take(3).map { "${it["title"].str} — ${it["detail"].text ?: it["status"].str}" }, open(context, feed.rt.key, null, HubNotifyIds.summary(feed.slot)), sound = false)
    }

    private fun summary(context: Context, feed: Feed, count: Int, lines: List<String>, intent: PendingIntent, sound: Boolean) {
        val m = manager(context)
        val id = HubNotifyIds.summary(feed.slot)
        if (count < 2) { m.cancel(id); return }
        val title = "$count requests need you"
        val style = Notification.InboxStyle().setBigContentTitle(title)
        lines.forEach { style.addLine(it) }
        val b = Notification.Builder(context, CH_REQUESTS).setSmallIcon(R.drawable.ic_stat_yaac)
            .setContentTitle(title).setStyle(style).setGroup(HubNotifyIds.group(feed.slot)).setGroupSummary(true)
            .setNumber(count).setContentIntent(intent).setOnlyAlertOnce(!sound)
        hubText(feed)?.let { b.setSubText(it); style.setSummaryText(it) }
        m.notify(id, b.build())
    }

    /** The hub's name as a notification's subtext, when the phone has more than one hub. */
    private fun hubText(feed: Feed): String? = if (many()) feed.rt.name else null

    const val ACTION_DECIDE = "dev.yaac.android.DECIDE"
    const val EXTRA_THREAD = "threadId"
    const val EXTRA_REQUEST = "requestId"
    const val EXTRA_DECISION = "decision"
    const val EXTRA_REVISION = "revision"

    private fun hubRequests(context: Context, feed: Feed, data: JsonObject) {
        if (!allowed(context)) return
        val m = manager(context)
        val revision = data["revision"].let { (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull() } ?: 0L
        val items = data["items"].objects
        val keys = items.map { it["threadId"].str + "#" + it["requestId"].str }.toSet()
        feed.shown.keys.filter { it !in keys }.forEach { m.cancel(feed.shown.remove(it)!!) }
        // Sound only when the count grows (P355); updates are silent.
        val sound = items.size > feed.lastCount
        feed.lastCount = items.size
        for (item in items) {
            val key = item["threadId"].str + "#" + item["requestId"].str
            val n = feed.shown.getOrPut(key) { HubNotifyIds.request(feed.slot, key) }
            m.notify(n, hubRequest(context, feed, item, revision, n, sound))
        }
        items.firstOrNull()?.let { first ->
            summary(context, feed, items.size, items.take(3).map { it["detail"].text ?: it["title"].str }, openRequest(context, feed, first, HubNotifyIds.summary(feed.slot)), sound)
        } ?: m.cancel(HubNotifyIds.summary(feed.slot))
    }

    private fun hubRequest(context: Context, feed: Feed, item: JsonObject, revision: Long, code: Int, sound: Boolean): Notification {
        val b = Notification.Builder(context, CH_REQUESTS)
            .setSmallIcon(R.drawable.ic_stat_yaac)
            .setContentTitle(item["workerTitle"].text ?: item["title"].str)
            .setContentText(listOfNotNull(item["kind"].text, item["title"].text).joinToString(" · "))
            .setCategory(Notification.CATEGORY_REMINDER)
            .setGroup(HubNotifyIds.group(feed.slot)).setAutoCancel(false).setOnlyAlertOnce(!sound)
            .setContentIntent(openRequest(context, feed, item, code))
            .addAction(Notification.Action.Builder(null, "Open", openRequest(context, feed, item, code)).build())
        hubText(feed)?.let { b.setSubText(it) }
        item["createdAt"].let { (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toLongOrNull() }?.let { b.setWhen(it).setShowWhen(true) }
        // Allow once / Deny only where the hub offers them (a plain allow/deny approval).
        item["decisions"].objects.take(3).forEachIndexed { i, d ->
            val intent = Intent(context, DecisionReceiver::class.java).setAction(ACTION_DECIDE)
                .putExtra(EXTRA_HUB, feed.rt.key)
                .putExtra(EXTRA_THREAD, item["threadId"].str).putExtra(EXTRA_REQUEST, item["requestId"].str)
                .putExtra(EXTRA_DECISION, d["id"].str).putExtra(EXTRA_REVISION, revision)
            val pi = PendingIntent.getBroadcast(context, HubNotifyIds.decision(code, i), intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            b.addAction(Notification.Action.Builder(null, d["label"].str, pi).build())
        }
        return b.build()
    }

    private fun openRequest(context: Context, feed: Feed, item: JsonObject, code: Int): PendingIntent = PendingIntent.getActivity(context, code,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
            .putExtra(EXTRA_HUB, feed.rt.key)
            .putExtra(EXTRA_THREAD, item["threadId"].str).putExtra(EXTRA_REQUEST, item["requestId"].str),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)

    private fun request(context: Context, feed: Feed, row: JsonObject, code: Int, sound: Boolean): Notification {
        val where = listOfNotNull(row["providerName"].text, row["line2"]["branch"].text).joinToString(" · ")
        val b = Notification.Builder(context, CH_REQUESTS)
            .setSmallIcon(R.drawable.ic_stat_yaac)
            .setContentTitle(row["title"].str)
            .setContentText("Needs you" + (row["detail"].text?.let { " — $it" } ?: "") + if (where.isNotEmpty()) " · $where" else "")
            .setCategory(Notification.CATEGORY_REMINDER)
            .setGroup(HubNotifyIds.group(feed.slot)).setOngoing(false).setAutoCancel(false).setOnlyAlertOnce(!sound)
            .setContentIntent(open(context, feed.rt.key, row["id"].str, code))
            .addAction(Notification.Action.Builder(null, "Open", open(context, feed.rt.key, row["id"].str, code)).build())
        hubText(feed)?.let { b.setSubText(it) }
        return b.build()
    }

    private fun open(context: Context, hub: String?, thread: String?, code: Int): PendingIntent = PendingIntent.getActivity(context, code,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
            .apply { if (hub != null) putExtra(EXTRA_HUB, hub); if (thread != null) putExtra(EXTRA_OPEN, thread) },
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
