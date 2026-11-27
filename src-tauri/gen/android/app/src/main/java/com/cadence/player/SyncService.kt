package com.cadence.player

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

/**
 * Keeps a sync from the computer going while the screen is off: a foreground service (data sync)
 * with a progress notification, holding the CPU and Wi-Fi awake. The copying itself runs in
 * Cadence's own code; the window starts, updates and stops this as the sync goes. It stops itself
 * after MAX_MS whatever happens, so a lost "stop" can't keep the phone awake.
 */
class SyncService : Service() {
  companion object {
    private const val CHANNEL = "sync"
    private const val NOTIFICATION_ID = 2
    private const val MAX_MS = 3 * 60 * 60 * 1000L

    /** Starts or updates the service while `running`, stops it otherwise. */
    fun report(context: Context, running: Boolean, text: String, done: Int, total: Int) {
      val intent = Intent(context, SyncService::class.java)
        .putExtra("running", running)
        .putExtra("text", text)
        .putExtra("done", done)
        .putExtra("total", total)
      if (!running) {
        context.stopService(intent)
        return
      }
      if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
    }
  }

  private var wakeLock: PowerManager.WakeLock? = null
  private var wifiLock: WifiManager.WifiLock? = null
  private val timer = Handler(Looper.getMainLooper())

  override fun onBind(intent: Intent?): IBinder? = null

  @Suppress("DEPRECATION")
  override fun onCreate() {
    super.onCreate()
    if (Build.VERSION.SDK_INT >= 26) {
      val channel = NotificationChannel(CHANNEL, "Sync with computer", NotificationManager.IMPORTANCE_LOW)
      channel.description = "Progress while music is copied from your computer"
      channel.setShowBadge(false)
      getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }
    wakeLock = (getSystemService(Context.POWER_SERVICE) as PowerManager)
      .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Cadence:sync")
      .apply { setReferenceCounted(false); acquire(MAX_MS) }
    wifiLock = (applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager)
      .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "Cadence:sync")
      .apply { setReferenceCounted(false); acquire() }
    timer.postDelayed({ stopSelf() }, MAX_MS)
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val text = intent?.getStringExtra("text").orEmpty().ifEmpty { "Copying music from your computer" }
    val done = intent?.getIntExtra("done", 0) ?: 0
    val total = intent?.getIntExtra("total", 0) ?: 0
    val open = PendingIntent.getActivity(
      this, 0,
      Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE,
    )
    val notification = NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_stat_cadence)
      .setContentTitle("Syncing with your computer")
      .setContentText(text)
      .setProgress(total.coerceAtLeast(0), done.coerceIn(0, total.coerceAtLeast(0)), total <= 0)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setContentIntent(open)
      .build()
    val type = if (Build.VERSION.SDK_INT >= 29) ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC else 0
    ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, type)
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    timer.removeCallbacksAndMessages(null)
    wakeLock?.release()
    wifiLock?.release()
    super.onDestroy()
  }
}
