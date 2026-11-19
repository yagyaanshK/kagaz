package app.kagaz

// After the phone restarts, or Kagaz is updated, nothing of Kagaz runs: the
// doorbell line lives in the app's Rust code, which starts with its window.
// If the line was on, bring it back: with "Appear on top" allowed, Android
// lets Kagaz open its window in the background, and it goes straight
// behind whatever is on screen (MainActivity, EXTRA_BACKGROUND) while the
// line starts as at every launch; without it, a notification asks for a
// tap. A line the person turned off stays off.

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.provider.Settings

class KagazBoot : BroadcastReceiver() {
    companion object {
        const val EXTRA_BACKGROUND = "kagaz_background"
        private const val WANTED = "line_wanted"

        /** The line is on (or off), as KagazPlugin.keepListening hears it. */
        fun wanted(context: Context, on: Boolean) {
            context.getSharedPreferences("kagaz", Context.MODE_PRIVATE).edit().putBoolean(WANTED, on).apply()
        }

        private fun isWanted(context: Context): Boolean =
            context.getSharedPreferences("kagaz", Context.MODE_PRIVATE).getBoolean(WANTED, false)
    }

    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action ?: return
        val updated = action == Intent.ACTION_MY_PACKAGE_REPLACED
        if (!updated && action != Intent.ACTION_BOOT_COMPLETED &&
            action != "android.intent.action.QUICKBOOT_POWERON"
        ) return
        if (!isWanted(context)) return
        val overlay = Build.VERSION.SDK_INT < Build.VERSION_CODES.M || Settings.canDrawOverlays(context)
        if (overlay) {
            try {
                context.startActivity(
                    Intent(context, MainActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
                        .putExtra(EXTRA_BACKGROUND, true)
                )
                return
            } catch (e: Exception) {
            }
        }
        KagazCallService.notListening(
            context,
            "Doorbell isn't listening",
            (if (updated) "Kagaz was updated. " else "The phone restarted. ") + "Tap to listen for doorbell calls again."
        )
    }
}
