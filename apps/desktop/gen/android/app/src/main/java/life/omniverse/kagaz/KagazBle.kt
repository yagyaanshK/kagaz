package life.omniverse.kagaz

// The phone's Bluetooth LE for the ceiling fans (the Rust half is
// kagaz-core's fans/phone.rs, reached through KagazNative): listen for
// adverts, and keep one connection per fan to write a command and read the
// state. Every call blocks the Rust thread that made it, never the main one.
//
// Answers are "ok:<text>" or "err:<code>:<detail>", codes as mobile.rs
// reads them: none, off, denied, reach, busy, refuses, other.
//
// Connections are precious: a fan takes only so many new ones before it
// stops answering until switched off at the wall (seen 2026-10-21), so a
// connection is kept until Rust closes it, a failed one is closed at once,
// and the same fan is not connected to again within two seconds.

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

@SuppressLint("MissingPermission")
object KagazBle {
    const val ASK_PERMISSION = 42
    const val ASK_TURN_ON = 43

    private class Failed(val code: String, message: String) : Exception(message)

    // ---- Permission and Bluetooth on, asked of the person when needed.

    private fun needed(): Array<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
        } else {
            // Before Android 12, finding Bluetooth devices needs location.
            arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    private fun allowed(context: Context) =
        needed().all { context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }

    @Volatile private var answered: CountDownLatch? = null

    /** MainActivity: the person answered a question Kagaz asked here. */
    fun answered(requestCode: Int) {
        if (requestCode == ASK_PERMISSION || requestCode == ASK_TURN_ON) answered?.countDown()
    }

    /** Ask on the window (when it is on screen) and wait for the answer. */
    private fun ask(ask: (Activity) -> Unit): Boolean {
        val activity = MainActivity.onScreen() ?: return false
        val latch = CountDownLatch(1)
        answered = latch
        activity.runOnUiThread {
            try {
                ask(activity)
            } catch (e: Exception) {
                latch.countDown()
            }
        }
        return latch.await(2, TimeUnit.MINUTES)
    }

    private fun adapter(context: Context): BluetoothAdapter {
        if (!allowed(context)) {
            ask { it.requestPermissions(needed(), ASK_PERMISSION) }
            if (!allowed(context)) throw Failed("denied", "Bluetooth permission not given")
        }
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
        val adapter = manager?.adapter ?: throw Failed("none", "this phone has no Bluetooth")
        if (!adapter.isEnabled) {
            // Android's own "Turn on Bluetooth?" question.
            @Suppress("DEPRECATION")
            ask { it.startActivityForResult(Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE), ASK_TURN_ON) }
            if (!adapter.isEnabled) throw Failed("off", "Bluetooth is turned off")
        }
        return adapter
    }

    private fun answer(work: () -> String): String = try {
        "ok:" + work()
    } catch (e: Failed) {
        "err:${e.code}:${e.message ?: ""}"
    } catch (e: SecurityException) {
        "err:denied:${e.message ?: ""}"
    } catch (e: Exception) {
        "err:other:${e.message ?: e.javaClass.simpleName}"
    }

    // ---- Listening for adverts

    /** Named devices heard over `millis`, as a JSON list. */
    fun scan(context: Context, millis: Int): String = answer {
        val scanner = adapter(context).bluetoothLeScanner ?: throw Failed("off", "Bluetooth is turned off")
        val heard = LinkedHashMap<String, JSONObject>()
        val callback = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, result: ScanResult) {
                val name = result.scanRecord?.deviceName ?: result.device.name ?: return
                synchronized(heard) {
                    heard[result.device.address] = JSONObject()
                        .put("address", result.device.address)
                        .put("name", name)
                        .put("rssi", result.rssi)
                }
            }

            override fun onBatchScanResults(results: MutableList<ScanResult>) {
                results.forEach { onScanResult(0, it) }
            }
        }
        val settings = ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build()
        scanner.startScan(null, settings, callback)
        try {
            Thread.sleep(millis.coerceIn(1000, 60000).toLong())
        } finally {
            try {
                scanner.stopScan(callback)
            } catch (e: Exception) {
            }
        }
        synchronized(heard) { JSONArray(heard.values).toString() }
    }

    // ---- Kept connections

    private class Event(val kind: String, val status: Int, val value: ByteArray? = null)

    private class Link(val address: String) : BluetoothGattCallback() {
        val events = LinkedBlockingQueue<Event>()
        var gatt: BluetoothGatt? = null
        var write: BluetoothGattCharacteristic? = null
        var read: BluetoothGattCharacteristic? = null
        @Volatile var connected = false

        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            connected = newState == BluetoothProfile.STATE_CONNECTED && status == BluetoothGatt.GATT_SUCCESS
            events.put(Event(if (connected) "connected" else "disconnected", status))
        }

        override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
            events.put(Event("mtu", status))
        }

        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            events.put(Event("services", status))
        }

        override fun onCharacteristicWrite(gatt: BluetoothGatt, c: BluetoothGattCharacteristic, status: Int) {
            events.put(Event("write", status))
        }

        // Android 13 and later.
        override fun onCharacteristicRead(gatt: BluetoothGatt, c: BluetoothGattCharacteristic, value: ByteArray, status: Int) {
            events.put(Event("read", status, value))
        }

        // Before Android 13.
        @Deprecated("Android 13 calls the one above")
        @Suppress("DEPRECATION")
        override fun onCharacteristicRead(gatt: BluetoothGatt, c: BluetoothGattCharacteristic, status: Int) {
            events.put(Event("read", status, c.value))
        }

        /** The next `kind` event within `millis`; a dropped connection
         *  ends the wait. */
        fun await(kind: String, millis: Long): Event? {
            val end = System.currentTimeMillis() + millis
            while (true) {
                val left = end - System.currentTimeMillis()
                if (left <= 0) return null
                val e = events.poll(left, TimeUnit.MILLISECONDS) ?: return null
                if (e.kind == kind) return e
                if (e.kind == "disconnected") throw Failed("reach", "the fan dropped the connection")
            }
        }

        fun shut() {
            connected = false
            try {
                gatt?.disconnect()
                gatt?.close()
            } catch (e: Exception) {
            }
            gatt = null
        }
    }

    private val links = HashMap<String, Link>()
    /** When each fan was last let go. */
    private val lastGone = HashMap<String, Long>()
    private const val GAP = 2000L

    private fun letGo(link: Link) {
        link.shut()
        lastGone[link.address] = System.currentTimeMillis()
    }

    /** Connect to the fan, gently: two tries at most, three seconds apart,
     *  each failed one closed, two seconds after the fan was last let go. */
    private fun open(context: Context, adapter: BluetoothAdapter, address: String, service: UUID, write: UUID, read: UUID): Link {
        val wait = (lastGone[address] ?: 0L) + GAP - System.currentTimeMillis()
        if (wait > 0) Thread.sleep(wait)
        val device: BluetoothDevice = adapter.getRemoteDevice(address)
        var last = Failed("reach", "the fan is out of reach")
        for (attempt in 0 until 2) {
            if (attempt > 0) Thread.sleep(3000)
            val link = Link(address)
            try {
                link.gatt = device.connectGatt(context, false, link, BluetoothDevice.TRANSPORT_LE)
                    ?: throw Failed("other", "Android would not start a connection")
                val up = link.await("connected", 12000)
                    ?: throw Failed("refuses", "the fan did not take the connection")
                if (!link.connected) throw Failed("reach", "the fan did not take the connection (${up.status})")
                // Room for a whole command in one write ({"light_mode":…}
                // is longer than the 20 bytes a connection starts with).
                if (link.gatt!!.requestMtu(185)) link.await("mtu", 3000)
                if (!link.gatt!!.discoverServices()) throw Failed("other", "Android would not list the fan's services")
                link.await("services", 10000) ?: throw Failed("busy", "the fan listed no services")
                val gatt = link.gatt!!
                val all = gatt.services.flatMap { it.characteristics }
                val ours = gatt.getService(service)
                // A fan that lists nothing at all has stopped taking connections.
                if (ours == null && all.size <= 1) throw Failed("busy", "the fan offered none of its controls")
                link.write = ours?.getCharacteristic(write)
                link.read = ours?.getCharacteristic(read)
                    ?: throw Failed("other", "this device has no Atomberg state service")
                return link
            } catch (e: Exception) {
                letGo(link)
                if (e !is Failed || e.code == "busy" || e.code == "other") throw e
                last = e
            }
        }
        throw last
    }

    private fun writeTo(link: Link, text: String) {
        val gatt = link.gatt ?: throw Failed("reach", "the fan dropped the connection")
        val c = link.write ?: throw Failed("other", "this fan takes no Atomberg commands")
        val bytes = text.toByteArray(Charsets.UTF_8)
        val started = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            gatt.writeCharacteristic(c, bytes, BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT) == BluetoothGatt.GATT_SUCCESS
        } else {
            @Suppress("DEPRECATION")
            run {
                c.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
                c.value = bytes
                gatt.writeCharacteristic(c)
            }
        }
        if (!started) throw Failed("other", "Android would not send the command")
        val done = link.await("write", 5000) ?: throw Failed("reach", "the fan did not take the command")
        if (done.status != BluetoothGatt.GATT_SUCCESS) throw Failed("other", "the fan refused the command (${done.status})")
    }

    private fun readFrom(link: Link): String {
        val gatt = link.gatt ?: throw Failed("reach", "the fan dropped the connection")
        if (!gatt.readCharacteristic(link.read)) throw Failed("other", "Android would not read the fan")
        val got = link.await("read", 5000) ?: throw Failed("reach", "the fan did not answer")
        if (got.status != BluetoothGatt.GATT_SUCCESS) throw Failed("other", "the fan would not be read (${got.status})")
        return String(got.value ?: ByteArray(0), Charsets.UTF_8).trim()
    }

    /** Write `command` (when given) and read the state, on the fan's kept
     *  connection, opened when there is none or the fan dropped it. */
    fun exchange(context: Context, address: String, service: String, write: String, read: String, command: String?): String = answer {
        synchronized(links) {
            val adapter = adapter(context)
            val key = address.uppercase()
            var link = links[key]
            if (link != null && !link.connected) {
                letGo(link)
                links.remove(key)
                link = null
            }
            if (link == null) {
                link = open(context, adapter, key, UUID.fromString(service), UUID.fromString(write), UUID.fromString(read))
                links[key] = link
            }
            try {
                link.events.clear()
                if (command != null) {
                    writeTo(link, command)
                    // The fan applies the command and refreshes its state string.
                    Thread.sleep(450)
                }
                readFrom(link)
            } catch (e: Exception) {
                letGo(link)
                links.remove(key)
                throw e
            }
        }
    }

    /** Let go of the fan's connection. */
    fun close(address: String) {
        synchronized(links) {
            links.remove(address.uppercase())?.let { letGo(it) }
        }
    }
}
