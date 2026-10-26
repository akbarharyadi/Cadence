# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# Cadence: the JavaScript bridge (CadenceAndroid.update) and the playback service / widget.
-keepclassmembers class com.cadence.player.** {
   @android.webkit.JavascriptInterface public <methods>;
}
-keep class com.cadence.player.PlaybackService { *; }
-keep class com.cadence.player.PlayerWidget { *; }

# Called from the audio engine (Rust) over JNI, so R8 can't see it's used.
-keep class com.cadence.player.DirectOutput { public static *; }
-keep class com.cadence.player.UsbDac { public static *; }
