package life.omniverse.kagaz

import android.content.Intent
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import java.lang.ref.WeakReference

class MainActivity : TauriActivity() {
  companion object {
    private var shown: WeakReference<MainActivity>? = null

    /** The call is over: back behind the lock screen (a ring brought the
     *  app over it; it must not stay usable there, open door and all). */
    fun callOver() {
      val activity = shown?.get() ?: return
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
        activity.setShowWhenLocked(false)
        activity.setTurnScreenOn(false)
      }
    }
  }

  /** Rust (main.rs): give this activity the app's window again when the
   *  app was closed while its process stayed (the doorbell line runs on). */
  private external fun kagazShown()

  override fun onCreate(savedInstanceState: Bundle?) {
    KagazNative.attach(this)
    super.onCreate(savedInstanceState)
    shown = WeakReference(this)
    kagazShown()
    // A ring's intent counts once: not again when Android restores the
    // activity, nor when Recents reopens a task that a ring started.
    val fresh = savedInstanceState == null &&
      (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) == 0
    if (fresh) {
      showOverLockScreen(intent)
      fromRing(intent)
    }
    // Android 15 and later draw apps under the status and navigation bars;
    // keep the page clear of them (and of the keyboard when it is up).
    val content = findViewById<View>(android.R.id.content)
    barIcons(resources.configuration)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }

  /** Dark status and navigation icons on the light page, light ones in dark
   *  mode; again when the phone's theme changes while Kagaz is open (the
   *  activity handles that change itself, so it is not recreated). */
  private fun barIcons(config: Configuration) {
    val dark = (config.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    WindowCompat.getInsetsController(window, findViewById<View>(android.R.id.content)).apply {
      isAppearanceLightStatusBars = !dark
      isAppearanceLightNavigationBars = !dark
    }
  }

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    barIcons(newConfig)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    showOverLockScreen(intent)
    fromRing(intent)
  }

  /** Opened from the ring notification: Answer picks up; the card itself
   *  shows the ring. Each only once (the extras are used up). */
  private fun fromRing(intent: Intent?) {
    if (intent == null) return
    if (intent.getBooleanExtra(KagazCallService.EXTRA_ANSWER, false)) {
      KagazCallService.quiet(this)
      KagazNative.send(KagazNative.ANSWER)
    } else if (intent.getBooleanExtra(KagazCallService.EXTRA_CALL, false)) {
      KagazNative.send(KagazNative.OPEN)
    }
    intent.removeExtra(KagazCallService.EXTRA_ANSWER)
    intent.removeExtra(KagazCallService.EXTRA_CALL)
  }

  /** Opened from a ring: show the call even on the lock screen. */
  private fun showOverLockScreen(intent: Intent?) {
    if (intent?.getBooleanExtra(KagazCallService.EXTRA_CALL, false) != true) return
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
      setShowWhenLocked(true)
      setTurnScreenOn(true)
    }
  }
}
