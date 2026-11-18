package app.kagaz

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import androidx.core.content.ContextCompat

/** Runs while the phone's screen goes to a TV: Android (14 and later)
 *  hands over the screen-sharing grant only inside such a service, and
 *  shows its notification, with Stop, all along. */
class KagazCastService : Service() {
    companion object {
        private const val CHANNEL = "kagaz-tv"
        private const val ID = 4207
        private const val STOP = "app.kagaz.TV_STOP"
        private var done: ((String?) -> Unit)? = null

        /** Starts the service with the person's consent; `then` hears null
         *  once the grant is held, else why not. */
        fun begin(context: Context, code: Int, data: Intent, then: (String?) -> Unit) {
            done = then
            val i = Intent(context, KagazCastService::class.java).putExtra("code", code).putExtra("data", data)
            try {
                ContextCompat.startForegroundService(context, i)
            } catch (e: Throwable) {
                done = null
                then(e.message ?: "Android would not start screen sharing")
            }
        }

        fun end(context: Context) {
            try {
                context.stopService(Intent(context, KagazCastService::class.java))
            } catch (e: Throwable) {
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) {
            KagazTv.projectionStopped()
            return START_NOT_STICKY
        }
        val nm = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "Showing on a TV", NotificationManager.IMPORTANCE_LOW))
        }
        val stop = PendingIntent.getService(
            this, 0, Intent(this, KagazCastService::class.java).setAction(STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        val n = builder
            .setSmallIcon(android.R.drawable.ic_menu_upload)
            .setContentTitle("Showing this phone on a TV")
            .setContentText("Kagaz sends the screen while this is here.")
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .build()
        try {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
            } else {
                startForeground(ID, n)
            }
        } catch (e: Throwable) {
            finish(e.message ?: "Android would not start screen sharing")
            stopSelf()
            return START_NOT_STICKY
        }
        val code = intent?.getIntExtra("code", 0) ?: 0
        @Suppress("DEPRECATION")
        val data: Intent? = intent?.getParcelableExtra("data")
        if (data == null) {
            finish("Android gave no screen-sharing grant")
            stopSelf()
            return START_NOT_STICKY
        }
        try {
            val pm = getSystemService(MediaProjectionManager::class.java)
            val mp = pm.getMediaProjection(code, data) ?: throw IllegalStateException("Android gave no screen-sharing grant")
            // Android 14 wants this before the screen is mirrored.
            mp.registerCallback(object : MediaProjection.Callback() {
                override fun onStop() {
                    KagazTv.projectionStopped()
                }
            }, Handler(Looper.getMainLooper()))
            KagazTv.projection = mp
            finish(null)
        } catch (e: Throwable) {
            finish(e.message ?: "Android gave no screen-sharing grant")
            stopSelf()
        }
        return START_NOT_STICKY
    }

    private fun finish(error: String?) {
        val d = done
        done = null
        d?.invoke(error)
    }

    override fun onDestroy() {
        if (Build.VERSION.SDK_INT >= 24) stopForeground(STOP_FOREGROUND_REMOVE) else @Suppress("DEPRECATION") stopForeground(true)
        super.onDestroy()
    }
}
