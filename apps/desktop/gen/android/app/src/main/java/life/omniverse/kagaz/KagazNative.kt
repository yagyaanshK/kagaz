package life.omniverse.kagaz

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** The ring notification's buttons and card, handed to the app's Rust code
 *  (main.rs), which holds the call line. */
object KagazNative {
    const val OPEN = 0
    const val ANSWER = 1
    const val DECLINE = 2

    @JvmStatic external fun ring(action: Int)

    /** False when the Rust side is not running (Android restarted only the
     *  service): there is no line, so no call to act on. */
    fun send(action: Int): Boolean = try {
        ring(action)
        true
    } catch (e: Throwable) {
        false
    }
}

/** Decline on the ring notification: this phone stops ringing (the other
 *  devices ring on) without opening Kagaz. */
class KagazRingReceiver : BroadcastReceiver() {
    companion object {
        const val ACTION_DECLINE = "life.omniverse.kagaz.DECLINE"
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION_DECLINE) return
        KagazCallService.ring(context, null)
        KagazNative.send(KagazNative.DECLINE)
    }
}
