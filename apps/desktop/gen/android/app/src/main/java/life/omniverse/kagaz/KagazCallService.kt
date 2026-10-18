package life.omniverse.kagaz

// Keeps the doorbell's call line alive while Kagaz is in the background or
// closed, and rings when a call comes in. The line itself (signalling and
// media) runs in the app's Rust code, in this process; this service keeps
// the process in the foreground so Android does not stop it (closing the
// app's window keeps the process, see main.rs), and owns the notifications.
// Should Android stop the process anyway and restart only this service,
// the line is gone: the notification then says so instead of claiming to
// listen.

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.net.Uri
import java.io.File
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

class KagazCallService : Service() {
    companion object {
        const val LINE_CHANNEL = "kagaz-line"
        // The ring's sound is Kagaz's own (the person's chosen tone, played
        // below), so this channel is silent; it still vibrates and wakes.
        const val RING_CHANNEL = "kagaz-ring-tone"
        private var player: MediaPlayer? = null
        const val LINE_ID = 1
        const val RING_ID = 2
        const val EXTRA_CALL = "kagaz_call"
        const val EXTRA_FROM_APP = "kagaz_from_app"
        const val STOPPED_ID = 3

        fun channels(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            nm.createNotificationChannel(
                NotificationChannel(LINE_CHANNEL, "Doorbell line", NotificationManager.IMPORTANCE_MIN).apply {
                    description = "Shown while Kagaz is ready to ring for doorbell calls"
                    setShowBadge(false)
                }
            )
            nm.deleteNotificationChannel("kagaz-ring") // the old one, with the system's sound
            nm.createNotificationChannel(
                NotificationChannel(RING_CHANNEL, "Doorbell calls", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "Rings when someone calls from a door station, a monitor or your family"
                    setSound(null, null)
                    enableVibration(true)
                    vibrationPattern = longArrayOf(0, 800, 600, 800, 600, 800)
                    lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                }
            )
        }

        /** Play `tone` (a file; empty for the phone's ringtone) on a loop. */
        private fun startTone(context: Context, tone: String) {
            stopTone()
            val uri = if (tone.isNotEmpty() && File(tone).exists()) Uri.fromFile(File(tone))
                else RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE)
            try {
                player = MediaPlayer().apply {
                    setAudioAttributes(
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build()
                    )
                    setDataSource(context, uri)
                    isLooping = true
                    setOnPreparedListener { it.start() }
                    prepareAsync()
                }
            } catch (e: Exception) {
                stopTone()
            }
        }

        private fun stopTone() {
            try { player?.stop() } catch (_: Exception) {}
            player?.release()
            player = null
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

        /** Ring for `peer` with `tone`, or (peer null) stop ringing. */
        fun ring(context: Context, peer: String?, tone: String = "") {
            channels(context)
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (peer == null) {
                nm.cancel(RING_ID)
                stopTone()
                return
            }
            if (player == null) startTone(context.applicationContext, tone)
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
            nm.notify(RING_ID, n)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        channels(this)
        val fromApp = intent?.getBooleanExtra(EXTRA_FROM_APP, false) == true
        val n = lineNotification(this, if (fromApp) "Ready for doorbell calls" else "Starting\u2026")
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(LINE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(LINE_ID, n)
        }
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (fromApp) {
            nm.cancel(STOPPED_ID)
        } else {
            // Restarted by Android after it stopped the app: the line is not
            // running. Say so, and let a tap start Kagaz (and the line) again.
            nm.notify(
                STOPPED_ID,
                NotificationCompat.Builder(this, LINE_CHANNEL)
                    .setSmallIcon(R.mipmap.ic_launcher_foreground)
                    .setContentTitle("Kagaz stopped listening")
                    .setContentText("Android closed Kagaz. Tap to listen for doorbell calls again.")
                    .setAutoCancel(true)
                    .setContentIntent(openApp(this, false))
                    .build()
            )
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) stopForeground(STOP_FOREGROUND_REMOVE) else @Suppress("DEPRECATION") stopForeground(true)
            stopSelf()
            return START_NOT_STICKY
        }
        return START_STICKY
    }

    override fun onDestroy() {
        ring(this, null)
        super.onDestroy()
    }
}
