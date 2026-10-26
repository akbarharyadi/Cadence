package com.cadence.player

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioMixerAttributes
import android.os.Build
import android.os.Handler
import android.os.Looper

/**
 * Direct output: bit-perfect playback to a USB DAC (Android 14+). Android is asked to pass Cadence's
 * music to the DAC untouched (no mixing, resampling or volume), at the song's own sample rate. The
 * audio engine (Rust, src/audio/android_direct.rs) calls this before opening its stream, then plays
 * exactly the format chosen here.
 *
 * Without bit-perfect, [prepareNative] still asks Android to run the DAC at the song's rate, so the
 * mixer passes it on without resampling.
 */
object DirectOutput {
  private val media = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()
  private var dac: AudioDeviceInfo? = null
  private val main = Handler(Looper.getMainLooper())
  /** Giving the DAC back waits a moment: the engine reopens its stream right away on a rate change or
   *  after Android interrupted it, and switching the DAC back and forth in between would interrupt it
   *  again (and again). */
  private var pendingRelease: Runnable? = null

  /** The most precise encodings first, as bits; all of them are formats AAudio can play. */
  private val encodings = listOf(
    AudioFormat.ENCODING_PCM_32BIT to 32,
    AudioFormat.ENCODING_PCM_24BIT_PACKED to 24,
    AudioFormat.ENCODING_PCM_16BIT to 16,
  )

  private fun usbOutput(audio: AudioManager): AudioDeviceInfo? =
    audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).firstOrNull {
      it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET
    }

  private fun nameOf(usb: AudioDeviceInfo) = usb.productName?.toString()?.takeIf { it.isNotBlank() } ?: "USB DAC"

  /** The DAC's most precise stereo format at `rate` with the given mixer behavior, and its bits. */
  private fun pick(audio: AudioManager, usb: AudioDeviceInfo, behavior: Int, rate: Int): Pair<AudioMixerAttributes, Int>? {
    if (Build.VERSION.SDK_INT < 34) return null
    val atRate = audio.getSupportedMixerAttributes(usb).filter {
      it.mixerBehavior == behavior && it.format.sampleRate == rate && it.format.channelCount == 2
    }
    return encodings.firstNotNullOfOrNull { (encoding, bits) ->
      atRate.firstOrNull { it.format.encoding == encoding }?.let { it to bits }
    }
  }

  /**
   * Makes `attributes` the DAC's preferred mixer setting. Answers "<bits> <name>" for a new setting,
   * "=<bits> <name>" when it was already set (Android doesn't need to switch), or null when refused.
   */
  private fun prefer(audio: AudioManager, usb: AudioDeviceInfo, attributes: AudioMixerAttributes, bits: Int): String? {
    if (Build.VERSION.SDK_INT < 34) return null
    val name = nameOf(usb)
    if (dac?.id == usb.id && audio.getPreferredMixerAttributes(media, usb) == attributes) return "=$bits $name"
    if (!audio.setPreferredMixerAttributes(media, usb, attributes)) return null
    dac = usb
    return "$bits $name"
  }

  /** A new setting replaces the old one: no need to give the DAC back in between. */
  private fun cancelRelease() = synchronized(this) {
    pendingRelease?.let { main.removeCallbacks(it) }
    pendingRelease = null
  }

  /**
   * Sets the USB DAC to play `rate` bit-perfect. Answers "<bits> <DAC name>" (with "=" first when it
   * was already set), or "!" and why it isn't possible (shown to the user).
   */
  @JvmStatic
  fun prepare(context: Context, rate: Int): String {
    cancelRelease()
    if (Build.VERSION.SDK_INT < 34) return "!it needs Android 14 or newer"
    val audio = context.getSystemService(AudioManager::class.java)
    val usb = usbOutput(audio) ?: return "!no USB DAC is connected"
    val name = nameOf(usb)
    val bitPerfect = audio.getSupportedMixerAttributes(usb).filter {
      it.mixerBehavior == AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT
    }
    if (bitPerfect.isEmpty()) return "!this phone doesn't offer bit-perfect USB playback"
    val (attributes, bits) = pick(audio, usb, AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT, rate)
      ?: return "!$name can't play $rate Hz bit-perfect"
    return prefer(audio, usb, attributes, bits) ?: "!Android didn't allow it"
  }

  /**
   * Asks Android to run the USB DAC's mixer at `rate` (Android 14+), so music at that rate reaches
   * the DAC without being resampled. Answers "<bits> <DAC name>" when it did (with "=" first when it
   * was already set), "0 <DAC name>" when it couldn't ask, or "" when no USB DAC is connected.
   */
  @JvmStatic
  fun prepareNative(context: Context, rate: Int): String {
    cancelRelease()
    val audio = context.getSystemService(AudioManager::class.java)
    val usb = usbOutput(audio) ?: return ""
    val name = nameOf(usb)
    val (attributes, bits) = pick(audio, usb, AudioMixerAttributes.MIXER_BEHAVIOR_DEFAULT, rate) ?: return "0 $name"
    return prefer(audio, usb, attributes, bits) ?: "0 $name"
  }

  /**
   * For phones where asking for the DAC's rate cuts the stream off (they rebuild their output for
   * it every time a stream starts): drops any setting right away and only answers "0 <DAC name>",
   * or "" when no USB DAC is connected. Many of these phones run the DAC at the music's rate anyway.
   */
  @JvmStatic
  fun plainNative(context: Context, @Suppress("UNUSED_PARAMETER") rate: Int): String {
    cancelRelease()
    val audio = context.getSystemService(AudioManager::class.java)
    synchronized(this) {
      dac?.let { if (Build.VERSION.SDK_INT >= 34) audio.clearPreferredMixerAttributes(media, it) }
      dac = null
    }
    return usbOutput(audio)?.let { "0 ${nameOf(it)}" } ?: ""
  }

  /**
   * Gives the DAC back to Android's usual mixer, so other apps and notifications are heard again
   * (a moment later, unless Cadence sets it again first).
   */
  @JvmStatic
  fun release(context: Context, @Suppress("UNUSED_PARAMETER") rate: Int): String {
    val audio = context.getSystemService(AudioManager::class.java)
    synchronized(this) {
      pendingRelease?.let { main.removeCallbacks(it) }
      val task = Runnable {
        synchronized(this) {
          pendingRelease = null
          val usb = dac ?: return@Runnable
          dac = null
          if (Build.VERSION.SDK_INT >= 34) audio.clearPreferredMixerAttributes(media, usb)
        }
      }
      pendingRelease = task
      main.postDelayed(task, 2000)
    }
    return ""
  }
}
