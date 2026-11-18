package app.kagaz

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.provider.Settings
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@app.tauri.annotation.InvokeArg
class ControlArgs {
    var what: String = ""
    var on: Boolean = false
}

/** TVs on the phone (tvs.rs, tvs.js): Android's screen-sharing consent,
 *  and the phone's own ways to a TV beside Kagaz's: Android's Cast or
 *  Samsung's Smart View, and Samsung DeX. */
@TauriPlugin(permissions = [Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "sound")])
class KagazTvPlugin(private val activity: Activity) : Plugin(activity) {
    override fun load(webView: WebView) {
        KagazTv.attach(activity)
    }

    /** Android's own prompts: first, once, the permission the phone's
     *  sound needs (Android files taking what the phone plays under
     *  recording audio); then, each time, share the screen? Resolves once
     *  the screen is allowed; without the sound's permission the picture
     *  goes alone. */
    @Command
    fun consent(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= 29 && getPermissionState("sound") != PermissionState.GRANTED) {
            requestPermissionForAlias("sound", invoke, "soundAnswered")
        } else {
            askScreen(invoke)
        }
    }

    @PermissionCallback
    fun soundAnswered(invoke: Invoke) {
        askScreen(invoke)
    }

    private fun askScreen(invoke: Invoke) {
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

    private fun switches(): JSObject {
        val o = JSObject()
        o.put("live", KagazTv.live)
        o.put("screenHidden", KagazTv.screenHidden)
        o.put("soundOnTv", KagazTv.soundOnTv)
        o.put("sound", KagazTv.canSound())
        o.put("soundKept", KagazTv.soundKept)
        // Android 13 and later can offer the quick panel's tiles itself.
        o.put("tiles", Build.VERSION.SDK_INT >= 33)
        o.put("floating", KagazTvFloat.wanted(activity) && KagazTvFloat.allowed(activity))
        return o
    }

    /** How the session's switches stand. */
    @Command
    fun controls(invoke: Invoke) {
        invoke.resolve(switches())
    }

    /** Turns one: "screen" (on: shown), "sound" (on: on the TV), "all"
     *  (on: everything shown), or "float" (the floating button; turning it
     *  on opens Android's "Appear on top" screen when that is not allowed
     *  yet, and the switch shows on once it is). */
    @Command
    fun control(invoke: Invoke) {
        val what = invoke.parseArgs(ControlArgs::class.java)
        when (what.what) {
            "screen" -> KagazTv.hideScreen(!what.on)
            "sound" -> KagazTv.soundTo(what.on)
            "all" -> KagazTv.hideAll(!what.on)
            "tiles" -> addTiles(0)
            "float" -> {
                KagazTvFloat.want(activity, what.on)
                if (what.on && !KagazTvFloat.allowed(activity)) {
                    tryOpen(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, android.net.Uri.parse("package:" + activity.packageName)))
                }
            }
            else -> {
                invoke.reject("no switch called ${what.what}")
                return
            }
        }
        invoke.resolve(switches())
    }

    /** Android's own offer to add each tile to the quick panel, one after
     *  the other (Android 13 and later). */
    private fun addTiles(from: Int) {
        if (Build.VERSION.SDK_INT < 33) return
        val tiles = arrayOf(
            Triple(KagazTvAllTile::class.java, "Show on TV", R.drawable.ic_tv_all),
            Triple(KagazTvScreenTile::class.java, "Screen on TV", R.drawable.ic_tv_screen),
            Triple(KagazTvSoundTile::class.java, "Sound on TV", R.drawable.ic_tv_sound),
        )
        if (from >= tiles.size) return
        val (cls, label, icon) = tiles[from]
        try {
            activity.getSystemService(android.app.StatusBarManager::class.java).requestAddTileService(
                ComponentName(activity, cls), label, android.graphics.drawable.Icon.createWithResource(activity, icon),
                activity.mainExecutor
            ) { addTiles(from + 1) }
        } catch (e: Throwable) {
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
