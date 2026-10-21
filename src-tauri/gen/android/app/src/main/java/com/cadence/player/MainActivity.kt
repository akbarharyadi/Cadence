package com.cadence.player

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.net.Uri
import androidx.activity.OnBackPressedCallback
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

    // Back goes back inside Cadence (closes Now Playing, returns to the last page). With nowhere
    // left to go, Cadence moves to the background like other music players, so playback and the
    // page you were on stay as they are.
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        val view = webView
        if (view == null) {
          moveTaskToBack(true)
          return
        }
        view.evaluateJavascript("window.__cadenceBack ? window.__cadenceBack() : false") { handled ->
          if (handled != "true") moveTaskToBack(true)
        }
      }
    })

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

    /**
     * Hands a downloaded update to Android's installer. The first time, Android asks to allow
     * installs from Cadence; the answer brings the user back here to tap Update again.
     */
    @JavascriptInterface
    fun installApk(path: String): String {
      if (Build.VERSION.SDK_INT >= 26 && !packageManager.canRequestPackageInstalls()) {
        runOnUiThread {
          startActivity(
            android.content.Intent(android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName"))
          )
        }
        return "permission"
      }
      val uri = androidx.core.content.FileProvider.getUriForFile(this@MainActivity, "$packageName.fileprovider", java.io.File(path))
      val intent = android.content.Intent(android.content.Intent.ACTION_VIEW)
        .setDataAndType(uri, "application/vnd.android.package-archive")
        .addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
      runOnUiThread { startActivity(intent) }
      return "installing"
    }
  }
}
