package com.cadence.player

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject

class MainActivity : TauriActivity() {
  private var webView: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Android 15+ draws apps under the status and gesture bars; keep Cadence's page clear of them,
    // with the app's own dark colour behind the bars.
    val content = findViewById<android.view.View>(android.R.id.content)
    content.setBackgroundColor(android.graphics.Color.parseColor("#10111A"))
    ViewCompat.setOnApplyWindowInsetsListener(content) { v, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
      v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
    // Cadence plays the music stored on the phone, so it needs permission to read audio files,
    // and (Android 13+) to show the playback notification. When the user answers, the app regains
    // focus and rescans (see the window focus handler in the app).
    val wanted = mutableListOf(
      if (Build.VERSION.SDK_INT >= 33) Manifest.permission.READ_MEDIA_AUDIO else Manifest.permission.READ_EXTERNAL_STORAGE
    )
    if (Build.VERSION.SDK_INT >= 33) wanted += Manifest.permission.POST_NOTIFICATIONS
    val missing = wanted.filter { ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED }
    if (missing.isNotEmpty()) ActivityCompat.requestPermissions(this, missing.toTypedArray(), 1)

    // Notification and lock-screen buttons go to the player in the app.
    PlaybackService.onCommand = { command ->
      runOnUiThread {
        webView?.evaluateJavascript("window.__cadenceMedia && window.__cadenceMedia(${JSONObject.quote(command)})", null)
      }
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    this.webView = webView
    webView.addJavascriptInterface(Bridge(), "CadenceAndroid")
  }

  /** Called from the app's JavaScript with what's playing. */
  inner class Bridge {
    @JavascriptInterface
    fun update(json: String) {
      val o = JSONObject(json)
      PlaybackService.title = o.optString("title")
      PlaybackService.artist = o.optString("artist")
      PlaybackService.album = o.optString("album")
      PlaybackService.artPath = o.optString("art")
      PlaybackService.playing = o.optBoolean("playing")
      PlaybackService.positionMs = (o.optDouble("position", 0.0) * 1000).toLong()
      PlaybackService.durationMs = (o.optDouble("duration", 0.0) * 1000).toLong()
      PlaybackService.reportedAt = android.os.SystemClock.elapsedRealtime()
      PlaybackService.quality = o.optString("quality")
      PlaybackService.accent = o.optString("accent")
      PlaybackService.update(this@MainActivity)
    }
  }
}
