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
            samsung && tryOpen(smartView) -> "Smart View is open: pick the TV there."
            tryOpen(Intent("android.settings.CAST_SETTINGS")) -> "Android's Cast screen is open: pick the TV there."
            tryOpen(Intent("android.settings.WIFI_DISPLAY_SETTINGS")) -> "Android's wireless display screen is open: pick the TV there."
            samsung -> "Open Smart View from the quick panel (swipe down twice), then pick the TV."
            else -> "Open Cast or Screen Cast from the quick panel (swipe down twice), then pick the TV."
        }
        ret.put("said", opened)
        invoke.resolve(ret)
    }

    /** Samsung DeX: a desktop on the TV, through Smart View. */
    @Command
    fun dex(invoke: Invoke) {
        val ret = JSObject()
        val samsung = Build.MANUFACTURER.equals("samsung", ignoreCase = true)
        if (!samsung) {
            ret.put("said", "DeX is Samsung's; this phone does not have it.")
            invoke.resolve(ret)
            return
        }
        val settings = Intent().setComponent(
            ComponentName("com.sec.android.desktopmode.uiservice", "com.sec.android.desktopmode.uiservice.activity.settings.SettingsActivity")
        )
        val opened = tryOpen(settings)
        ret.put(
            "said",
            if (opened) "DeX settings are open. To start DeX on the TV: quick panel, DeX, then pick the TV."
            else "Start DeX from the quick panel (swipe down twice, DeX), then pick the TV."
        )
        invoke.resolve(ret)
    }
}
