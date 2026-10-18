package life.omniverse.kagaz

import android.content.Intent
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  /** Rust (main.rs): give this activity the app's window again when the
   *  app was closed while its process stayed (the doorbell line runs on). */
  private external fun kagazShown()

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    kagazShown()
    showOverLockScreen(intent)
    // Android 15 and later draw apps under the status and navigation bars;
    // keep the page clear of them (and of the keyboard when it is up).
    val content = findViewById<View>(android.R.id.content)
    // Dark status and navigation icons on the light page, light ones in dark mode.
    val dark = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    WindowCompat.getInsetsController(window, content).apply {
      isAppearanceLightStatusBars = !dark
      isAppearanceLightNavigationBars = !dark
    }
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    showOverLockScreen(intent)
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
