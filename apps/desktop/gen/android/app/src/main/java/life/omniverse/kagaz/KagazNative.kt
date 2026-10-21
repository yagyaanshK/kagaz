package life.omniverse.kagaz

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper

/** Kagaz's Rust code (main.rs, mobile.rs) and Android, both ways, without
 *  the Tauri plugin: plugin calls need the app's window, and the doorbell
 *  rings (and signs in again) while the window is closed. */
object KagazNative {
    const val OPEN = 0
    const val ANSWER = 1
    const val DECLINE = 2
    const val HANGUP = 3

    private var app: Context? = null
    private val main = Handler(Looper.getMainLooper())

    /** Rust keeps this class (and its Java VM) to call the methods below. */
    @JvmStatic external fun hello()

    /** Called by MainActivity before the app starts, so Rust can call in
     *  from the beginning. */
    fun attach(context: Context) {
        app = context.applicationContext
        try {
            Rust.hashCode() // loads the library, as the activity would next
            hello()
        } catch (e: Throwable) {
        }
    }

    // ---- Android to Rust: the ring notification's card and buttons.

    @JvmStatic external fun ring(action: Int)

    /** The call is in (or out of) a picture-in-picture window. */
    @JvmStatic external fun pip(on: Boolean)

    fun sendPip(on: Boolean) {
        try {
            pip(on)
        } catch (e: Throwable) {
        }
    }

    /** False when the Rust side is not running (Android restarted only the
     *  service): there is no line, so no call to act on. */
    fun send(action: Int): Boolean = try {
        ring(action)
        true
    } catch (e: Throwable) {
        false
    }

    // ---- Rust to Android, from Rust's own threads.

    /** A call's phase changed: ring for `peer` while "ringing", stop
     *  otherwise; once it is over ("idle"), the lock screen is back. */
    @JvmStatic
    fun callChanged(phase: String, peer: String, tone: String, silent: Boolean, vibrate: Boolean) {
        val context = app ?: return
        main.post {
            KagazCallService.ring(context, if (phase == "ringing") peer else null, tone, silent, vibrate)
            val talking = phase != "idle" && phase != "ringing"
            KagazCallService.microphone(talking)
            MainActivity.inCall(talking)
            if (phase == "idle") MainActivity.callOver()
        }
    }

    @JvmStatic fun secretGet(name: String): String? = KagazSecrets.get(app!!, name)

    @JvmStatic fun secretSet(name: String, value: String) = KagazSecrets.set(app!!, name, value)

    @JvmStatic fun secretDelete(name: String) = KagazSecrets.delete(app!!, name)
}

/** Decline on the ring notification: this phone stops ringing (the other
 *  devices ring on) without opening Kagaz. */
class KagazRingReceiver : BroadcastReceiver() {
    companion object {
        const val ACTION_DECLINE = "life.omniverse.kagaz.DECLINE"
        /** Hang up, from the call's picture-in-picture window. */
        const val ACTION_HANGUP = "life.omniverse.kagaz.HANGUP"
    }

    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            ACTION_DECLINE -> {
                KagazCallService.ring(context, null)
                KagazNative.send(KagazNative.DECLINE)
            }
            ACTION_HANGUP -> KagazNative.send(KagazNative.HANGUP)
        }
    }
}
