package life.omniverse.kagaz

// The Android half of Kagaz's phone layer (the Rust half is
// apps/desktop/src/mobile.rs):
// - holds Wi-Fi's multicast lock so discovery hears mDNS and WS-Discovery;
// - opens Android's print dialog for a PDF, which prints through whatever
//   print service (vendor plugin, Mopria) the phone has;
// - copies a finished scan into Downloads/Kagaz;
// - keeps the doorbell line alive in the background and rings for calls
//   (KagazCallService).

import android.app.Activity
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
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
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
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
class CallArgs {
    var phase: String = "idle"
    var peer: String = ""
}

@TauriPlugin
class KagazPlugin(private val activity: Activity) : Plugin(activity) {
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

    @Command
    fun callChanged(invoke: Invoke) {
        val args = invoke.parseArgs(CallArgs::class.java)
        KagazCallService.ring(activity.applicationContext, if (args.phase == "ringing") args.peer else null)
        invoke.resolve()
    }

    /** Notifications (Android 13+), and leaving battery optimisation, asked once. */
    private fun askOnce() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            activity.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            activity.requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 41)
        }
        val prefs = activity.getSharedPreferences("kagaz", Context.MODE_PRIVATE)
        val power = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
        if (!power.isIgnoringBatteryOptimizations(activity.packageName) && !prefs.getBoolean("asked_battery", false)) {
            prefs.edit().putBoolean("asked_battery", true).apply()
            activity.startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:" + activity.packageName))
            )
        }
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
