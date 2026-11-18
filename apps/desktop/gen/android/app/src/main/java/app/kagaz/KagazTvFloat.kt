package app.kagaz

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.WindowManager
import android.widget.ImageView
import android.widget.LinearLayout
import kotlin.math.abs

/** The session's switches as a small button that floats over every app,
 *  as Samsung's Smart View and screen recorder have: a tap opens it into
 *  a strip (everything, the screen, the sound, Stop), a drag moves it.
 *  Only while mirroring, only when the person turned it on in Kagaz and
 *  allowed "Appear on top" in Android's settings. It is part of the
 *  screen, so the TV shows it too. */
@SuppressLint("StaticFieldLeak")
object KagazTvFloat {
    private const val PREFS = "kagaz-tv"
    private const val WANTED = "float"
    private val main = Handler(Looper.getMainLooper())
    private var strip: LinearLayout? = null
    private var open = false
    private val redraw: () -> Unit = { main.post { app?.let { sync(it) } } }
    private var app: Context? = null

    fun allowed(context: Context): Boolean = Build.VERSION.SDK_INT < 23 || Settings.canDrawOverlays(context)

    fun wanted(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(WANTED, false)

    fun want(context: Context, on: Boolean) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(WANTED, on).apply()
        main.post { sync(context.applicationContext) }
    }

    /** Hears of every change from now on. */
    fun attach(context: Context) {
        app = context.applicationContext
        KagazTv.listeners.addIfAbsent(redraw)
    }

    private fun dp(context: Context, n: Int) = (n * context.resources.displayMetrics.density + 0.5f).toInt()

    /** Shows, hides or redraws the button to fit how things stand. */
    private fun sync(context: Context) {
        val should = KagazTv.live && wanted(context) && allowed(context)
        val wm = context.getSystemService(WindowManager::class.java) ?: return
        val had = strip
        if (!should) {
            if (had != null) {
                try { wm.removeView(had) } catch (e: Throwable) {}
                strip = null
                open = false
            }
            return
        }
        if (had == null) {
            val v = LinearLayout(context)
            v.orientation = LinearLayout.VERTICAL
            val at = WindowManager.LayoutParams(
                WindowManager.LayoutParams.WRAP_CONTENT, WindowManager.LayoutParams.WRAP_CONTENT,
                if (Build.VERSION.SDK_INT >= 26) WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
                else @Suppress("DEPRECATION") WindowManager.LayoutParams.TYPE_PHONE,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
                PixelFormat.TRANSLUCENT
            )
            at.gravity = Gravity.TOP or Gravity.START
            at.x = context.resources.displayMetrics.widthPixels - dp(context, 64)
            at.y = context.resources.displayMetrics.heightPixels / 3
            try {
                wm.addView(v, at)
            } catch (e: Throwable) {
                return
            }
            strip = v
        }
        fill(context, strip!!)
    }

    private fun round(context: Context, fill: Int): GradientDrawable {
        val g = GradientDrawable()
        g.cornerRadius = dp(context, 28).toFloat()
        g.setColor(fill)
        return g
    }

    /** One round button of the strip. */
    private fun button(context: Context, icon: Int, says: String, fill: Int, tap: (() -> Unit)?): ImageView {
        val b = ImageView(context)
        b.setImageResource(icon)
        b.setColorFilter(0xFFFFFFFF.toInt())
        b.background = round(context, fill)
        val pad = dp(context, 12)
        b.setPadding(pad, pad, pad, pad)
        b.contentDescription = says
        val lp = LinearLayout.LayoutParams(dp(context, 48), dp(context, 48))
        lp.setMargins(dp(context, 4), dp(context, 4), dp(context, 4), dp(context, 4))
        b.layoutParams = lp
        if (tap != null) b.setOnClickListener { tap() }
        return b
    }

    /** The strip: Kagaz's own mark as the handle (it opens the strip, and
     *  drags), then the switches, each always its own picture (an eye, a
     *  TV, a speaker), struck through while that is kept from the TV, and
     *  Stop apart in red. Nothing swaps places or pictures on a tap. */
    @SuppressLint("ClickableViewAccessibility")
    private fun fill(context: Context, v: LinearLayout) {
        v.removeAllViews()
        v.background = round(context, 0xE61B211D.toInt())
        val quiet = 0xFF374B3E.toInt()
        val handle = button(
            context, R.drawable.ic_stat_kagaz,
            if (open) "Close Kagaz's mirroring switches" else "Kagaz's mirroring switches", 0xFF2F6F4E.toInt(), null
        )
        val slop = ViewConfiguration.get(context).scaledTouchSlop
        var downX = 0f
        var downY = 0f
        var startX = 0
        var startY = 0
        var moved = false
        handle.setOnTouchListener { _, e ->
            val wm = context.getSystemService(WindowManager::class.java)
            val at = v.layoutParams as WindowManager.LayoutParams
            when (e.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = e.rawX; downY = e.rawY; startX = at.x; startY = at.y; moved = false
                }
                MotionEvent.ACTION_MOVE -> {
                    if (moved || abs(e.rawX - downX) > slop || abs(e.rawY - downY) > slop) {
                        moved = true
                        at.x = startX + (e.rawX - downX).toInt()
                        at.y = startY + (e.rawY - downY).toInt()
                        try { wm.updateViewLayout(v, at) } catch (t: Throwable) {}
                    }
                }
                MotionEvent.ACTION_UP -> if (!moved) {
                    open = !open
                    fill(context, v)
                }
            }
            true
        }
        v.addView(handle)
        if (!open) return
        val hiddenSome = KagazTv.anythingHidden
        v.addView(
            button(
                context, if (hiddenSome) R.drawable.ic_tv_all_off else R.drawable.ic_tv_all,
                if (hiddenSome) "Something is hidden from the TV. Show everything" else "Everything shows on the TV. Hide everything", quiet
            ) { KagazTv.hideAll(!KagazTv.anythingHidden) }
        )
        v.addView(
            button(
                context, if (KagazTv.screenHidden) R.drawable.ic_tv_screen_off else R.drawable.ic_tv_screen,
                if (KagazTv.screenHidden) "The screen is hidden from the TV. Show it" else "The screen shows on the TV. Hide it", quiet
            ) { KagazTv.hideScreen(!KagazTv.screenHidden) }
        )
        if (KagazTv.canSound()) {
            v.addView(
                button(
                    context, if (KagazTv.soundOnTv) R.drawable.ic_tv_sound else R.drawable.ic_tv_sound_off,
                    if (KagazTv.soundOnTv) "The sound plays on the TV. Keep it on this phone" else "The sound stays on this phone. Play it on the TV", quiet
                ) { KagazTv.soundTo(!KagazTv.soundOnTv) }
            )
        }
        v.addView(button(context, R.drawable.ic_tv_stop, "Stop mirroring", 0xFFC62828.toInt()) { KagazTv.projectionStopped() })
    }
}
