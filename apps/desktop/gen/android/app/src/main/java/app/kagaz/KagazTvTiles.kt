package app.kagaz

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/** The session's switches as tiles in Android's quick panel, beside Wi-Fi
 *  and Bluetooth: one swipe and one tap from any app. A tile is lit while
 *  what it stands for goes to the TV, and greyed out when nothing is
 *  being mirrored. The person adds them once, from the panel's edit
 *  screen. */
abstract class KagazTvTile : TileService() {
    private val redraw: () -> Unit = { show() }

    /** Lit: this goes to the TV now. */
    abstract fun on(): Boolean
    abstract fun name(): String
    abstract fun state(): String
    abstract fun tap()

    override fun onStartListening() {
        KagazTv.listeners.addIfAbsent(redraw)
        show()
    }

    override fun onStopListening() {
        KagazTv.listeners.remove(redraw)
    }

    override fun onClick() {
        if (KagazTv.live) {
            tap()
            show()
        } else {
            // Nothing to switch: open Kagaz, where mirroring starts.
            val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            try {
                @Suppress("DEPRECATION")
                startActivityAndCollapse(open)
            } catch (e: Throwable) {
            }
        }
    }

    private fun show() {
        val t = qsTile ?: return
        t.label = name()
        if (!KagazTv.live) {
            t.state = Tile.STATE_UNAVAILABLE
            if (Build.VERSION.SDK_INT >= 29) t.subtitle = "Not mirroring"
        } else {
            t.state = if (on()) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
            if (Build.VERSION.SDK_INT >= 29) t.subtitle = state()
        }
        t.updateTile()
    }

    companion object {
        /** Asks Android to draw the tiles again after a change. */
        fun refresh(context: Context) {
            for (c in arrayOf(KagazTvAllTile::class.java, KagazTvScreenTile::class.java, KagazTvSoundTile::class.java)) {
                try {
                    requestListeningState(context, ComponentName(context, c))
                } catch (e: Throwable) {
                }
            }
        }
    }
}

/** Everything: lit while the screen shows and the sound plays on the TV. */
class KagazTvAllTile : KagazTvTile() {
    override fun on() = !KagazTv.anythingHidden
    override fun name() = "Show on TV"
    override fun state() = if (KagazTv.anythingHidden) "Hidden" else "Everything showing"
    override fun tap() = KagazTv.hideAll(!KagazTv.anythingHidden)
}

/** The screen alone. */
class KagazTvScreenTile : KagazTvTile() {
    override fun on() = !KagazTv.screenHidden
    override fun name() = "Screen on TV"
    override fun state() = if (KagazTv.screenHidden) "Hidden" else "Showing"
    override fun tap() = KagazTv.hideScreen(!KagazTv.screenHidden)
}

/** The sound's place. */
class KagazTvSoundTile : KagazTvTile() {
    override fun on() = KagazTv.soundOnTv
    override fun name() = "Sound on TV"
    override fun state() = if (KagazTv.soundOnTv) "On the TV" else "On this phone"
    override fun tap() = KagazTv.soundTo(!KagazTv.soundOnTv)
}
