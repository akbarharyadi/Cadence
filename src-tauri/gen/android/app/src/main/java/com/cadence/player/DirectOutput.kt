package com.cadence.player

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioMixerAttributes
import android.os.Build

/**
 * Direct output: bit-perfect playback to a USB DAC (Android 14+). Android is asked to pass Cadence's
 * music to the DAC untouched (no mixing, resampling or volume), at the song's own sample rate. The
 * audio engine (Rust, src/audio/android_direct.rs) calls this before opening its stream, then plays
 * exactly the format chosen here.
 */
object DirectOutput {
  private val media = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()
  private var dac: AudioDeviceInfo? = null

  /** The most precise encodings first, as bits; all of them are formats AAudio can play. */
  private val encodings = listOf(
    AudioFormat.ENCODING_PCM_32BIT to 32,
    AudioFormat.ENCODING_PCM_24BIT_PACKED to 24,
    AudioFormat.ENCODING_PCM_16BIT to 16,
  )

  /**
   * Sets the USB DAC to play `rate` bit-perfect. Answers "<bits> <DAC name>", or "!" and why it
   * isn't possible (shown to the user).
   */
  @JvmStatic
  fun prepare(context: Context, rate: Int): String {
    if (Build.VERSION.SDK_INT < 34) return "!it needs Android 14 or newer"
    val audio = context.getSystemService(AudioManager::class.java)
    val usb = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).firstOrNull {
      it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET
    } ?: return "!no USB DAC is connected"
    val name = usb.productName?.toString()?.takeIf { it.isNotBlank() } ?: "USB DAC"
    val bitPerfect = audio.getSupportedMixerAttributes(usb).filter {
      it.mixerBehavior == AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT
    }
    if (bitPerfect.isEmpty()) return "!this phone doesn't offer bit-perfect USB playback"
    val atRate = bitPerfect.filter { it.format.sampleRate == rate && it.format.channelCount == 2 }
    val (pick, bits) = encodings.firstNotNullOfOrNull { (encoding, bits) ->
      atRate.firstOrNull { it.format.encoding == encoding }?.let { it to bits }
    } ?: return "!$name can't play $rate Hz bit-perfect"
    if (!audio.setPreferredMixerAttributes(media, usb, pick)) return "!Android didn't allow it"
    dac = usb
    return "$bits $name"
  }

  /** Gives the DAC back to Android's mixer, so other apps and notifications are heard again. */
  @JvmStatic
  fun release(context: Context, @Suppress("UNUSED_PARAMETER") rate: Int): String {
    val usb = dac ?: return ""
    dac = null
    if (Build.VERSION.SDK_INT >= 34) {
      context.getSystemService(AudioManager::class.java).clearPreferredMixerAttributes(media, usb)
    }
    return ""
  }
}
