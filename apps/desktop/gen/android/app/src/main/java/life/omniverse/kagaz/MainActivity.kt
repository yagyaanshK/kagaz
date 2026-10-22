package life.omniverse.kagaz

import android.app.PendingIntent
import android.app.PictureInPictureParams
import android.app.RemoteAction
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.drawable.Icon
import android.os.Build
import android.os.Bundle
import android.util.Rational
import android.view.View
import androidx.annotation.RequiresApi
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import java.lang.ref.WeakReference

class MainActivity : TauriActivity() {
  companion object {
    private var shown: WeakReference<MainActivity>? = null
    private var calling = false

    /** The window, when there is one, to ask the person something on. */
    fun onScreen(): MainActivity? = shown?.get()

    /** A call began or ended: during one, Home (or Back at Kagaz's top)
     *  shrinks it into a picture-in-picture window, as WhatsApp's calls do. */
    fun inCall(on: Boolean) {
      calling = on
      shown?.get()?.pipReady()
    }

    /** The call is over: back behind the lock screen (a ring brought the
     *  app over it; it must not stay usable there, open door and all), and
     *  its picture-in-picture window goes. */
    fun callOver() {
      val activity = shown?.get() ?: return
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
        activity.setShowWhenLocked(false)
        activity.setTurnScreenOn(false)
      }
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && activity.isInPictureInPictureMode) {
        activity.moveTaskToBack(true)
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
    pipReady()
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

  // ---- Picture-in-picture during a call

  private fun canPip() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
    packageManager.hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)

  /** The call's small window: the door's picture shape, and Hang up. */
  @RequiresApi(Build.VERSION_CODES.O)
  private fun pipParams(): PictureInPictureParams {
    val hangUp = PendingIntent.getBroadcast(
      this, 4,
      Intent(this, KagazRingReceiver::class.java).setAction(KagazRingReceiver.ACTION_HANGUP),
      PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
    )
    val action = RemoteAction(Icon.createWithResource(this, R.drawable.ic_call_end), "Hang up", "Hang up", hangUp)
    val builder = PictureInPictureParams.Builder()
      .setAspectRatio(Rational(4, 3))
      .setActions(listOf(action))
    // Android 12 and later go small by themselves on Home, while a call is on.
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) builder.setAutoEnterEnabled(calling)
    return builder.build()
  }

  private fun pipReady() {
    if (!canPip()) return
    try {
      setPictureInPictureParams(pipParams())
    } catch (e: Exception) {
    }
  }

  private fun enterPip(): Boolean {
    if (!calling || !canPip()) return false
    return try {
      enterPictureInPictureMode(pipParams())
    } catch (e: Exception) {
      false
    }
  }

  /** Home during a call (before Android 12, which does this itself). */
  override fun onUserLeaveHint() {
    super.onUserLeaveHint()
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) enterPip()
  }

  /** Back with nowhere left to go in Kagaz (Tauri hands it here): during a
   *  call the call goes small instead of Kagaz leaving. */
  @Deprecated("Tauri's Back handler calls this")
  @Suppress("DEPRECATION")
  override fun onBackPressed() {
    if (enterPip()) return
    super.onBackPressed()
  }

  /** The page shows only the call while it is in the small window. */
  override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: Configuration) {
    super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
    KagazNative.sendPip(isInPictureInPictureMode)
  }

  // ---- Answers to what the fans' Bluetooth asked (KagazBle)

  override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
    super.onRequestPermissionsResult(requestCode, permissions, grantResults)
    KagazBle.answered(requestCode)
  }

  @Deprecated("KagazBle asks with startActivityForResult")
  @Suppress("DEPRECATION")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    super.onActivityResult(requestCode, resultCode, data)
    KagazBle.answered(requestCode)
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
