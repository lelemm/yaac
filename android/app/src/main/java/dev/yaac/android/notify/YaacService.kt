package dev.yaac.android.notify

import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import dev.yaac.android.core.YaacRuntime

/**
 * "Keep running in the background" (the desktop tray, P375): a foreground service whose ongoing
 * notification is the tray icon, "YAAC · N need you", with Quit. It holds the process, and so
 * the controller and its attach, while no window is open.
 */
class YaacService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_QUIT) {
            YaacRuntime.quit(this)
            stopSelf()
            return START_NOT_STICKY
        }
        val notification = Notifier.ongoing(this)
        if (Build.VERSION.SDK_INT >= 34) startForeground(Notifier.ONGOING_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(Notifier.ONGOING_ID, notification)
        return START_NOT_STICKY
    }

    companion object { const val ACTION_QUIT = "dev.yaac.android.QUIT" }
}
