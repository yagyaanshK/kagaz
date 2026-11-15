package app.kagaz

import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.MediaRecorder
import android.media.MicrophoneInfo
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AudioEffect
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Process
import android.util.Log

/** The microphone for doorbell calls, taken by Kagaz itself instead of the
 *  web page, so it can use both of the phone's microphones.
 *
 *  Why: on the loudspeaker Android records a call from the phone's second
 *  microphone (the "back" one), and the page can neither choose another
 *  nor take two. On a phone whose second microphone was blocked the other
 *  side heard nothing (2026-11-16: the page got exact silence while taps
 *  on the body came through). Apps built on WebRTC record the same way
 *  (VOICE_COMMUNICATION, mono) and leave the choice to the phone.
 *
 *  The ways tried, in order; Android says which microphones each really
 *  uses (AudioRecord.getActiveMicrophones):
 *   0. VOICE_COMMUNICATION in stereo: the phone's own echo cancelling,
 *      kept when two microphones are active.
 *   1. MIC in stereo with Android's echo canceller and noise suppressor on
 *      it: two raw microphones, kept when two are active.
 *   2. VOICE_COMMUNICATION in mono from the lowest microphone on the phone
 *      (its position from AudioManager.getMicrophones): the fallback.
 *  With two, each 20 ms the louder microphone counts for more, so one that
 *  hears nothing no longer silences the call. A way that gives only exact
 *  silence for 4 s (what a blocked or taken microphone gives; a working one
 *  always hears a little) is left for the next.
 *
 *  Out: 8 kHz 16-bit mono, 40 ms at a time, to Rust (KagazNative.micPcm),
 *  as the page's microphone sent it. State to the page (KagazNative.micState,
 *  the "call-mic" event). */
object CallMic {
    private const val TAG = "KagazMic"
    private const val RATE = 16000
    private const val FRAME = 320 // 20 ms at 16 kHz
    private const val BOTH = 0
    private const val BOTH_RAW = 1
    private const val BOTTOM = 2
    private val NAMES = arrayOf("both microphones", "both microphones, raw", "the bottom microphone")
    private const val SILENT_MS = 4000

    @Volatile private var running = false
    @Volatile var muted = false
    private var thread: Thread? = null

    /** Start (or keep) recording; false when Kagaz may not use the microphone. */
    @Synchronized
    fun start(context: Context, mute: Boolean): Boolean {
        muted = mute
        if (running) return true
        if (context.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) return false
        val a = context.applicationContext.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        running = true
        thread = Thread({ run(a) }, "kagaz-call-mic").apply { start() }
        return true
    }

    @Synchronized
    fun stop() {
        running = false
        thread?.join(1500)
        thread = null
    }

    private class Open(val rec: AudioRecord, val channels: Int, val mode: Int, val mics: Int, val effects: List<AudioEffect>) {
        fun close() {
            for (e in effects) try { e.release() } catch (x: Exception) {}
            try { rec.stop() } catch (x: Exception) {}
            rec.release()
        }
    }

    private enum class End { STOPPED, SILENT, BROKEN }

    private fun run(a: AudioManager) {
        try { Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO) } catch (e: Exception) {}
        inventory(a)
        var from = BOTH
        while (running) {
            val o = choose(a, from)
            if (o == null) {
                report(-1, 0, true, "the microphone could not be opened")
                sleep(1000)
                continue
            }
            report(o.mode, o.mics, false, "")
            val end = pump(o)
            o.close()
            when (end) {
                End.STOPPED -> {}
                // Taken from us (a route change, another app): the same way again.
                End.BROKEN -> { from = o.mode; sleep(300) }
                // Only silence: the next way; the last one again after a pause.
                End.SILENT -> {
                    report(o.mode, o.mics, true, "${NAMES[o.mode]} gave only silence")
                    if (o.mode == BOTTOM) sleep(2000)
                    from = minOf(o.mode + 1, BOTTOM)
                }
            }
        }
        Log.i(TAG, "stopped")
    }

    /** The first way from `from` on that opens, and for the two-microphone
     *  ways, that Android says records from two microphones. */
    private fun choose(a: AudioManager, from: Int): Open? {
        for (mode in from..BOTTOM) {
            val o = open(a, mode) ?: continue
            if (mode == BOTTOM || o.mics >= 2 || o.mics < 0) return o
            Log.i(TAG, "${NAMES[mode]}: Android records from ${o.mics} microphone(s) only; next way")
            o.close()
        }
        return null
    }

    private fun open(a: AudioManager, mode: Int): Open? {
        val channels = if (mode == BOTTOM) 1 else 2
        val mask = if (channels == 2) AudioFormat.CHANNEL_IN_STEREO else AudioFormat.CHANNEL_IN_MONO
        val source = if (mode == BOTH_RAW) MediaRecorder.AudioSource.MIC else MediaRecorder.AudioSource.VOICE_COMMUNICATION
        val min = AudioRecord.getMinBufferSize(RATE, mask, AudioFormat.ENCODING_PCM_16BIT)
        if (min <= 0) return null
        val rec = try {
            AudioRecord.Builder()
                .setAudioSource(source)
                .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT).setSampleRate(RATE).setChannelMask(mask).build())
                .setBufferSizeInBytes(maxOf(min, RATE / 5 * 2 * channels))
                .build()
        } catch (e: Exception) {
            Log.i(TAG, "${NAMES[mode]}: cannot open: $e")
            return null
        }
        if (rec.state != AudioRecord.STATE_INITIALIZED || rec.channelCount != channels) {
            rec.release()
            return null
        }
        if (mode == BOTTOM) bottomMic(a)?.let { rec.preferredDevice = it }
        val effects = mutableListOf<AudioEffect>()
        if (mode == BOTH_RAW) {
            try {
                if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(rec.audioSessionId)?.let { it.enabled = true; effects.add(it) }
                if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(rec.audioSessionId)?.let { it.enabled = true; effects.add(it) }
            } catch (e: Exception) {}
        }
        try {
            rec.startRecording()
        } catch (e: Exception) {
            for (x in effects) try { x.release() } catch (y: Exception) {}
            rec.release()
            return null
        }
        if (rec.recordingState != AudioRecord.RECORDSTATE_RECORDING) {
            for (x in effects) try { x.release() } catch (y: Exception) {}
            rec.release()
            return null
        }
        sleep(150) // the route settles before Android can say which microphones it uses
        var mics = -1
        var which = ""
        if (Build.VERSION.SDK_INT >= 28) {
            try {
                val active = rec.activeMicrophones
                mics = active.size
                which = active.joinToString { describe(it) }
            } catch (e: Exception) {}
        }
        Log.i(TAG, "opened ${NAMES[mode]}: source $source, $channels channel(s), ${effects.size} effect(s), active microphones $mics [$which]")
        return Open(rec, channels, mode, mics, effects)
    }

    /** Record, mix, send; returns why it stopped. */
    private fun pump(o: Open): End {
        val buf = ShortArray(FRAME * o.channels)
        val block = ShortArray(FRAME) // 40 ms at 8 kHz
        var filled = 0
        var w = 0.5f // the left microphone's share
        var silentMs = 0
        while (running) {
            var n = 0
            while (n < buf.size) {
                if (!running) return End.STOPPED
                val r = o.rec.read(buf, n, buf.size - n)
                if (r < 0) {
                    Log.i(TAG, "${NAMES[o.mode]}: read failed ($r)")
                    return End.BROKEN
                }
                n += r
            }
            if (o.channels == 2) {
                var e0 = 0.0
                var e1 = 0.0
                for (i in 0 until FRAME) {
                    val l = buf[2 * i].toDouble()
                    val r = buf[2 * i + 1].toDouble()
                    e0 += l * l
                    e1 += r * r
                }
                if (e0 + e1 > 0) w += ((e0 / (e0 + e1)).toFloat() - w) * 0.3f
            }
            // Down to 8 kHz: each pair of samples averaged (a cheap low-pass).
            var any = false
            for (i in 0 until FRAME / 2) {
                val v = if (o.channels == 2) {
                    (w * (buf[4 * i] + buf[4 * i + 2]) + (1 - w) * (buf[4 * i + 1] + buf[4 * i + 3])) / 2f
                } else {
                    (buf[2 * i] + buf[2 * i + 1]) / 2f
                }
                val s = v.toInt().coerceIn(-32768, 32767)
                if (s != 0) any = true
                block[filled++] = s.toShort()
            }
            if (any) {
                if (silentMs >= SILENT_MS / 2) report(o.mode, o.mics, false, "")
                silentMs = 0
            } else {
                silentMs += 20
                if (silentMs >= SILENT_MS) return End.SILENT
            }
            if (filled == block.size) {
                filled = 0
                if (!muted) send(block)
            }
        }
        return End.STOPPED
    }

    private fun send(pcm: ShortArray) {
        val bytes = ByteArray(pcm.size * 2)
        for (i in pcm.indices) {
            val v = pcm[i].toInt()
            bytes[2 * i] = (v and 0xff).toByte()
            bytes[2 * i + 1] = (v shr 8 and 0xff).toByte()
        }
        try { KagazNative.micPcm(bytes) } catch (e: Throwable) {}
    }

    /** The microphone mounted lowest on the phone (its main one), as Android
     *  lists it; else one named for the bottom, else the first built-in. */
    private fun bottomMic(a: AudioManager): AudioDeviceInfo? {
        val ins = a.getDevices(AudioManager.GET_DEVICES_INPUTS).filter { it.type == AudioDeviceInfo.TYPE_BUILTIN_MIC }
        if (ins.isEmpty()) return null
        if (Build.VERSION.SDK_INT >= 28) {
            try {
                val lowest = a.microphones
                    .filter { it.type == AudioDeviceInfo.TYPE_BUILTIN_MIC && it.position != MicrophoneInfo.POSITION_UNKNOWN }
                    .minByOrNull { it.position.y }
                if (lowest != null) ins.firstOrNull { it.address == lowest.address }?.let { return it }
            } catch (e: Exception) {}
            ins.firstOrNull { it.address.contains("bottom", true) }?.let { return it }
            ins.firstOrNull { !it.address.contains("back", true) && !it.address.contains("top", true) }?.let { return it }
        }
        return ins.first()
    }

    /** The phone's microphones, once per call, for the log. */
    private fun inventory(a: AudioManager) {
        if (Build.VERSION.SDK_INT < 28) return
        try {
            Log.i(TAG, "microphones: " + a.microphones.joinToString { describe(it) })
        } catch (e: Exception) {}
    }

    private fun describe(m: MicrophoneInfo): String {
        val where = when (m.location) {
            MicrophoneInfo.LOCATION_MAINBODY -> "body"
            MicrophoneInfo.LOCATION_MAINBODY_MOVABLE -> "movable part"
            MicrophoneInfo.LOCATION_PERIPHERAL -> "accessory"
            else -> "unknown place"
        }
        val pos = if (m.position == MicrophoneInfo.POSITION_UNKNOWN) "" else " at y=${"%.3f".format(m.position.y)} m"
        return "${m.description} (${m.address}, $where$pos)"
    }

    private fun report(mode: Int, mics: Int, silent: Boolean, why: String) {
        val w = why.replace("\\", "").replace("\"", "'")
        val json = "{\"mode\":$mode,\"way\":\"${if (mode >= 0) NAMES[mode] else ""}\",\"mics\":$mics,\"silent\":$silent,\"why\":\"$w\"}"
        Log.i(TAG, "state $json")
        try { KagazNative.micState(json) } catch (e: Throwable) {}
    }

    private fun sleep(ms: Long) {
        try { Thread.sleep(ms) } catch (e: InterruptedException) {}
    }
}
