package dev.yaac.android.notify

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import dev.yaac.android.MainActivity
import dev.yaac.android.core.YaacRuntime

/**
 * Allow once / Deny on a request notification: `shell.requestAction` with the revision the
 * notification was drawn from. A stale revision is refused by the controller (never re-sent,
 * invariant 10); it then shows why and opens the card. Without a running controller the app
 * opens at the request instead. The decision goes to the hub the notification came from (P404).
 */
class DecisionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val thread = intent.getStringExtra(Notifier.EXTRA_THREAD) ?: return
        val request = intent.getStringExtra(Notifier.EXTRA_REQUEST) ?: return
        val hub = intent.getStringExtra(Notifier.EXTRA_HUB)
        val controller = (if (hub != null) YaacRuntime.running[hub] else YaacRuntime.selected)?.controller
        if (controller == null) {
            context.startActivity(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(Notifier.EXTRA_HUB, hub).putExtra(Notifier.EXTRA_THREAD, thread).putExtra(Notifier.EXTRA_REQUEST, request))
            return
        }
        controller.send("shell.requestAction", mapOf(
            "threadId" to thread, "requestId" to request,
            "decision" to intent.getStringExtra(Notifier.EXTRA_DECISION),
            "revision" to intent.getLongExtra(Notifier.EXTRA_REVISION, 0),
        ))
    }
}
