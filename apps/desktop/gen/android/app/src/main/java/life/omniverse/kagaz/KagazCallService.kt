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
import android.database.ContentObserver
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.media.VolumeProvider
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.net.Uri
import java.io.File
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.Settings
import androidx.core.app.NotificationCompat
import androidx.core.app.Person

class KagazCallService : Service() {
    companion object {
        const val LINE_CHANNEL = "kagaz-line"
        // The ring's sound and vibration are Kagaz's own (the person's
        // choice in the doorbell settings, played below), so this channel
        // makes neither; it shows the card and wakes the screen.
        const val RING_CHANNEL = "kagaz-calls"
        private var player: MediaPlayer? = null
        private var vibrator: Vibrator? = null
        private var ringing = false
        const val LINE_ID = 1
        const val RING_ID = 2
        const val EXTRA_CALL = "kagaz_call"
        const val EXTRA_ANSWER = "kagaz_answer"
        const val EXTRA_FROM_APP = "kagaz_from_app"
        const val STOPPED_ID = 3
        private var running: KagazCallService? = null
        private var lineText = "Ready for doorbell calls"

        /** During an answered call the service also holds the microphone,
         *  so the person is still heard after switching to another app.
         *  Android allows that only while Kagaz is on screen, which it is
         *  when a call is answered; otherwise this does nothing. */
        fun microphone(on: Boolean) {
            val service = running ?: return
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return
            var types = ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            if (on) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
            try {
                service.startForeground(LINE_ID, lineNotification(service, lineText), types)
            } catch (e: Exception) {
            }
        }

        fun channels(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            nm.createNotificationChannel(
                NotificationChannel(LINE_CHANNEL, "Doorbell line", NotificationManager.IMPORTANCE_MIN).apply {
                    description = "Shown while Kagaz is ready to ring for doorbell calls"
                    setShowBadge(false)
                }
            )
            // Earlier ones: with the system's sound; with a fixed vibration.
            nm.deleteNotificationChannel("kagaz-ring")
            nm.deleteNotificationChannel("kagaz-ring-tone")
            nm.createNotificationChannel(
                NotificationChannel(RING_CHANNEL, "Doorbell calls", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "Shows who is calling from a door station, a monitor or your family. The ring sound and vibration are set in Kagaz's doorbell settings; they follow silent mode and Do Not Disturb unless this is allowed to override Do Not Disturb."
                    setSound(null, null)
                    enableVibration(false)
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

        /** What the phone's own settings let a ring do now, as (sound,
         *  vibration): silent mode neither, vibrate mode vibration only, and
         *  Do Not Disturb (Bedtime and Sleep modes are one) neither, unless
         *  the person let "Doorbell calls" override it in Android's settings.
         *  The card shows in every case. */
        private fun allowed(context: Context): Pair<Boolean, Boolean> {
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M &&
                nm.currentInterruptionFilter > NotificationManager.INTERRUPTION_FILTER_ALL &&
                !ringsInDnd(context)
            ) return Pair(false, false)
            val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
            return when (audio.ringerMode) {
                AudioManager.RINGER_MODE_NORMAL -> Pair(true, true)
                AudioManager.RINGER_MODE_VIBRATE -> Pair(false, true)
                else -> Pair(false, false)
            }
        }

        /** The person let "Doorbell calls" override Do Not Disturb. */
        fun ringsInDnd(context: Context): Boolean {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return false
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            return nm.getNotificationChannel(RING_CHANNEL)?.canBypassDnd() == true
        }

        /** Vibrate as a phone call does, until stopped. */
        private fun startVibration(context: Context) {
            val v = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                (context.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as VibratorManager).defaultVibrator
            } else {
                @Suppress("DEPRECATION") context.getSystemService(Context.VIBRATOR_SERVICE) as Vibrator
            }
            val pattern = longArrayOf(0, 800, 600)
            val attrs = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE).build()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                v.vibrate(VibrationEffect.createWaveform(pattern, 0), attrs)
            } else {
                @Suppress("DEPRECATION") v.vibrate(pattern, 0, attrs)
            }
            vibrator = v
        }

        private fun stopVibration() {
            vibrator?.cancel()
            vibrator = null
        }

        /** The ring goes quiet (sound and vibration); its card stays. */
        fun quiet(context: Context) {
            stopTone()
            stopVibration()
            Silencer.stop(context)
        }

        /** A volume key quiets a ring, as on a phone call. While ringing,
         *  Kagaz holds a media session whose volume is its own, so a press
         *  reaches it instead of changing a volume. Where Android sends the
         *  key elsewhere anyway, the volume it changed tells the same. */
        private object Silencer {
            private var session: MediaSession? = null
            private var observer: ContentObserver? = null

            fun start(context: Context) {
                stop(context)
                val app = context.applicationContext
                val main = Handler(Looper.getMainLooper())
                val press = Runnable { quiet(app) }
                try {
                    session = MediaSession(app, "kagaz-ring").apply {
                        setPlaybackToRemote(object : VolumeProvider(VolumeProvider.VOLUME_CONTROL_RELATIVE, 1, 1) {
                            override fun onAdjustVolume(direction: Int) { main.post(press) }
                        })
                        setPlaybackState(PlaybackState.Builder().setState(PlaybackState.STATE_PLAYING, 0, 1f).build())
                        isActive = true
                    }
                } catch (e: Exception) {
                    session = null
                }
                val audio = app.getSystemService(Context.AUDIO_SERVICE) as AudioManager
                val streams = intArrayOf(AudioManager.STREAM_RING, AudioManager.STREAM_MUSIC, AudioManager.STREAM_NOTIFICATION, AudioManager.STREAM_ALARM)
                val before = streams.map { audio.getStreamVolume(it) }
                observer = object : ContentObserver(main) {
                    override fun onChange(selfChange: Boolean) {
                        if (streams.map { audio.getStreamVolume(it) } != before) press.run()
                    }
                }.also { app.contentResolver.registerContentObserver(Settings.System.CONTENT_URI, true, it) }
            }

            fun stop(context: Context) {
                session?.release()
                session = null
                observer?.let { context.applicationContext.contentResolver.unregisterContentObserver(it) }
                observer = null
            }
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
                .setSmallIcon(R.drawable.ic_stat_kagaz)
                .setContentTitle("Kagaz")
                .setContentText(text)
                .setOngoing(true)
                .setPriority(NotificationCompat.PRIORITY_MIN)
                .setContentIntent(openApp(context, false))
                .build()

        /** Ring for `peer`, or (peer null) stop ringing. The card is a
         *  call's, as in WhatsApp: tapping it opens the ring in Kagaz without
         *  answering; Decline and Answer act at once. `silent`: the card
         *  only, no `tone` (empty: the phone's ringtone); `vibrate` either way.
         *  The phone's silent, vibrate and Do Not Disturb modes come first. */
        fun ring(context: Context, peer: String?, tone: String = "", silent: Boolean = false, vibrate: Boolean = true) {
            channels(context)
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (peer == null) {
                nm.cancel(RING_ID)
                ringing = false
                quiet(context)
                return
            }
            val app = context.applicationContext
            if (!ringing) {
                // A new ring (the card is redrawn while it rings: only once).
                ringing = true
                val (sound, buzz) = allowed(app)
                val toned = !silent && sound
                val buzzed = vibrate && buzz
                if (toned) startTone(app, tone)
                if (buzzed) startVibration(app)
                if (toned || buzzed) Silencer.start(app)
            }
            val open = openApp(context, true)
            val answer = PendingIntent.getActivity(
                context, 2,
                Intent(context, MainActivity::class.java).apply {
                    flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
                    putExtra(EXTRA_CALL, true)
                    putExtra(EXTRA_ANSWER, true)
                },
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
            )
            val decline = PendingIntent.getBroadcast(
                context, 3,
                Intent(context, KagazRingReceiver::class.java).setAction(KagazRingReceiver.ACTION_DECLINE),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
            )
            // A call's card refuses a caller without a name.
            val caller = Person.Builder().setName(peer.ifBlank { "Doorbell" }).setImportant(true).build()
            val n = NotificationCompat.Builder(context, RING_CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_kagaz)
                .setStyle(NotificationCompat.CallStyle.forIncomingCall(caller, decline, answer))
                .setContentText("Doorbell")
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setPriority(NotificationCompat.PRIORITY_MAX)
                .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                // Rings end within two minutes; should Kagaz be stopped
                // mid-ring, the card must not stay (it cannot be swiped away).
                .setTimeoutAfter(120_000)
                .setContentIntent(open)
                // A call's card must have one (or come from a foreground
                // service); on a locked phone it opens the ring full screen.
                .setFullScreenIntent(open, true)
                .build()
            nm.notify(RING_ID, n)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        channels(this)
        val fromApp = intent?.getBooleanExtra(EXTRA_FROM_APP, false) == true
        running = this
        val n = lineNotification(this, if (fromApp) lineText else "Starting\u2026")
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
                    .setSmallIcon(R.drawable.ic_stat_kagaz)
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
        if (running === this) running = null
        ring(this, null)
        super.onDestroy()
    }
}
