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
import android.widget.RemoteViews
import androidx.core.content.ContextCompat

/** Runs while the phone's screen goes to a TV: Android (14 and later)
 *  hands over the screen-sharing grant only inside such a service, and
 *  shows its notification all along. The notification carries the
 *  session's switches, reachable over any app and from the lock screen:
 *  everything, the screen alone, the sound's place, and Stop. Each
 *  switch keeps its own picture (an eye, a TV, a speaker), struck through
 *  while that is kept from the TV; Stop stands apart, in red; a line of
 *  text says how things are. */
class KagazCastService : Service() {
    companion object {
        private const val CHANNEL = "kagaz-tv"
        private const val ID = 4207
        private const val STOP = "app.kagaz.TV_STOP"
        private const val ALL = "app.kagaz.TV_ALL"
        private const val SCREEN = "app.kagaz.TV_SCREEN"
        private const val SOUND = "app.kagaz.TV_SOUND"
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

    private val redraw: () -> Unit = {
        try {
            getSystemService(NotificationManager::class.java).notify(ID, notification())
        } catch (e: Throwable) {
        }
    }

    private fun tap(action: String): PendingIntent = PendingIntent.getService(
        this, 0, Intent(this, KagazCastService::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )

    /** The notification's row: how things stand, the three switches (each
     *  always its own picture, struck through while kept from the TV),
     *  and Stop in red. */
    private fun row(): RemoteViews {
        val hidden = KagazTv.screenHidden
        val onTv = KagazTv.soundOnTv
        val v = RemoteViews(packageName, R.layout.kagaz_tv_notification)
        val sound = when {
            !onTv -> "sound on this phone"
            KagazTv.soundKept -> "this app keeps its sound on the phone"
            else -> "sound on the TV"
        }
        v.setTextViewText(R.id.tv_state, (if (hidden) "Screen hidden, " else "Screen showing, ") + sound)
        v.setImageViewResource(R.id.tv_all, if (KagazTv.anythingHidden) R.drawable.ic_tv_all_off else R.drawable.ic_tv_all)
        v.setContentDescription(R.id.tv_all, if (KagazTv.anythingHidden) "Something is hidden from the TV. Show everything" else "Everything shows on the TV. Hide everything")
        v.setOnClickPendingIntent(R.id.tv_all, tap(ALL))
        v.setImageViewResource(R.id.tv_screen, if (hidden) R.drawable.ic_tv_screen_off else R.drawable.ic_tv_screen)
        v.setContentDescription(R.id.tv_screen, if (hidden) "The screen is hidden from the TV. Show it" else "The screen shows on the TV. Hide it")
        v.setOnClickPendingIntent(R.id.tv_screen, tap(SCREEN))
        v.setImageViewResource(R.id.tv_sound, if (onTv) R.drawable.ic_tv_sound else R.drawable.ic_tv_sound_off)
        v.setContentDescription(R.id.tv_sound, if (onTv) "The sound plays on the TV. Keep it on this phone" else "The sound stays on this phone. Play it on the TV")
        v.setOnClickPendingIntent(R.id.tv_sound, tap(SOUND))
        v.setOnClickPendingIntent(R.id.tv_stop, tap(STOP))
        return v
    }

    private fun notification(): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        builder
            .setSmallIcon(R.drawable.ic_stat_kagaz)
            .setContentTitle("Mirroring to a TV")
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
        if (Build.VERSION.SDK_INT >= 24) {
            // The same row folded and unfolded: the switches are in reach
            // without opening the notification.
            builder.setStyle(Notification.DecoratedCustomViewStyle())
            builder.setCustomContentView(row())
            builder.setCustomBigContentView(row())
        } else {
            builder.setContentText("Open Kagaz for its switches.")
        }
        return builder.build()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            STOP -> {
                KagazTv.projectionStopped()
                return START_NOT_STICKY
            }
            ALL -> {
                KagazTv.hideAll(!KagazTv.anythingHidden)
                return START_NOT_STICKY
            }
            SCREEN -> {
                KagazTv.hideScreen(!KagazTv.screenHidden)
                return START_NOT_STICKY
            }
            SOUND -> {
                KagazTv.soundTo(!KagazTv.soundOnTv)
                return START_NOT_STICKY
            }
        }
        val nm = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "Showing on a TV", NotificationManager.IMPORTANCE_LOW))
        }
        val n = notification()
        KagazTv.listeners.addIfAbsent(redraw)
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
        android.util.Log.i("KagazTv", "screen-sharing grant: ${error ?: "held"}")
        val d = done
        done = null
        d?.invoke(error)
    }

    override fun onDestroy() {
        KagazTv.listeners.remove(redraw)
        if (Build.VERSION.SDK_INT >= 24) stopForeground(STOP_FOREGROUND_REMOVE) else @Suppress("DEPRECATION") stopForeground(true)
        super.onDestroy()
    }
}
