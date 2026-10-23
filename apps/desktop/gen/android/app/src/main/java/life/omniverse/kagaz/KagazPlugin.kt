package life.omniverse.kagaz

// The Android half of Kagaz's phone layer (the Rust half is
// apps/desktop/src/mobile.rs):
// - holds Wi-Fi's multicast lock so discovery hears mDNS and WS-Discovery;
// - opens Android's print dialog for a PDF, which prints through whatever
//   print service (vendor plugin, Mopria) the phone has;
// - copies a finished scan into Downloads/Kagaz;
// - keeps the doorbell line alive in the background (KagazCallService);
//   the ring itself goes through KagazNative, which works with the window
//   closed, when plugin calls cannot run;
// - keeps a remembered sign-in, encrypted with a key that never leaves the
//   Android Keystore (the desktop's password vault does this elsewhere).

import android.app.Activity
import android.app.DownloadManager
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.core.content.ContextCompat
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.print.PageRange
import android.print.PrintAttributes
import android.print.PrintDocumentAdapter
import android.print.PrintDocumentInfo
import android.print.PrintManager
import android.provider.MediaStore
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import app.tauri.plugin.PluginManager
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream

@InvokeArg
class PrintArgs {
    lateinit var path: String
    var jobName: String = "Kagaz"
}

@InvokeArg
class SaveArgs {
    lateinit var path: String
    lateinit var name: String
    var mime: String = "application/octet-stream"
}

@InvokeArg
class ListenArgs {
    var on: Boolean = false
}

@InvokeArg
class SpeakerArgs {
    var on: Boolean = true
}

@InvokeArg
class SecretArgs {
    lateinit var name: String
    var value: String? = null
}

private const val SECRET_KEY = "kagaz-secrets"

/** Remembered secrets: AES-GCM with a Keystore key; only the sealed bytes
 *  (IV first) sit in the app's private preferences. Used by the plugin and,
 *  with the window closed, by Rust through KagazNative. */
object KagazSecrets {
    private fun secretKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(SECRET_KEY, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(SECRET_KEY, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return generator.generateKey()
    }

    private fun secrets(context: Context) =
        context.applicationContext.getSharedPreferences("kagaz-secrets", Context.MODE_PRIVATE)

    fun get(context: Context, name: String): String? {
        val sealed = secrets(context).getString(name, null) ?: return null
        val bytes = Base64.decode(sealed, Base64.NO_WRAP)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(128, bytes, 0, 12))
        return String(cipher.doFinal(bytes, 12, bytes.size - 12), Charsets.UTF_8)
    }

    fun set(context: Context, name: String, value: String) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, secretKey())
        val sealed = cipher.iv + cipher.doFinal(value.toByteArray(Charsets.UTF_8))
        secrets(context).edit().putString(name, Base64.encodeToString(sealed, Base64.NO_WRAP)).apply()
    }

    fun delete(context: Context, name: String) {
        secrets(context).edit().remove(name).apply()
    }
}

@TauriPlugin
class KagazPlugin(private val first: Activity) : Plugin(first) {
    // Kagaz outlives its window (the doorbell line keeps the process alive
    // after the app is closed), so a later window is a new activity: use
    // whichever one is on screen, not the one this plugin was made with.
    private val activity: Activity
        get() = PluginManager.activity ?: first
    private var multicast: WifiManager.MulticastLock? = null

    override fun load(webView: WebView) {
        super.load(webView)
        val wifi = activity.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicast = wifi.createMulticastLock("kagaz-discovery").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    @Command
    fun keepListening(invoke: Invoke) {
        val args = invoke.parseArgs(ListenArgs::class.java)
        val context = activity.applicationContext
        val service = Intent(context, KagazCallService::class.java)
        activity.runOnUiThread {
            try {
                if (args.on) {
                    askOnce()
                    service.putExtra(KagazCallService.EXTRA_FROM_APP, true)
                    ContextCompat.startForegroundService(context, service)
                } else {
                    context.stopService(service)
                }
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject(e.message ?: "could not keep the doorbell line open")
            }
        }
    }

    /** Notifications (Android 13+), and leaving battery optimisation, asked once. */
    private fun askOnce() {
        // Once: the line starts at every launch, and asking again each time
        // would nag someone who said no (Settings can still turn it on).
        val prefs = activity.getSharedPreferences("kagaz", Context.MODE_PRIVATE)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            activity.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED &&
            !prefs.getBoolean("asked_notifications", false)
        ) {
            prefs.edit().putBoolean("asked_notifications", true).apply()
            activity.requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 41)
        }
        val power = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
        if (!power.isIgnoringBatteryOptimizations(activity.packageName) && !prefs.getBoolean("asked_battery", false)) {
            prefs.edit().putBoolean("asked_battery", true).apply()
            activity.startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:" + activity.packageName))
            )
        }
    }

    @Command
    fun secretGet(invoke: Invoke) {
        val args = invoke.parseArgs(SecretArgs::class.java)
        val ret = JSObject()
        try {
            KagazSecrets.get(activity, args.name)?.let { ret.put("value", it) }
            invoke.resolve(ret)
        } catch (e: Exception) {
            invoke.reject("the saved sign-in could not be unlocked")
        }
    }

    @Command
    fun secretSet(invoke: Invoke) {
        val args = invoke.parseArgs(SecretArgs::class.java)
        try {
            KagazSecrets.set(activity, args.name, args.value ?: "")
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject("the sign-in could not be saved on this phone")
        }
    }

    @Command
    fun secretDelete(invoke: Invoke) {
        val args = invoke.parseArgs(SecretArgs::class.java)
        KagazSecrets.delete(activity, args.name)
        invoke.resolve()
    }

    @Command
    fun print(invoke: Invoke) {
        val args = invoke.parseArgs(PrintArgs::class.java)
        val file = File(args.path)
        activity.runOnUiThread {
            try {
                val manager = activity.getSystemService(Context.PRINT_SERVICE) as PrintManager
                manager.print(args.jobName, PdfAdapter(file, args.jobName), PrintAttributes.Builder().build())
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject(e.message ?: "the print dialog would not open")
            }
        }
    }

    @Command
    fun saveToDownloads(invoke: Invoke) {
        val args = invoke.parseArgs(SaveArgs::class.java)
        try {
            val source = File(args.path)
            val location = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val values = ContentValues().apply {
                    put(MediaStore.Downloads.DISPLAY_NAME, args.name)
                    put(MediaStore.Downloads.MIME_TYPE, args.mime)
                    put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/Kagaz")
                }
                val resolver = activity.contentResolver
                val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                    ?: throw IllegalStateException("Android would not make the file in Downloads")
                val out = resolver.openOutputStream(uri)
                    ?: throw IllegalStateException("Android would not open the file in Downloads")
                out.use { o -> FileInputStream(source).use { it.copyTo(o) } }
                "Downloads/Kagaz/" + args.name
            } else {
                // Before Android 10 the shared Downloads folder needs a
                // storage permission; the app's own Downloads folder does not.
                val dir = File(activity.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS), "Kagaz")
                dir.mkdirs()
                val target = File(dir, args.name)
                source.copyTo(target, overwrite = true)
                target.absolutePath
            }
            val ret = JSObject()
            ret.put("location", location)
            invoke.resolve(ret)
        } catch (e: Exception) {
            invoke.reject(e.message ?: "could not save into Downloads")
        }
    }

    // Call sound on the loudspeaker (a door station or a camera is a video
    // call, which phones play out loud), unless a headset or Bluetooth
    // device is connected; off gives the phone back its own choice. Using
    // the microphone puts the phone in call mode, which picks the earpiece.
    @Command
    fun loudspeaker(invoke: Invoke) {
        val args = invoke.parseArgs(SpeakerArgs::class.java)
        val audio = activity.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        val personal = setOf(
            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
            AudioDeviceInfo.TYPE_USB_HEADSET,
        )
        if (Build.VERSION.SDK_INT >= 31) {
            if (args.on) {
                val devices = audio.availableCommunicationDevices
                val headset = devices.any { it.type in personal || it.type == AudioDeviceInfo.TYPE_BLE_HEADSET }
                val speaker = devices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
                if (!headset && speaker != null) audio.setCommunicationDevice(speaker)
            } else {
                audio.clearCommunicationDevice()
            }
        } else {
            @Suppress("DEPRECATION")
            if (args.on) {
                val headset = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any { it.type in personal }
                audio.mode = AudioManager.MODE_IN_COMMUNICATION
                audio.isSpeakerphoneOn = !headset
            } else {
                audio.isSpeakerphoneOn = false
                audio.mode = AudioManager.MODE_NORMAL
            }
        }
        invoke.resolve()
    }

    // Whether doorbell calls ring in Do Not Disturb: Android keeps that
    // switch on the "Doorbell calls" notifications' own page, where only
    // the person can change it (see KagazCallService.allowed).
    @Command
    fun ringInDnd(invoke: Invoke) {
        KagazCallService.channels(activity)
        val ret = JSObject()
        ret.put("on", KagazCallService.ringsInDnd(activity))
        invoke.resolve(ret)
    }

    /** That page, opened for the person; else Kagaz's notifications. */
    @Command
    fun ringSettings(invoke: Invoke) {
        KagazCallService.channels(activity)
        val tries = mutableListOf<Intent>()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            tries += Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
                .putExtra(Settings.EXTRA_CHANNEL_ID, KagazCallService.RING_CHANNEL)
            tries += Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
        }
        tries += Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${activity.packageName}"))
        for (intent in tries) {
            try {
                activity.startActivity(intent)
                invoke.resolve()
                return
            } catch (e: Exception) {
            }
        }
        invoke.reject("this phone has no page for Kagaz's notifications")
    }

    /** Saved files' folder (Downloads/Kagaz) in the Files app; else Downloads. */
    @Command
    fun showDownloads(invoke: Invoke) {
        val folder = Uri.parse("content://com.android.externalstorage.documents/document/primary%3ADownload%2FKagaz")
        val tries = listOf(
            Intent(Intent.ACTION_VIEW).setDataAndType(folder, "vnd.android.document/directory"),
            Intent(DownloadManager.ACTION_VIEW_DOWNLOADS),
        )
        for (intent in tries) {
            try {
                activity.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                invoke.resolve()
                return
            } catch (e: Exception) {
            }
        }
        invoke.reject("this phone has no app to show Downloads")
    }
}

/** Hands a finished PDF to the print system as it is. */
private class PdfAdapter(private val file: File, private val name: String) : PrintDocumentAdapter() {
    override fun onLayout(
        oldAttributes: PrintAttributes?,
        newAttributes: PrintAttributes,
        cancellationSignal: CancellationSignal,
        callback: LayoutResultCallback,
        extras: Bundle?
    ) {
        if (cancellationSignal.isCanceled) {
            callback.onLayoutCancelled()
            return
        }
        val info = PrintDocumentInfo.Builder(name)
            .setContentType(PrintDocumentInfo.CONTENT_TYPE_DOCUMENT)
            .build()
        callback.onLayoutFinished(info, oldAttributes != newAttributes)
    }

    override fun onWrite(
        pages: Array<out PageRange>,
        destination: ParcelFileDescriptor,
        cancellationSignal: CancellationSignal,
        callback: WriteResultCallback
    ) {
        try {
            FileInputStream(file).use { input ->
                FileOutputStream(destination.fileDescriptor).use { input.copyTo(it) }
            }
            callback.onWriteFinished(arrayOf(PageRange.ALL_PAGES))
        } catch (e: Exception) {
            callback.onWriteFailed(e.message)
        }
    }
}
