package com.cadence.player

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbManager
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.core.content.ContextCompat

/**
 * USB exclusive: hands a USB DAC over to Cadence's own driver (Rust, src/audio/usb_dac.rs), for
 * phones without Android's bit-perfect USB playback. Android asks the user once per plug-in whether
 * Cadence may use the DAC; after that the engine gets the device's usbfs file and streams to it
 * itself, while Android (and other apps) can't use the DAC.
 */
object UsbDac {
  private const val ACTION_PERMISSION = "com.cadence.player.USB_PERMISSION"
  private val main = Handler(Looper.getMainLooper())
  private var connection: UsbDeviceConnection? = null
  /** The DAC the user was asked about, and the answer once it comes ("1" allowed, "0" not). */
  private var asked: String? = null
  @Volatile private var answer = ""
  /** DACs the user didn't allow (until they're plugged in again): not asked about again. */
  private val refused = mutableSetOf<String>()

  /**
   * The DAC's volume while Cadence drives it, in Android's own volume steps for USB: Android's volume
   * doesn't reach the DAC then, so Cadence applies it (the volume keys move it, see PlaybackService).
   * It starts where Android's music volume was, so taking the DAC over doesn't change the loudness.
   */
  @Volatile var level = 0
    private set
  @Volatile var maxLevel = 15
    private set
  @Volatile private var gain = 1f
  private var usbType = AudioDeviceInfo.TYPE_USB_HEADSET

  /** Moves the DAC's volume to `step` (0 to [maxLevel]). */
  fun setLevel(context: Context, step: Int) {
    level = step.coerceIn(0, maxLevel)
    gain = gainAt(context.getSystemService(AudioManager::class.java), level)
  }

  /** Android's own loudness for USB at volume step `step`, as a factor (1 = full). */
  private fun gainAt(audio: AudioManager, step: Int): Float {
    if (step <= 0) return 0f
    if (step >= maxLevel) return 1f
    if (Build.VERSION.SDK_INT >= 28) {
      runCatching { audio.getStreamVolumeDb(AudioManager.STREAM_MUSIC, step, usbType) }.getOrNull()
        ?.takeIf { it.isFinite() }
        ?.let { return Math.pow(10.0, it / 20.0).toFloat().coerceIn(0f, 1f) }
    }
    val x = step.toFloat() / maxLevel
    return x * x
  }

  /** The DAC's volume as a factor, for the driver ("0.0123"). */
  @JvmStatic
  fun gain(@Suppress("UNUSED_PARAMETER") context: Context, @Suppress("UNUSED_PARAMETER") arg: Int): String = gain.toString()

  /** While Cadence holds the DAC, and for a moment after taking it or giving it back. */
  @Volatile var holding = false
    private set
  @Volatile private var changedAt = 0L

  /**
   * Android's audio system sees the DAC disappear when Cadence takes it (and come back after): that
   * isn't headphones being unplugged, so it mustn't pause the music.
   */
  fun quiet(): Boolean = holding || SystemClock.elapsedRealtime() - changedAt < 3000

  private fun isDac(d: UsbDevice) = (0 until d.interfaceCount).any { i ->
    val intf = d.getInterface(i)
    intf.interfaceClass == UsbConstants.USB_CLASS_AUDIO && intf.interfaceSubclass == 2 &&
      (0 until intf.endpointCount).any { e ->
        val ep = intf.getEndpoint(e)
        ep.type == UsbConstants.USB_ENDPOINT_XFER_ISOC && ep.direction == UsbConstants.USB_DIR_OUT
      }
  }

  private fun nameOf(d: UsbDevice) = d.productName?.takeIf { it.isNotBlank() } ?: "USB DAC"

  /** A device is the same DAC only while it stays plugged in (Android numbers each plug-in anew). */
  private fun keyOf(d: UsbDevice) = "${d.deviceName} ${d.vendorId}:${d.productId}"

  /**
   * Opens the DAC for Cadence's driver. Answers "<usbfs file> <name>", "?" and a message while the
   * user is asked to allow it (Android shows a dialog; the engine tries again once they do), or "!"
   * and why it isn't possible.
   */
  @JvmStatic
  fun open(context: Context, @Suppress("UNUSED_PARAMETER") rate: Int): String {
    val usb = context.getSystemService(UsbManager::class.java) ?: return "!this phone can't use USB devices"
    val dac = usb.deviceList.values.firstOrNull(::isDac) ?: return "!no USB DAC is connected"
    val name = nameOf(dac)
    val key = keyOf(dac)
    val wasRefused = synchronized(this) {
      refused.retainAll(usb.deviceList.values.map(::keyOf).toSet())
      key in refused
    }
    if (!usb.hasPermission(dac)) {
      if (wasRefused) return "!Cadence wasn't allowed to use $name"
      ask(context.applicationContext, usb, dac)
      return "?waiting for you to allow Cadence to use $name"
    }
    // Android still plays to the DAC at this point: its music volume there is where Cadence starts.
    val audio = context.getSystemService(AudioManager::class.java)
    audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).firstOrNull {
      it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET
    }?.let { usbType = it.type }
    maxLevel = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).coerceAtLeast(1)
    setLevel(context, audio.getStreamVolume(AudioManager.STREAM_MUSIC))
    connection?.close()
    val conn = usb.openDevice(dac) ?: return "!Android didn't open $name"
    connection = conn
    holding = true
    changedAt = SystemClock.elapsedRealtime()
    main.post { PlaybackService.remoteVolume(true) }
    return "${conn.fileDescriptor} $name"
  }

  /**
   * Closes the DAC after the driver let it go. With `waitMs`, waits (at most that long) until Android's
   * audio system lists the DAC again, so playing through Android finds it.
   */
  @JvmStatic
  fun close(context: Context, waitMs: Int): String {
    connection?.close()
    connection = null
    if (holding) {
      holding = false
      changedAt = SystemClock.elapsedRealtime()
      main.post { PlaybackService.remoteVolume(false) }
    }
    if (waitMs > 0) {
      val audio = context.getSystemService(AudioManager::class.java)
      val until = SystemClock.elapsedRealtime() + waitMs
      while (SystemClock.elapsedRealtime() < until) {
        val back = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any {
          it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET
        }
        if (back) break
        Thread.sleep(50)
      }
    }
    return ""
  }

  /** The user's answer to the question, once ("1" allowed, "0" not, "" none yet). */
  @JvmStatic
  fun takeAnswer(@Suppress("UNUSED_PARAMETER") context: Context, @Suppress("UNUSED_PARAMETER") arg: Int): String =
    synchronized(this) { answer.also { answer = "" } }

  private fun ask(context: Context, usb: UsbManager, dac: UsbDevice) {
    val key = keyOf(dac)
    synchronized(this) {
      if (asked == key) return
      asked = key
      answer = ""
    }
    val receiver = object : BroadcastReceiver() {
      override fun onReceive(c: Context, intent: Intent) {
        runCatching { c.unregisterReceiver(this) }
        val allowed = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
        synchronized(this@UsbDac) {
          asked = null
          if (!allowed) refused += key
          answer = if (allowed) "1" else "0"
        }
      }
    }
    ContextCompat.registerReceiver(context, receiver, IntentFilter(ACTION_PERMISSION), ContextCompat.RECEIVER_NOT_EXPORTED)
    // Android fills in the answer, so the intent has to be mutable (and only for Cadence itself).
    val flags = if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
    val pending = PendingIntent.getBroadcast(context, 0, Intent(ACTION_PERMISSION).setPackage(context.packageName), flags)
    main.post { usb.requestPermission(dac, pending) }
  }
}
