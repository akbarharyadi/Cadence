package com.cadence.player

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.media.app.NotificationCompat.MediaStyle

/**
 * Keeps Cadence playing while the screen is off or another app is open, and shows the media
 * notification (also on the lock screen and for Bluetooth buttons) through a MediaSession.
 * The music itself plays in Cadence's audio engine; this only keeps the app alive and passes
 * the buttons back to it.
 */
class PlaybackService : Service() {
  companion object {
    private const val CHANNEL = "playback"
    private const val NOTIFICATION_ID = 1
    const val ACTION_UPDATE = "com.cadence.player.UPDATE"
    const val ACTION_COMMAND = "com.cadence.player.COMMAND"

    /** Where button presses go (set by MainActivity: it forwards them to the player). */
    @Volatile var onCommand: ((String) -> Unit)? = null

    // What's playing, as last reported by the app.
    @Volatile var title = ""
    @Volatile var artist = ""
    @Volatile var album = ""
    @Volatile var artPath = ""
    @Volatile var playing = false
    @Volatile var positionMs = 0L
    @Volatile var durationMs = 0L
    /** When positionMs was reported (elapsedRealtime), to move the widget's progress on from it. */
    @Volatile var reportedAt = 0L
    /** e.g. "Hi-Res Lossless 24-bit/96 kHz". */
    @Volatile var quality = ""
    /** The album's accent colour (#rrggbb), for the widget's progress line. */
    @Volatile var accent = ""

    fun update(context: Context) {
      val intent = Intent(context, PlaybackService::class.java).setAction(ACTION_UPDATE)
      if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
    }
  }

  private lateinit var session: MediaSessionCompat
  private var wakeLock: PowerManager.WakeLock? = null
  private var shownArt = ""
  private var artBitmap: Bitmap? = null
  private val ticker = android.os.Handler(android.os.Looper.getMainLooper())
  /** While playing, moves the widget's time and progress line on every few seconds. */
  private val tick = object : Runnable {
    override fun run() {
      if (!playing || !PlayerWidget.any(this@PlaybackService)) return
      PlayerWidget.updateAll(this@PlaybackService, artBitmap)
      ticker.postDelayed(this, 5000)
    }
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    super.onCreate()
    val nm = getSystemService(NotificationManager::class.java)
    if (Build.VERSION.SDK_INT >= 26) {
      val channel = NotificationChannel(CHANNEL, "Playback", NotificationManager.IMPORTANCE_LOW)
      channel.description = "What's playing, with play and skip buttons"
      channel.setShowBadge(false)
      nm.createNotificationChannel(channel)
    }
    session = MediaSessionCompat(this, "Cadence").apply {
      setCallback(object : MediaSessionCompat.Callback() {
        override fun onPlay() = send("play")
        override fun onPause() = send("pause")
        override fun onSkipToNext() = send("next")
        override fun onSkipToPrevious() = send("prev")
        override fun onStop() = send("pause")
        override fun onSeekTo(pos: Long) = send("seek:" + (pos / 1000.0))
      })
      isActive = true
    }
    wakeLock = (getSystemService(Context.POWER_SERVICE) as PowerManager)
      .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Cadence:playback")
      .apply { setReferenceCounted(false) }
  }

  private fun send(command: String) {
    val handler = onCommand
    if (handler != null) handler(command)
    else {
      // The app isn't running (e.g. a widget tap after Android closed it): open it.
      startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_COMMAND) intent.getStringExtra("command")?.let { send(it) }
    refresh()
    return START_NOT_STICKY
  }

  private fun refresh() {
    if (artPath != shownArt) {
      shownArt = artPath
      artBitmap = if (artPath.isNotEmpty()) runCatching { BitmapFactory.decodeFile(artPath) }.getOrNull() else null
    }
    session.setMetadata(
      MediaMetadataCompat.Builder()
        .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title)
        .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, artist)
        .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, album)
        .putLong(MediaMetadataCompat.METADATA_KEY_DURATION, durationMs)
        .putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, artBitmap)
        .build()
    )
    session.setPlaybackState(
      PlaybackStateCompat.Builder()
        .setActions(
          PlaybackStateCompat.ACTION_PLAY or PlaybackStateCompat.ACTION_PAUSE or PlaybackStateCompat.ACTION_PLAY_PAUSE or
            PlaybackStateCompat.ACTION_SKIP_TO_NEXT or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS or PlaybackStateCompat.ACTION_SEEK_TO
        )
        .setState(if (playing) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED, positionMs, if (playing) 1f else 0f)
        .build()
    )

    PlayerWidget.updateAll(this, artBitmap)
    ticker.removeCallbacks(tick)
    if (playing) ticker.postDelayed(tick, 5000)
    val type = if (Build.VERSION.SDK_INT >= 29) ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK else 0
    ServiceCompat.startForeground(this, NOTIFICATION_ID, notification(), type)
    // The CPU may sleep with the screen off; keep it awake only while music plays.
    if (playing) wakeLock?.acquire(6 * 60 * 60 * 1000L) else wakeLock?.release()
    if (!playing) {
      // Paused: the notification can be swiped away, and Android may stop the app later.
      ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_DETACH)
    }
  }

  private fun action(icon: Int, label: String, command: String, code: Int): NotificationCompat.Action {
    val intent = Intent(this, PlaybackService::class.java).setAction(ACTION_COMMAND).putExtra("command", command)
    val pending = PendingIntent.getService(this, code, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    return NotificationCompat.Action(icon, label, pending)
  }

  private fun notification(): Notification {
    val open = PendingIntent.getActivity(
      this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE
    )
    return NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_stat_cadence)
      .setContentTitle(title.ifEmpty { "Cadence" })
      .setContentText(artist)
      .setLargeIcon(artBitmap)
      .setContentIntent(open)
      .setOnlyAlertOnce(true)
      .setShowWhen(false)
      .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
      .setOngoing(playing)
      .addAction(action(android.R.drawable.ic_media_previous, "Previous", "prev", 1))
      .addAction(
        if (playing) action(android.R.drawable.ic_media_pause, "Pause", "pause", 2)
        else action(android.R.drawable.ic_media_play, "Play", "play", 3)
      )
      .addAction(action(android.R.drawable.ic_media_next, "Next", "next", 4))
      .setStyle(MediaStyle().setMediaSession(session.sessionToken).setShowActionsInCompactView(0, 1, 2))
      .build()
  }

  override fun onTaskRemoved(rootIntent: Intent?) {
    // Swiping Cadence away from recent apps while paused ends it; while playing, music continues.
    if (!playing) stopSelf()
  }

  override fun onDestroy() {
    ticker.removeCallbacks(tick)
    wakeLock?.release()
    session.release()
    super.onDestroy()
  }
}
