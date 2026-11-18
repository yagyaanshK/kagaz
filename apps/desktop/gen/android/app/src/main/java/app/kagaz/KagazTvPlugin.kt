package app.kagaz

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.provider.Settings
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/** TVs on the phone (tvs.rs, tvs.js): Android's screen-sharing consent,
 *  and the phone's own ways to a TV beside Kagaz's: Android's Cast or
 *  Samsung's Smart View, and Samsung DeX. */
@TauriPlugin
class KagazTvPlugin(private val activity: Activity) : Plugin(activity) {
    override fun load(webView: WebView) {
        KagazTv.attach(activity)
    }

    /** Android's own prompt: share the screen? Resolves once allowed. */
    @Command
    fun consent(invoke: Invoke) {
        val pm = activity.getSystemService(MediaProjectionManager::class.java)
        startActivityForResult(invoke, pm.createScreenCaptureIntent(), "consented")
    }

    @ActivityCallback
    fun consented(invoke: Invoke, result: ActivityResult) {
        val data = result.data
        if (result.resultCode != Activity.RESULT_OK || data == null) {
            invoke.reject("Screen sharing was not allowed")
            return
        }
        KagazCastService.begin(activity, result.resultCode, data) { error ->
            activity.runOnUiThread {
                if (error == null) invoke.resolve(JSObject()) else invoke.reject(error)
            }
        }
    }

    private fun tryOpen(vararg intents: Intent): Boolean {
        for (i in intents) {
            try {
                activity.startActivity(i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return true
            } catch (e: Throwable) {
            }
        }
        return false
    }

    /** The phone's own way to a TV: Samsung's Smart View, or Android's Cast
     *  screen. Says which opened. */
    @Command
    fun systemCast(invoke: Invoke) {
        val samsung = Build.MANUFACTURER.equals("samsung", ignoreCase = true)
        val ret = JSObject()
        val smartView = Intent().setComponent(
            ComponentName("com.samsung.android.smartmirroring", "com.samsung.android.smartmirroring.CaptureActivity")
        )
        val opened = when {
            samsung && tryOpen(smartView) -> "Samsung's Smart View is open: pick the TV there."
            tryOpen(Intent("android.settings.CAST_SETTINGS")) -> "The phone's Cast screen is open: pick the TV there (turn on Wireless display at the top for Miracast TVs)."
            tryOpen(Intent("android.settings.WIFI_DISPLAY_SETTINGS")) -> "Android's wireless display screen is open: pick the TV there."
            samsung -> "Open Smart View from the quick panel (swipe down twice), then pick the TV."
            else -> "Open Cast or Screen Cast from the quick panel (swipe down twice), then pick the TV."
        }
        ret.put("said", opened)
        invoke.resolve(ret)
    }

    /** Samsung DeX: a desktop on the TV. Opens DeX's own start screen
     *  (what its quick-panel tile opens), else its settings. */
    @Command
    fun dex(invoke: Invoke) {
        val ret = JSObject()
        val samsung = Build.MANUFACTURER.equals("samsung", ignoreCase = true)
        if (!samsung) {
            ret.put("said", "DeX is Samsung's; this phone does not have it.")
            invoke.resolve(ret)
            return
        }
        val pkg = "com.sec.android.desktopmode.uiservice"
        val toggle = Intent(Intent.ACTION_VIEW).setComponent(ComponentName(pkg, "$pkg.activity.ModeToggleActivity"))
        val settings = Intent().setComponent(ComponentName(pkg, "$pkg.activity.settings.SettingsActivity"))
        val linkNote = " DeX needs Link to Windows turned off (Settings, Connected devices)."
        ret.put(
            "said",
            when {
                tryOpen(toggle) -> "DeX is starting: pick the TV on the phone." + linkNote
                tryOpen(settings) -> "DeX settings are open." + linkNote
                else -> "Start DeX from the quick panel (swipe down twice, DeX)." + linkNote
            }
        )
        invoke.resolve(ret)
    }
}
