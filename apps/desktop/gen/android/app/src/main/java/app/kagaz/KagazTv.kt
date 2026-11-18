package app.kagaz

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.Surface

/** The phone's screen for a TV: Android's own H.264 encoder (MediaCodec)
 *  fed by a virtual display that mirrors the screen, each encoded picture
 *  handed to Kagaz's Rust (tvs.rs), which runs the session with the TV.
 *  Rust starts it once the TV has chosen a size, asks for whole pictures,
 *  and stops it.
 *
 *  The phone's sound goes beside the pictures (Android 10 and later):
 *  what the phone is playing, taken with the same screen-sharing grant,
 *  encoded as Opus, 20 ms a packet. Packets go all the time, silence when
 *  the sound stays on the phone or cannot be taken, so the TV's player
 *  never waits for sound. While the sound is on the TV the phone's own
 *  speaker is kept quiet: its media volume is turned to nothing (what
 *  Android hands over is not touched by that; measured on a Galaxy S10)
 *  and put back after. Some apps forbid taking their sound (music
 *  services, calls): Android hands over silence for them, and gives
 *  their sound only to its own Cast. After four seconds of that while
 *  something plays, the phone's volume is put back, so such an app is
 *  heard on the phone; the moment real sound comes, the phone goes quiet
 *  again.
 *
 *  Two switches, for privacy: the screen hidden (the TV gets a blur, see
 *  KagazTvPicture) and the sound's place (the TV, or the phone). Every
 *  session starts with the screen hidden and the sound on the phone, so
 *  nothing reaches the TV until the person shows it. The notification,
 *  the quick panel's tiles, the floating button and the app's own page
 *  all turn these same switches and hear of each change.
 *
 *  The volume keys stay the phone's own, and the TV's volume its
 *  remote's. A press up while the phone is kept quiet raises the phone's
 *  volume a notch; Kagaz turns it back to nothing within a tenth of a
 *  second (it looks ten times a second, no more). */
object KagazTv {
    /** Android's screen-sharing grant, from KagazCastService. */
    @Volatile var projection: MediaProjection? = null

    private var app: Context? = null
    private var codec: MediaCodec? = null
    private var display: VirtualDisplay? = null
    private var surface: Surface? = null
    private var drainer: Thread? = null
    @Volatile private var running = false
    private var sounder: Thread? = null
    private var opus: String? = null
    private var opusLooked = false

    private var picture: KagazTvPicture? = null

    /** The TV gets a blur in place of the screen. */
    @Volatile var screenHidden = true
        private set
    /** Where the phone's sound plays: on the TV, or on the phone. */
    @Volatile var soundOnTv = false
        private set
    /** The phone's screen is going to a TV now. */
    val live: Boolean get() = running
    /** Something is kept from the TV, the screen or the sound. */
    val anythingHidden: Boolean get() = screenHidden || !soundOnTv
    /** Told of every change of the switches, and of a session's start and end. */
    val listeners = java.util.concurrent.CopyOnWriteArrayList<() -> Unit>()
    /** The phone's sound is being taken (the grant and the permission are there). */
    @Volatile private var capturing = false
    /** What is taken is real sound, not the silence Android hands over
     *  for an app that forbids it (or when nothing plays). */
    @Volatile private var heard = false
    /** The sound is wanted on the TV, something plays, and Android hands
     *  over only silence: the app playing keeps its sound to the phone. */
    @Volatile var soundKept = false
        private set
    /** The phone's media volume before Kagaz turned it to nothing, to
     *  put back after; -1 while Kagaz has not touched it. */
    private var volumeBefore = -1
    private val soundLock = Any()
    /** When Kagaz itself last touched the phone's volume. */

    /** Rust keeps this class (and its Java VM) to call the methods below. */
    @JvmStatic external fun hello()

    /** An encoded picture (start codes; headers before each whole one). */
    @JvmStatic external fun frame(data: ByteArray, key: Boolean)

    /** The person stopped sharing (the notification, or Android itself). */
    @JvmStatic external fun stopAsked()

    /** One Opus packet of the phone's sound. */
    @JvmStatic external fun sound(data: ByteArray)

    /** The switches changed, or a session began or ended (for the app's page). */
    @JvmStatic external fun changed(live: Boolean, screenHidden: Boolean, soundOnTv: Boolean, soundKept: Boolean)

    fun hideScreen(hide: Boolean) = set(hide, soundOnTv)

    /** The sound's place: on the TV (the phone's speaker muted) or on the
     *  phone (the TV gets silence). */
    fun soundTo(tv: Boolean) = set(screenHidden, tv)

    /** Both at once: the screen hidden and the sound on the phone, or the
     *  screen shown and the sound on the TV. */
    fun hideAll(hide: Boolean) = set(hide, !hide)

    private fun set(hide: Boolean, tv: Boolean) {
        screenHidden = hide
        soundOnTv = tv
        picture?.hidden = hide
        app?.let { applySound(it) }
        announce()
    }

    private fun announce() {
        try {
            changed(running, screenHidden, soundOnTv, soundKept)
        } catch (e: Throwable) {
        }
        for (l in listeners) {
            try {
                l()
            } catch (e: Throwable) {
            }
        }
    }

    /** From Rust, before a session: whether sound will come with the
     *  pictures (Android 10 and an Opus encoder on the phone). */
    @JvmStatic
    fun canSound(): Boolean {
        if (Build.VERSION.SDK_INT < 29) return false
        if (!opusLooked) {
            opusLooked = true
            opus = try {
                MediaCodecList(MediaCodecList.REGULAR_CODECS)
                    .findEncoderForFormat(MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48000, 2))
            } catch (e: Throwable) {
                null
            }
            Log.i("KagazTv", "Opus encoder: ${opus ?: "none"}")
        }
        return opus != null
    }

    fun attach(context: Context) {
        val c = context.applicationContext
        if (app == null) {
            // The tiles and the floating button follow every change.
            listeners.addIfAbsent { KagazTvTile.refresh(c) }
            KagazTvFloat.attach(c)
        }
        app = c
        try {
            hello()
        } catch (e: Throwable) {
        }
    }

    /** From Rust, when the TV has chosen: "" once encoding, else why not. */
    @JvmStatic
    fun start(width: Int, height: Int, fps: Int, bitrate: Int): String {
        Log.i("KagazTv", "start ${width}x$height at $fps, $bitrate bit/s; grant held: ${projection != null}")
        val mp = projection ?: return "screen sharing was not allowed on the phone yet"
        val context = app ?: return "Kagaz is not ready yet"
        release(false)
        return try {
            val c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            try {
                c.configure(format(width, height, fps, bitrate, true), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            } catch (e: Exception) {
                Log.i("KagazTv", "the encoder refused the low-delay settings: $e")
                // Some encoders refuse the profile or the low-delay wishes.
                c.reset()
                c.configure(format(width, height, fps, bitrate, false), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            }
            val s = c.createInputSurface()
            c.start()
            codec = c
            surface = s
            // Hidden until shown: the first picture the TV gets is a blur.
            screenHidden = true
            soundOnTv = false
            val pic = KagazTvPicture(s, width, height, true)
            picture = pic
            val onto = pic.start()
            running = true
            display = mp.createVirtualDisplay(
                "Kagaz", width, height, context.resources.displayMetrics.densityDpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, onto, null, null
            )
            drainer = Thread({ drain(c) }, "kagaz-tv-encoder").also { it.start() }
            Log.i("KagazTv", "encoding on ${c.name}")
            if (canSound()) sounder = Thread({ soundLoop(mp, context) }, "kagaz-tv-sound").also { it.start() }
            announce()
            ""
        } catch (e: Throwable) {
            Log.w("KagazTv", "the encoder did not start", e)
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
            if (key) Log.i("KagazTv", "a whole picture, ${out.size} bytes")
            try {
                frame(out, key)
            } catch (e: Throwable) {
                Log.w("KagazTv", "Kagaz did not take a picture", e)
                break
            }
        }
    }

    /** The phone's sound to Opus packets, until the session stops. */
    private fun soundLoop(mp: MediaProjection, context: Context) {
        val frame = 960 * 2 * 2 // 20 ms of 48 kHz stereo, 16 bits
        var record: AudioRecord? = null
        var enc: MediaCodec? = null
        try {
            if (Build.VERSION.SDK_INT >= 29 &&
                context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
            ) {
                try {
                    val what = AudioPlaybackCaptureConfiguration.Builder(mp)
                        .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                        .addMatchingUsage(AudioAttributes.USAGE_GAME)
                        .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                        .build()
                    val form = AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(48000)
                        .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                        .build()
                    val least = AudioRecord.getMinBufferSize(48000, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT)
                    val r = AudioRecord.Builder()
                        .setAudioFormat(form)
                        .setBufferSizeInBytes(maxOf(least, frame * 4))
                        .setAudioPlaybackCaptureConfig(what)
                        .build()
                    r.startRecording()
                    if (r.recordingState == AudioRecord.RECORDSTATE_RECORDING) record = r else r.release()
                } catch (e: Throwable) {
                    Log.w("KagazTv", "the phone's sound cannot be taken", e)
                }
            }
            capturing = record != null
            Log.i("KagazTv", "sound: ${if (capturing) "taking what the phone plays" else "silence (not allowed, or not possible)"}")
            applySound(context)
            val f = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48000, 2)
            f.setInteger(MediaFormat.KEY_BIT_RATE, 128_000)
            f.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, frame)
            val c = MediaCodec.createByCodecName(opus!!)
            c.configure(f, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            c.start()
            enc = c
            val pcm = ByteArray(frame)
            val info = MediaCodec.BufferInfo()
            var sent = 0L
            val began = System.nanoTime()
            val am = context.getSystemService(AudioManager::class.java)
            var silentFor = 0 // packets of plain silence in a row
            var loudest = 0 // this second's peak, of 32767
            var packets = 0
            while (running) {
                val r = record
                if (r != null) {
                    var got = 0
                    while (got < frame && running) {
                        val n = r.read(pcm, got, frame - got)
                        if (n <= 0) break
                        got += n
                    }
                    if (got < frame) java.util.Arrays.fill(pcm, got, frame, 0)
                    // How loud this packet is (its largest sample).
                    var peak = 0
                    var i = 1
                    while (i < frame) {
                        val v = Math.abs(pcm[i].toInt() shl 8 or (pcm[i - 1].toInt() and 0xff))
                        if (v > peak) peak = v
                        i += 8
                    }
                    if (peak > loudest) loudest = peak
                    // Real sound: the phone goes quiet at once. A second
                    // and a half of plain silence while something plays:
                    // that app keeps its sound, and the phone plays it.
                    silentFor = if (peak > 0) 0 else silentFor + 1
                    heard = peak > 0 || (heard && silentFor < 75)
                    // Real sound: no app is keeping its sound. Four
                    // seconds of plain silence while something plays: the
                    // app playing keeps its sound, and the phone plays it.
                    val nowKept = when {
                        peak > 0 || !soundOnTv -> false
                        silentFor >= 200 && am != null && am.isMusicActive -> true
                        else -> soundKept
                    }
                    if (nowKept != soundKept) {
                        soundKept = nowKept
                        Log.i("KagazTv", if (nowKept) "sound: the app playing keeps its sound to the phone" else "sound: real sound comes again")
                        applySound(context)
                        announce()
                    }
                    packets++
                    // Ten times a second: a press of the volume keys may
                    // have raised the phone's volume; quiet again.
                    if (packets % 5 == 0) applySound(context)
                    if (packets % 250 == 0) {
                        Log.i("KagazTv", "sound: loudest $loudest of 32767 in 5 s; on the TV: $soundOnTv; phone kept quiet: ${volumeBefore >= 0}; something plays: ${am?.isMusicActive}")
                        loudest = 0
                    }
                    if (!soundOnTv) java.util.Arrays.fill(pcm, 0.toByte())
                } else {
                    // Silence, at the pace sound would come.
                    val due = began + (sent + 1) * 20_000_000L
                    val wait = (due - System.nanoTime()) / 1_000_000L
                    if (wait > 0) Thread.sleep(wait)
                }
                val i = c.dequeueInputBuffer(20_000)
                if (i >= 0) {
                    val b = c.getInputBuffer(i)
                    if (b != null) {
                        b.clear()
                        b.put(pcm)
                    }
                    c.queueInputBuffer(i, 0, frame, sent * 20_000L, 0)
                    sent++
                }
                while (true) {
                    val o = c.dequeueOutputBuffer(info, 0)
                    if (o < 0) break
                    val b = c.getOutputBuffer(o)
                    if (b != null && info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                        val bytes = ByteArray(info.size)
                        b.position(info.offset)
                        b.get(bytes)
                        try {
                            sound(bytes)
                        } catch (e: Throwable) {
                        }
                    }
                    c.releaseOutputBuffer(o, false)
                }
            }
        } catch (e: Throwable) {
            Log.w("KagazTv", "the sound stopped", e)
        } finally {
            capturing = false
            heard = false
            soundKept = false
            applySound(context)
            try { record?.stop() } catch (e: Throwable) {}
            try { record?.release() } catch (e: Throwable) {}
            try { enc?.stop() } catch (e: Throwable) {}
            try { enc?.release() } catch (e: Throwable) {}
        }
    }

    /** The phone's speaker is kept quiet exactly while its sound is taken
     *  for the TV and no app is keeping its sound: the media volume turned
     *  to nothing, and put back where it was after (unless the person has
     *  moved it meanwhile). */
    private fun applySound(context: Context) {
        val am = context.getSystemService(AudioManager::class.java) ?: return
        synchronized(soundLock) {
            val want = running && capturing && soundOnTv && !soundKept
            try {
                val now = am.getStreamVolume(AudioManager.STREAM_MUSIC)
                if (want) {
                    if (volumeBefore < 0) volumeBefore = now
                    if (now != 0) am.setStreamVolume(AudioManager.STREAM_MUSIC, 0, 0)
                } else if (volumeBefore >= 0) {
                    if (now == 0 && volumeBefore > 0) am.setStreamVolume(AudioManager.STREAM_MUSIC, volumeBefore, 0)
                    volumeBefore = -1
                }
            } catch (e: Throwable) {
                Log.w("KagazTv", "the phone's volume could not be changed", e)
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
        try { sounder?.join(500) } catch (e: Throwable) {}
        sounder = null
        app?.let { applySound(it) }
        val was = display != null
        try { display?.release() } catch (e: Throwable) {}
        display = null
        try { picture?.stop() } catch (e: Throwable) {}
        picture = null
        try { codec?.stop() } catch (e: Throwable) {}
        try { codec?.release() } catch (e: Throwable) {}
        codec = null
        try { surface?.release() } catch (e: Throwable) {}
        surface = null
        if (was) announce()
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
