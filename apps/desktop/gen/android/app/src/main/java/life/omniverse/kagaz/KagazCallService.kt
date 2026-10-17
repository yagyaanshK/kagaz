package life.omniverse.kagaz

// Keeps the doorbell's call line alive while Kagaz is in the background,
// and rings when a call comes in. The line itself (signalling and media)
// runs in the app's Rust code; this service only keeps the process in the
// foreground so Android does not stop it, and owns the notifications.

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

class KagazCallService : Service() {
    companion object {
        const val LINE_CHANNEL = "kagaz-line"
        const val RING_CHANNEL = "kagaz-ring"
        const val LINE_ID = 1
        const val RING_ID = 2
        const val EXTRA_CALL = "kagaz_call"

        fun channels(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            nm.createNotificationChannel(
                NotificationChannel(LINE_CHANNEL, "Doorbell line", NotificationManager.IMPORTANCE_MIN).apply {
                    description = "Shown while Kagaz is ready to ring for doorbell calls"
                    setShowBadge(false)
                }
            )
            nm.createNotificationChannel(
                NotificationChannel(RING_CHANNEL, "Doorbell calls", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "Rings when someone calls from a door station or monitor"
                    setSound(
                        RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE),
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build()
                    )
                    enableVibration(true)
                    vibrationPattern = longArrayOf(0, 800, 600, 800, 600, 800)
                    lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                }
            )
        }

        private fun openApp(context: Context, call: Boolean): PendingIntent {
            val intent = Intent(context, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
                putExtra(EXTRA_CALL, call)
            }
            return PendingIntent.getActivity(
                context, if (call) 1 else 0, intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
            )
        }

        fun lineNotification(context: Context, text: String): Notification =
            NotificationCompat.Builder(context, LINE_CHANNEL)
                .setSmallIcon(R.mipmap.ic_launcher_foreground)
                .setContentTitle("Kagaz")
                .setContentText(text)
                .setOngoing(true)
                .setPriority(NotificationCompat.PRIORITY_MIN)
                .setContentIntent(openApp(context, false))
                .build()

        /** Ring for `peer`, or (peer null) stop ringing. */
        fun ring(context: Context, peer: String?) {
            channels(context)
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (peer == null) {
                nm.cancel(RING_ID)
                return
            }
            val open = openApp(context, true)
            val n = NotificationCompat.Builder(context, RING_CHANNEL)
                .setSmallIcon(R.mipmap.ic_launcher_foreground)
                .setContentTitle(peer)
                .setContentText("is calling. Tap to answer.")
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setPriority(NotificationCompat.PRIORITY_MAX)
                .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
                .setOngoing(true)
                .setAutoCancel(true)
                .setContentIntent(open)
                .setFullScreenIntent(open, true)
                .build()
            n.flags = n.flags or Notification.FLAG_INSISTENT
            nm.notify(RING_ID, n)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        channels(this)
        val n = lineNotification(this, "Ready for doorbell calls")
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(LINE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(LINE_ID, n)
        }
        return START_STICKY
    }

    override fun onDestroy() {
        ring(this, null)
        super.onDestroy()
    }
}
