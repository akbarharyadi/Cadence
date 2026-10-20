package com.cadence.player

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.res.ColorStateList
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ColorMatrix
import android.graphics.ColorMatrixColorFilter
import android.graphics.Paint
import android.os.Build
import android.os.SystemClock
import android.view.View
import android.widget.RemoteViews

/**
 * The home-screen player widget, styled like the desktop mini player. It shows what
 * PlaybackService last reported.
 */
class PlayerWidget : AppWidgetProvider() {
  companion object {
    private var washFor: Bitmap? = null
    private var wash: Bitmap? = null

    /** Redraws every Cadence widget on the home screen. */
    fun updateAll(context: Context, art: Bitmap?) {
      val manager = AppWidgetManager.getInstance(context)
      val ids = manager.getAppWidgetIds(ComponentName(context, PlayerWidget::class.java))
      if (ids.isEmpty()) return
      manager.updateAppWidget(ids, views(context, art))
    }

    /** Whether any widget is on the home screen (no need to keep refreshing otherwise). */
    fun any(context: Context): Boolean =
      AppWidgetManager.getInstance(context).getAppWidgetIds(ComponentName(context, PlayerWidget::class.java)).isNotEmpty()

    /**
     * The cover, blurred and saturated, for the background. Shrinking it to a few pixels and
     * scaling it back up with filtering gives a soft blur without any blur library.
     */
    private fun washOf(art: Bitmap): Bitmap {
      if (art === washFor) wash?.let { return it }
      val tiny = Bitmap.createScaledBitmap(art, 10, 10, true)
      val out = Bitmap.createBitmap(160, 80, Bitmap.Config.ARGB_8888)
      val paint = Paint(Paint.FILTER_BITMAP_FLAG).apply {
        colorFilter = ColorMatrixColorFilter(ColorMatrix().apply { setSaturation(1.6f) })
      }
      // Overscale a little, so the edges don't show the scaling.
      Canvas(out).drawBitmap(Bitmap.createScaledBitmap(tiny, 40, 40, true), null, android.graphics.Rect(-20, -40, 180, 120), paint)
      washFor = art
      wash = out
      return out
    }

    private fun command(context: Context, command: String, code: Int): PendingIntent {
      val intent = Intent(context, PlaybackService::class.java)
        .setAction(PlaybackService.ACTION_COMMAND)
        .putExtra("command", command)
      return PendingIntent.getForegroundService(context, 100 + code, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun time(ms: Long): String {
      val s = (ms / 1000).coerceAtLeast(0)
      return "%d:%02d".format(s / 60, s % 60)
    }

    /** One row when the widget is short, the full player when it has room (Android 12+ picks). */
    private fun views(context: Context, art: Bitmap?): RemoteViews {
      if (Build.VERSION.SDK_INT < 31) return layout(context, art, R.layout.widget_player)
      return RemoteViews(
        mapOf(
          android.util.SizeF(200f, 50f) to layout(context, art, R.layout.widget_player_compact),
          android.util.SizeF(200f, 136f) to layout(context, art, R.layout.widget_player),
        )
      )
    }

    private fun layout(context: Context, art: Bitmap?, id: Int): RemoteViews {
      val v = RemoteViews(context.packageName, id)
      val hasSong = PlaybackService.title.isNotEmpty()
      v.setTextViewText(R.id.widget_title, if (hasSong) PlaybackService.title else "Cadence")
      v.setTextViewText(R.id.widget_artist, if (hasSong) PlaybackService.artist else "Pick something to play")

      if (art != null) {
        v.setImageViewBitmap(R.id.widget_art, art)
        v.setImageViewBitmap(R.id.widget_wash, washOf(art))
      } else {
        v.setImageViewResource(R.id.widget_art, R.drawable.ic_stat_cadence)
        v.setImageViewBitmap(R.id.widget_wash, null)
      }

      val quality = PlaybackService.quality
      v.setViewVisibility(R.id.widget_quality_row, if (hasSong && quality.isNotEmpty()) View.VISIBLE else View.GONE)
      v.setTextViewText(R.id.widget_quality, quality)

      // Where the song is now: the last reported position, moved on by the time since, if playing.
      val duration = PlaybackService.durationMs
      var position = PlaybackService.positionMs
      if (PlaybackService.playing) position += SystemClock.elapsedRealtime() - PlaybackService.reportedAt
      position = position.coerceIn(0, maxOf(duration, 0))
      v.setTextViewText(R.id.widget_time, if (hasSong && duration > 0) "${time(position)} / ${time(duration)}" else "")
      v.setProgressBar(R.id.widget_progress, 1000, if (duration > 0) (position * 1000 / duration).toInt() else 0, false)
      v.setViewVisibility(R.id.widget_progress, if (hasSong) View.VISIBLE else View.INVISIBLE)
      if (Build.VERSION.SDK_INT >= 31) {
        val accent = runCatching { Color.parseColor(PlaybackService.accent) }.getOrDefault(Color.parseColor("#8B7CFF"))
        v.setColorStateList(R.id.widget_progress, "setProgressTintList", ColorStateList.valueOf(accent))
      }

      v.setImageViewResource(R.id.widget_play, if (PlaybackService.playing) R.drawable.ic_w_pause else R.drawable.ic_w_play)
      v.setOnClickPendingIntent(R.id.widget_prev, command(context, "prev", 1))
      v.setOnClickPendingIntent(R.id.widget_play, command(context, if (PlaybackService.playing) "pause" else "play", 2))
      v.setOnClickPendingIntent(R.id.widget_next, command(context, "next", 3))
      val open = PendingIntent.getActivity(
        context, 0, Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE
      )
      v.setOnClickPendingIntent(R.id.widget_root, open)
      v.setOnClickPendingIntent(R.id.widget_art, open)
      return v
    }
  }

  override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
    manager.updateAppWidget(ids, views(context, null))
    // Ask the service for the current song (and cover), if it's running.
    if (PlaybackService.title.isNotEmpty()) PlaybackService.update(context)
  }
}
