//! Direct output on Android: bit-perfect playback to a USB DAC (Android 14 and later).
//!
//! Android normally mixes every app's sound and resamples it, usually to 48 kHz. Since Android 14 an
//! app may ask for a USB device's mixer to be "bit-perfect" for its music: nothing mixed, resampled
//! or scaled. The Kotlin side (`DirectOutput.kt`) picks the DAC's most precise format at the song's
//! rate and sets it; the stream here then has to play exactly that format (rate, stereo, 16, 24 or
//! 32-bit). cpal only opens 16-bit and float streams on Android, hence AAudio directly.
//!
//! Without bit-perfect (and in shared mode), Cadence still plays at the song's own rate
//! ([`open_native`]): Android 14+ is asked to run the DAC at that rate, and many phones switch it
//! anyway (as for FiiO Music), so nothing resamples the music on its way through the mixer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow};
use ndk::audio::{
    AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode, AudioSharingMode, AudioStream,
    AudioStreamBuilder,
};
use ringbuf::HeapRb;
use ringbuf::traits::Split;

use super::output::{Backend, Output, Renderer, Shared};
use super::pcm::{Dither, Format, encode};

/// Keeps the stream playing; dropping it stops it and gives the DAC back to Android's mixer.
pub struct DirectStream {
    stream: Option<AudioStream>,
    _mixer: Mixer,
}

impl Drop for DirectStream {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.request_stop();
        }
        // `_mixer` goes after the stream is closed.
    }
}

/// The DAC's mixer set to bit-perfect for Cadence; dropping it sets it back to normal, so other
/// apps and notifications are heard again.
struct Mixer;

impl Drop for Mixer {
    fn drop(&mut self) {
        if let Err(e) = kotlin("release", 0) {
            log::warn!("couldn't give the USB DAC back to Android: {e:#}");
        }
    }
}

/// Plays to the USB DAC, bit-perfect at `rate`. Fails (so the engine falls back to Android's mixer)
/// when there's no USB DAC, the phone doesn't offer bit-perfect USB playback, or the DAC can't play
/// that rate.
pub fn open(shared: Arc<Shared>, rate: u32) -> Result<Output> {
    // "24 FiiO K3": the bits Android set the DAC to, and its name; or "!" and why it can't.
    let answer = kotlin("prepare", rate)?;
    if let Some(why) = answer.strip_prefix('!') {
        return Err(anyhow!("{why}"));
    }
    let mixer = Mixer;
    let (bits, device) = answer.split_once(' ').unwrap_or((answer.as_str(), "USB DAC"));
    let unchanged = bits.starts_with('=');
    let bits: usize = bits.trim_start_matches('=').parse().map_err(|_| anyhow!("unexpected answer {answer:?}"))?;
    if !unchanged {
        settle();
    }
    // AAudio's format numbers: 1 = 16-bit, 3 = packed 24-bit, 4 = 32-bit (the latter two from
    // Android 12, which ndk's enum doesn't name).
    let format = match bits {
        16 => AudioFormat::PCM_I16,
        24 => AudioFormat::from(3),
        _ => AudioFormat::from(4),
    };
    let pcm = Format { store_bits: bits, valid_bits: bits };
    let channels = 2;

    // Half a second of buffer, as in the other modes.
    let (prod, cons) = HeapRb::<f32>::new((rate as usize / 2) * channels).split();
    let mut renderer = Renderer::new(cons, shared.clone(), channels, rate, true);
    let mut floats: Vec<f32> = Vec::new();
    let mut dither = Dither::default();
    let failed = shared.clone();
    let stream = AudioStreamBuilder::new()?
        .direction(AudioDirection::Output)
        .sample_rate(rate as i32)
        .channel_count(channels as i32)
        .format(format)
        .sharing_mode(AudioSharingMode::Shared)
        // The normal path (not the low-latency one), which is where Android attaches bit-perfect playback.
        .performance_mode(AudioPerformanceMode::None)
        .data_callback(Box::new(move |_, data, frames| {
            let frames = frames.max(0) as usize;
            floats.resize(frames * channels, 0.0);
            renderer.render(&mut floats);
            // SAFETY: AAudio hands over a buffer of `frames` frames in the stream's format.
            let out = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, frames * channels * bits / 8) };
            encode(&floats, out, pcm, &mut dither);
            AudioCallbackResult::Continue
        }))
        .error_callback(Box::new(move |_, e| {
            // Usually the DAC was unplugged: the engine reopens the output.
            log::warn!("USB DAC stream error: {e:?}");
            failed.failed.store(true, Ordering::Release);
        }))
        .open_stream()?;
    // Anything else would go through Android's mixer after all.
    if stream.sample_rate() != rate as i32 || stream.format() != format || stream.channel_count() != channels as i32 {
        return Err(anyhow!(
            "Android opened {} Hz, {:?} instead of {rate} Hz, {bits}-bit",
            stream.sample_rate(),
            stream.format()
        ));
    }
    stream.request_start()?;
    log::info!("direct output: {device}, {bits}-bit at {rate} Hz");
    Ok(Output {
        backend: Backend::Direct(DirectStream { stream: Some(stream), _mixer: mixer }),
        prod,
        rate,
        channels,
        device: device.to_string(),
        bits: Some(bits as u32),
    })
}

/// Set once asking Android for the DAC's rate got the stream cut off: some phones (MediaTek's
/// "HiFi audio") then rebuild their output every time a stream starts. They switch the DAC to the
/// music's rate by themselves, so from then on Cadence only plays at the song's rate.
static DONT_ASK: AtomicBool = AtomicBool::new(false);

/// Keeps a stream through Android's mixer playing; dropping it gives the DAC its usual rate back.
pub struct NativeStream {
    _stream: cpal::Stream,
    /// Whether Android was asked to run the DAC at the stream's rate.
    asked: bool,
    // After the stream (fields drop in order).
    _mixer: Option<Mixer>,
}

/// `out` was cut off right after it opened. If Android had been asked for the DAC's rate, that's the
/// likely cause: stop asking (until Cadence restarts).
pub fn interrupted(out: &Output) {
    if let Backend::Native(n) = &out.backend
        && n.asked
        && !DONT_ASK.swap(true, Ordering::Relaxed)
    {
        log::warn!("asking Android for the DAC's rate cut the stream off; not asking any more");
    }
}

/// Plays through Android's mixer at `rate`, the song's own. With a USB DAC on Android 14+, Android
/// is asked to run the DAC at that rate too (see [`DONT_ASK`]).
pub fn open_native(shared: Arc<Shared>, rate: u32) -> Result<Output> {
    // "32 FiiO K3": the DAC's mixer now runs at `rate` with that many bits ("=32 FiiO K3": it already
    // did); "0 FiiO K3": a USB DAC Android wasn't asked about; "": no USB DAC.
    let method = if DONT_ASK.load(Ordering::Relaxed) { "plainNative" } else { "prepareNative" };
    let answer = kotlin(method, rate).unwrap_or_else(|e| {
        log::warn!("couldn't ask Android for {rate} Hz: {e:#}");
        String::new()
    });
    let (bits, dac) = answer.split_once(' ').map_or(("0", None), |(b, name)| (b, Some(name)));
    let unchanged = bits.starts_with('=');
    let bits: u32 = bits.trim_start_matches('=').parse().unwrap_or(0);
    let mixer = (bits > 0).then_some(Mixer);
    if bits > 0 && !unchanged {
        settle();
    }
    let out = super::output::open_shared_at(shared, rate)?;
    let Backend::Shared(stream) = out.backend else {
        unreachable!("open_shared_at opens a shared stream")
    };
    match dac {
        Some(dac) if bits > 0 => log::info!("native rate: {dac} mixer at {rate} Hz, {bits}-bit"),
        Some(dac) => log::info!("native rate: {dac}, Android not asked for {rate} Hz"),
        None => {}
    }
    Ok(Output {
        backend: Backend::Native(NativeStream { _stream: stream, asked: mixer.is_some(), _mixer: mixer }),
        device: dac.map_or(out.device, str::to_string),
        ..out
    })
}

/// Gives Android a moment to switch the DAC to a new mixer setting: a stream opened while it does
/// lands on the old one and is cut off as soon as the switch happens.
fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(300));
}

/// Calls `DirectOutput.<method>(context, rate)` in the app's Kotlin code.
fn kotlin(method: &str, rate: u32) -> Result<String> {
    call("DirectOutput", method, rate as i32)
}

/// Calls `<class>.<method>(context, arg)`: a static method of the app's Kotlin code
/// (`com.cadence.player`) that takes the Android context and a number and answers a string.
pub(super) fn call(class: &str, method: &str, arg: i32) -> Result<String> {
    use jni::objects::{JClass, JObject, JString, JValue};
    let ctx = ndk_context::android_context();
    // SAFETY: the VM and context Tauri registered for this process, valid while the app runs.
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };
    // A local frame frees the call's Java references even if this thread stays attached.
    let result = env.with_local_frame(8, |env| -> jni::errors::Result<String> {
        // FindClass on a thread Rust started only sees Android's own classes: ask the app's loader.
        let loader = env.call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string(format!("com.cadence.player.{class}"))?;
        let class: JClass =
            env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?.l()?.into();
        let answer = env
            .call_static_method(
                &class,
                method,
                "(Landroid/content/Context;I)Ljava/lang/String;",
                &[JValue::Object(&context), JValue::Int(arg)],
            )?
            .l()?;
        Ok(env.get_string(&JString::from(answer))?.into())
    });
    if result.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    Ok(result?)
}
