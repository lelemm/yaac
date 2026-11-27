package dev.yaac.android.notify

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import dev.yaac.android.MainActivity
import dev.yaac.android.R
import dev.yaac.android.core.CallManager
import dev.yaac.android.core.CallPhase

/**
 * A voice call's foreground service (type microphone): it keeps the call, and the microphone,
 * with the screen off or another app in front. Its ongoing notification shows the thread, the
 * call's clock, and Mute and End. [CallManager] starts and stops it.
 */
class CallService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_END -> { CallManager.end(); return START_NOT_STICKY }
            ACTION_MUTE -> { CallManager.mute(!CallManager.muted); return START_NOT_STICKY }
        }
        // startForeground first, always: a service started for the foreground that stops before
        // calling it is an error. A call that already ended then simply lets it go.
        val n = notification(this)
        runCatching {
            if (Build.VERSION.SDK_INT >= 30) startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
            else startForeground(ID, n)
        }.onFailure { CallManager.end(); stopSelf(); return START_NOT_STICKY }
        if (!CallManager.phase.live) stopSelf()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        getSystemService(NotificationManager::class.java).cancel(ID)
        super.onDestroy()
    }

    companion object {
        const val ID = 40
        private const val CHANNEL = "yaac.call"
        const val ACTION_END = "dev.yaac.android.CALL_END"
        const val ACTION_MUTE = "dev.yaac.android.CALL_MUTE"

        /** Redraws the ongoing notification (state, mute) while the call lives. */
        fun update(context: Context) {
            if (context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED && Build.VERSION.SDK_INT >= 33) return
            if (!CallManager.phase.live) return
            context.getSystemService(NotificationManager::class.java).notify(ID, notification(context))
        }

        private fun notification(context: Context): Notification {
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL, "Voice calls", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "A voice call with a thread is running"; setShowBadge(false)
                })
            val phase = CallManager.phase
            val text = when {
                phase == CallPhase.Active && CallManager.muted -> "In a call · muted"
                phase == CallPhase.Active -> "In a call"
                else -> "Connecting…"
            }
            val b = Notification.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_yaac)
                .setContentTitle(CallManager.title.ifBlank { "Voice call" })
                .setContentText(text)
                .setCategory(Notification.CATEGORY_CALL)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setContentIntent(PendingIntent.getActivity(context, 40,
                    Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
                .addAction(Notification.Action.Builder(null, if (CallManager.muted) "Unmute" else "Mute",
                    PendingIntent.getService(context, 41, Intent(context, CallService::class.java).setAction(ACTION_MUTE), PendingIntent.FLAG_IMMUTABLE)).build())
                .addAction(Notification.Action.Builder(null, "End",
                    PendingIntent.getService(context, 42, Intent(context, CallService::class.java).setAction(ACTION_END), PendingIntent.FLAG_IMMUTABLE)).build())
            if (Build.VERSION.SDK_INT >= 31) b.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            CallManager.startedAt?.let { started ->
                // The clock: elapsedRealtime to wall time for the chronometer.
                b.setWhen(System.currentTimeMillis() - (SystemClock.elapsedRealtime() - started)).setUsesChronometer(true).setShowWhen(true)
            }
            return b.build()
        }
    }
}
