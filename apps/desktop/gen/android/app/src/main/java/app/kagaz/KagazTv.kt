package app.kagaz

import android.content.Context
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.os.Build
import android.os.Bundle
import android.view.Surface

/** The phone's screen for a TV: Android's own H.264 encoder (MediaCodec)
 *  fed by a virtual display that mirrors the screen, each encoded picture
 *  handed to Kagaz's Rust (tvs.rs), which runs the Miracast session. Rust
 *  starts it once the TV has chosen a size, asks for whole pictures, and
 *  stops it. */
object KagazTv {
    /** Android's screen-sharing grant, from KagazCastService. */
    @Volatile var projection: MediaProjection? = null

    private var app: Context? = null
    private var codec: MediaCodec? = null
    private var display: VirtualDisplay? = null
    private var surface: Surface? = null
    private var drainer: Thread? = null
    @Volatile private var running = false

    /** Rust keeps this class (and its Java VM) to call the methods below. */
    @JvmStatic external fun hello()

    /** An encoded picture (start codes; headers before each whole one). */
    @JvmStatic external fun frame(data: ByteArray, key: Boolean)

    /** The person stopped sharing (the notification, or Android itself). */
    @JvmStatic external fun stopAsked()

    fun attach(context: Context) {
        app = context.applicationContext
        try {
            hello()
        } catch (e: Throwable) {
        }
    }

    /** From Rust, when the TV has chosen: "" once encoding, else why not. */
    @JvmStatic
    fun start(width: Int, height: Int, fps: Int, bitrate: Int): String {
        val mp = projection ?: return "screen sharing was not allowed on the phone yet"
        val context = app ?: return "Kagaz is not ready yet"
        release(false)
        return try {
            val c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            try {
                c.configure(format(width, height, fps, bitrate, true), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            } catch (e: Exception) {
                // Some encoders refuse the profile or the low-delay wishes.
                c.reset()
                c.configure(format(width, height, fps, bitrate, false), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            }
            val s = c.createInputSurface()
            c.start()
            codec = c
            surface = s
            running = true
            display = mp.createVirtualDisplay(
                "Kagaz", width, height, context.resources.displayMetrics.densityDpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, s, null, null
            )
            drainer = Thread({ drain(c) }, "kagaz-tv-encoder").also { it.start() }
            ""
        } catch (e: Throwable) {
            release(false)
            e.message ?: e.toString()
        }
    }

    private fun format(width: Int, height: Int, fps: Int, bitrate: Int, wishes: Boolean): MediaFormat {
        val f = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height)
        f.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
        f.setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
        f.setInteger(MediaFormat.KEY_FRAME_RATE, fps)
        f.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10)
        f.setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR)
        // A still screen still sends a picture now and then.
        f.setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000L)
        if (!wishes) return f
        f.setInteger(
            MediaFormat.KEY_PROFILE,
            if (Build.VERSION.SDK_INT >= 27) MediaCodecInfo.CodecProfileLevel.AVCProfileConstrainedBaseline
            else MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline
        )
        f.setInteger(
            MediaFormat.KEY_LEVEL,
            if (height > 720 && fps > 30) MediaCodecInfo.CodecProfileLevel.AVCLevel42
            else MediaCodecInfo.CodecProfileLevel.AVCLevel4
        )
        f.setInteger(MediaFormat.KEY_PRIORITY, 0)
        if (Build.VERSION.SDK_INT >= 29) {
            f.setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            f.setInteger(MediaFormat.KEY_PREPEND_HEADER_TO_SYNC_FRAMES, 1)
        }
        if (Build.VERSION.SDK_INT >= 30) f.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        return f
    }

    private fun drain(c: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        var headers: ByteArray? = null
        while (running) {
            val i = try {
                c.dequeueOutputBuffer(info, 20_000)
            } catch (e: Throwable) {
                break
            }
            if (i < 0) continue
            val buf = c.getOutputBuffer(i)
            if (buf == null) {
                c.releaseOutputBuffer(i, false)
                continue
            }
            val bytes = ByteArray(info.size)
            buf.position(info.offset)
            buf.get(bytes)
            c.releaseOutputBuffer(i, false)
            if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                headers = bytes
                continue
            }
            val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
            val h = headers
            // The headers before each whole picture, where the encoder did
            // not put them there itself.
            val out = if (key && h != null && !startsWithHeaders(bytes)) h + bytes else bytes
            try {
                frame(out, key)
            } catch (e: Throwable) {
                break
            }
        }
    }

    /** Whether the first unit is a sequence parameter set (type 7). */
    private fun startsWithHeaders(b: ByteArray): Boolean {
        val at = when {
            b.size > 4 && b[0].toInt() == 0 && b[1].toInt() == 0 && b[2].toInt() == 0 && b[3].toInt() == 1 -> 4
            b.size > 3 && b[0].toInt() == 0 && b[1].toInt() == 0 && b[2].toInt() == 1 -> 3
            else -> return false
        }
        return (b[at].toInt() and 0x1f) == 7
    }

    /** From Rust, when the TV wants a whole picture. */
    @JvmStatic
    fun keyFrame() {
        try {
            codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        } catch (e: Throwable) {
        }
    }

    /** From Rust, when the session ends: everything goes, Android's grant
     *  too (a new session asks again, as Android wants). */
    @JvmStatic
    fun stop() {
        release(true)
    }

    @Synchronized
    private fun release(all: Boolean) {
        running = false
        try { drainer?.join(500) } catch (e: Throwable) {}
        drainer = null
        try { display?.release() } catch (e: Throwable) {}
        display = null
        try { codec?.stop() } catch (e: Throwable) {}
        try { codec?.release() } catch (e: Throwable) {}
        codec = null
        try { surface?.release() } catch (e: Throwable) {}
        surface = null
        if (all) {
            val mp = projection
            projection = null
            try { mp?.stop() } catch (e: Throwable) {}
            app?.let { KagazCastService.end(it) }
        }
    }

    /** Android stopped the grant (the person ended it from the status bar). */
    fun projectionStopped() {
        projection = null
        try {
            stopAsked()
        } catch (e: Throwable) {
        }
    }
}
