//! Audio output: the device stream and the real-time renderer that feeds it from the ring buffer.
//!
//! The renderer is shared by every backend. It applies volume, short fades (so pause, seek and skip
//! never click), measures the peak level, and only clamps when the device takes integer samples —
//! float output passes through untouched.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use ringbuf::traits::{Consumer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

/// Length of the fade on pause/resume, seek and skip.
const FADE_MS: f32 = 12.0;

pub struct Shared {
    pub playing: AtomicBool,
    pub volume: AtomicU32,
    /// Device frames handed to the device since the last flush.
    pub consumed: AtomicU64,
    /// Set by the worker to ask the renderer to drop everything buffered; cleared by the renderer.
    pub flush: AtomicBool,
    /// The device went away or changed; the worker reopens it.
    pub failed: AtomicBool,
    /// Highest output magnitude since the worker last read it (f32 bits; positive floats order like u32).
    pub peak: AtomicU32,
    /// Frames the device asked for while playing that the ring couldn't supply (dropouts).
    pub underrun: AtomicU64,
    /// Samples cut off at full scale (integer devices only).
    pub clipped: AtomicU64,
    /// Diagnostics: record what goes to the device into a file in the temp folder.
    pub tap: AtomicBool,
}

impl Shared {
    pub fn new(volume: f32) -> Self {
        Self {
            playing: AtomicBool::new(false),
            volume: AtomicU32::new(volume.to_bits()),
            consumed: AtomicU64::new(0),
            flush: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            peak: AtomicU32::new(0),
            underrun: AtomicU64::new(0),
            clipped: AtomicU64::new(0),
            tap: AtomicBool::new(false),
        }
    }

    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::AcqRel))
    }
}

/// Real-time side: pulls samples from the ring and writes device buffers.
pub struct Renderer {
    cons: HeapCons<f32>,
    shared: Arc<Shared>,
    channels: usize,
    /// Current fade gain (0 = silent, 1 = full) and how much it moves per frame.
    fade: f32,
    fade_step: f32,
    clamp: bool,
    /// Open while recording (see `Shared::tap`).
    tap: Option<std::io::BufWriter<std::fs::File>>,
    /// Frames still to play before a dry ring counts as a dropout (the pause right after a skip or
    /// a new stream is expected, not a glitch).
    settle: usize,
}

impl Renderer {
    pub fn new(
        cons: HeapCons<f32>,
        shared: Arc<Shared>,
        channels: usize,
        rate: u32,
        clamp: bool,
    ) -> Self {
        let fade_step = 1.0 / (rate as f32 * FADE_MS / 1000.0).max(1.0);
        let rate = rate;
        Self {
            cons,
            shared,
            channels,
            fade: 0.0,
            fade_step,
            clamp,
            tap: None,
            settle: rate as usize / 5,
        }
    }

    /// Fills `out` (interleaved f32) with the next audio.
    pub fn render(&mut self, out: &mut [f32]) {
        let music = self.render_music(out);
        // Exclusive mode: many DACs on a digital input mute on exact digital silence and click when
        // sound returns (heard on every skip). Gaps get a ±1-LSB (24-bit, ≈ −138 dBFS) signal instead,
        // which keeps them awake and is far below hearing. The music itself is never touched.
        if self.clamp {
            let lsb = 1.0 / 8_388_608.0;
            for (i, s) in out[music..].iter_mut().enumerate() {
                if *s == 0.0 {
                    *s = if (i / self.channels) % 2 == 0 { lsb } else { -lsb };
                }
            }
        }
    }

    /// Renders what's in the ring (with fades and volume); returns how many samples are music.
    fn render_music(&mut self, out: &mut [f32]) -> usize {
        if self.shared.flush.load(Ordering::Acquire) {
            // Fade out what's playing, then drop the rest. Device buffers can be shorter than the
            // fade (exclusive mode: a few ms), so the fade carries on over as many buffers as it
            // needs; cutting it short is heard as a click on every skip.
            let n = self.pop_into(out);
            self.ramp(&mut out[..n], 0.0);
            self.apply_volume(&mut out[..n]);
            out[n..].fill(0.0);
            if self.fade == 0.0 || n < out.len() {
                self.cons.clear();
                self.fade = 0.0;
                self.settle = self.settle.max(self.fade_frames() * 16);
                self.shared.flush.store(false, Ordering::Release);
            }
            self.finish(out, n);
            return if self.fade == 0.0 { 0 } else { n };
        }
        let playing = self.shared.playing.load(Ordering::Relaxed);
        if !playing && self.fade == 0.0 {
            out.fill(0.0);
            self.record(out);
            return 0;
        }
        let n = self.pop_into(out);
        let frames = out.len() / self.channels;
        if self.settle > 0 {
            self.settle = self.settle.saturating_sub(frames);
        } else if playing && n < out.len() {
            self.shared
                .underrun
                .fetch_add(((out.len() - n) / self.channels) as u64, Ordering::Relaxed);
        }
        out[n..].fill(0.0);
        let target = if playing { 1.0 } else { 0.0 };
        self.ramp(&mut out[..n], target);
        self.apply_volume(&mut out[..n]);
        self.finish(out, n);
        n
    }

    /// The volume slider. Every path to the device must go through this: in exclusive mode it's the
    /// only volume control, so a missed buffer plays at full level (heard as a burst on every skip).
    fn apply_volume(&self, samples: &mut [f32]) {
        let volume = f32::from_bits(self.shared.volume.load(Ordering::Relaxed));
        if volume != 1.0 {
            samples.iter_mut().for_each(|s| *s *= volume);
        }
    }

    /// Length of a fade, in frames.
    fn fade_frames(&self) -> usize {
        (1.0 / self.fade_step) as usize
    }

    /// Frames waiting in the ring.
    pub fn buffered_frames(&self) -> usize {
        use ringbuf::traits::Observer;
        self.cons.occupied_len() / self.channels.max(1)
    }

    fn pop_into(&mut self, out: &mut [f32]) -> usize {
        let n = self.cons.pop_slice(out);
        n / self.channels * self.channels
    }

    /// Moves the fade gain towards `target`, frame by frame.
    fn ramp(&mut self, samples: &mut [f32], target: f32) {
        if self.fade == target {
            if target == 0.0 {
                samples.fill(0.0);
            }
            return;
        }
        for frame in samples.chunks_exact_mut(self.channels) {
            self.fade = if target > self.fade {
                (self.fade + self.fade_step).min(target)
            } else {
                (self.fade - self.fade_step).max(target)
            };
            frame.iter_mut().for_each(|s| *s *= self.fade);
        }
    }

    /// Meters, clamps (integer devices only) and counts consumed frames.
    fn finish(&mut self, out: &mut [f32], popped: usize) {
        let peak = out[..popped].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        self.shared.peak.fetch_max(peak.to_bits(), Ordering::AcqRel);
        if self.clamp && peak > 1.0 {
            let over = out.iter().filter(|s| s.abs() > 1.0).count();
            self.shared.clipped.fetch_add(over as u64, Ordering::Relaxed);
            out.iter_mut().for_each(|s| *s = s.clamp(-1.0, 1.0));
        }
        self.shared
            .consumed
            .fetch_add((popped / self.channels) as u64, Ordering::AcqRel);
        self.record(out);
    }

    /// Diagnostics: appends the device output (f32, interleaved) to `%TEMP%\cadence-tap.f32`.
    fn record(&mut self, out: &[f32]) {
        use std::io::Write;
        if !self.shared.tap.load(Ordering::Relaxed) {
            self.tap = None;
            return;
        }
        if self.tap.is_none() {
            let path = std::env::temp_dir().join("cadence-tap.f32");
            self.tap = std::fs::File::create(path).ok().map(std::io::BufWriter::new);
        }
        if let Some(w) = &mut self.tap {
            for s in out {
                let _ = w.write_all(&s.to_le_bytes());
            }
        }
    }
}

/// What keeps the device stream alive.
pub enum Backend {
    /// Shared mode (through the system mixer), via cpal.
    Shared(#[allow(dead_code)] cpal::Stream),
    /// WASAPI exclusive mode, straight to the device.
    #[cfg(windows)]
    Exclusive(#[allow(dead_code)] super::exclusive::ExclusiveStream),
    /// macOS / Linux direct output: the device itself runs at the song's rate (see [`open_direct`]).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Direct(#[allow(dead_code)] cpal::Stream),
    /// Android direct output: bit-perfect to a USB DAC (see `android_direct`).
    #[cfg(target_os = "android")]
    Direct(#[allow(dead_code)] super::android_direct::DirectStream),
    /// Android shared mode: through Android's mixer at the song's own rate (see `android_direct`).
    #[cfg(target_os = "android")]
    Native(#[allow(dead_code)] super::android_direct::NativeStream),
    /// Android USB exclusive: Cadence's own driver streams to the DAC (see `usb_dac`).
    #[cfg(target_os = "android")]
    Usb(#[allow(dead_code)] super::usb_dac::UsbStream),
}

pub struct Output {
    pub backend: Backend,
    pub prod: HeapProd<f32>,
    pub rate: u32,
    pub channels: usize,
    pub device: String,
    /// Integer bits sent to the device (exclusive mode).
    pub bits: Option<u32>,
}

impl Output {
    pub fn exclusive(&self) -> bool {
        #[cfg(windows)]
        return matches!(self.backend, Backend::Exclusive(_));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        return matches!(self.backend, Backend::Direct(_));
        #[cfg(target_os = "android")]
        return matches!(self.backend, Backend::Direct(_) | Backend::Usb(_));
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux", target_os = "android")))]
        false
    }
}

/// How much of the music a sample format keeps (lower is better; 9 = unusable).
fn rank(f: SampleFormat) -> u8 {
    match f {
        SampleFormat::F32 => 0,
        SampleFormat::I32 => 1,
        SampleFormat::I24 => 2,
        SampleFormat::F64 => 3,
        SampleFormat::I16 => 4,
        SampleFormat::U16 => 5,
        _ => 9,
    }
}

/// The most precise format the device takes at exactly `rate` with `channels`, if any.
fn config_at(device: &cpal::Device, rate: u32, channels: u16) -> Option<cpal::SupportedStreamConfig> {
    device
        .supported_output_configs()
        .ok()?
        .filter(|c| c.channels() == channels && c.min_sample_rate() <= rate && rate <= c.max_sample_rate())
        .filter(|c| rank(c.sample_format()) < 9)
        .min_by_key(|c| rank(c.sample_format()))
        .map(|c| c.with_sample_rate(rate))
}

/// The device's own rate and channels, in the most precise sample format it accepts there.
/// Some systems (often Linux) suggest 16-bit first, which would cut 24-bit music down; float or
/// 32/24-bit keeps every bit, and the system mixer takes it from there.
fn best_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let default = device.default_output_config()?;
    Ok(match config_at(device, default.sample_rate(), default.channels()) {
        Some(b) if rank(b.sample_format()) < rank(default.sample_format()) => b,
        _ => default,
    })
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Speakers".into())
}

/// Opens the default device in shared mode (through the system mixer) at its own rate.
pub fn open_shared(shared: Arc<Shared>) -> Result<Output> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no output device"))?;
    let supported = best_config(&device)?;
    log::info!("shared output format: {} at {} Hz", supported.sample_format(), supported.sample_rate());
    let (stream, prod, rate, channels) = start(&device, supported, shared)?;
    Ok(Output {
        backend: Backend::Shared(stream),
        prod,
        rate,
        channels,
        device: device_name(&device),
        bits: None,
    })
}

/// Android: the default output at exactly `rate`, through Android's mixer, in the most precise format
/// it takes there. Android runs a USB DAC (or Bluetooth) at the music's rate when it can, so this
/// avoids resampling that the device's usual 48 kHz would need.
#[cfg(target_os = "android")]
pub fn open_shared_at(shared: Arc<Shared>, rate: u32) -> Result<Output> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow!("no output device"))?;
    let supported = config_at(&device, rate, 2).ok_or_else(|| anyhow!("Android can't play {rate} Hz"))?;
    log::info!("shared output format: {} at {rate} Hz", supported.sample_format());
    let (stream, prod, rate, channels) = start(&device, supported, shared)?;
    Ok(Output {
        backend: Backend::Shared(stream),
        prod,
        rate,
        channels,
        device: device_name(&device),
        bits: None,
    })
}

/// Direct output on macOS and Linux: the device runs at the song's own rate, so nothing resamples it.
///
/// - macOS: opening a stream at a rate makes Core Audio switch the device to it (as Audio MIDI Setup
///   would), in the device's most precise format. Other apps share that rate meanwhile.
/// - Linux: plays to the sound card's ALSA `hw:` device, around PulseAudio/PipeWire. That needs the
///   card free; while the sound server holds it, opening fails and Cadence falls back to shared mode.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn open_direct(shared: Arc<Shared>, rate: u32) -> Result<Output> {
    let host = cpal::default_host();
    #[cfg(target_os = "macos")]
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no output device"))?;
    #[cfg(target_os = "linux")]
    let device = hardware_device(&host, rate)?;
    let channels = device.default_output_config().map(|c| c.channels()).unwrap_or(2);
    let supported = config_at(&device, rate, channels)
        .or_else(|| config_at(&device, rate, 2))
        .ok_or_else(|| anyhow!("{} can't play {rate} Hz", device_name(&device)))?;
    let format = supported.sample_format();
    log::info!("direct output format: {format} at {rate} Hz");
    let bits = match format {
        SampleFormat::I16 | SampleFormat::U16 => Some(16),
        SampleFormat::I24 => Some(24),
        SampleFormat::I32 => Some(32),
        _ => None,
    };
    let (stream, prod, rate, channels) = start(&device, supported, shared)?;
    Ok(Output {
        backend: Backend::Direct(stream),
        prod,
        rate,
        channels,
        device: device_name(&device),
        bits,
    })
}

/// The sound card to play to directly: its ALSA `hw:` device (no conversion at all). A USB DAC comes
/// first, then anything but HDMI (monitors are rarely the speakers), and one that takes `rate`.
#[cfg(target_os = "linux")]
fn hardware_device(host: &cpal::Host, rate: u32) -> Result<cpal::Device> {
    let mut cards: Vec<(u8, cpal::Device)> = host
        .output_devices()?
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let pcm = id.split_once(':').map_or(id.as_str(), |(_, p)| p).to_string();
            if !pcm.starts_with("hw:") {
                return None;
            }
            let desc = format!("{} {pcm}", device_name(&d)).to_lowercase();
            let score = if desc.contains("usb") {
                0
            } else if desc.contains("hdmi") {
                2
            } else {
                1
            };
            Some((score, d))
        })
        .collect();
    cards.sort_by_key(|(score, _)| *score);
    cards
        .iter()
        .find(|(_, d)| config_at(d, rate, 2).is_some())
        .or(cards.first())
        .map(|(_, d)| d.clone())
        .ok_or_else(|| anyhow!("no sound card found"))
}

/// Builds and starts a stream in `supported`'s format, fed by a new renderer.
fn start(
    device: &cpal::Device,
    supported: cpal::SupportedStreamConfig,
    shared: Arc<Shared>,
) -> Result<(cpal::Stream, HeapProd<f32>, u32, usize)> {
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let (rate, channels) = (config.sample_rate, config.channels as usize);
    let float = matches!(format, SampleFormat::F32 | SampleFormat::F64);
    // Half a second of buffer: plenty to ride out hiccups, short enough that EQ changes are heard quickly.
    let (prod, cons) = HeapRb::<f32>::new((rate as usize / 2) * channels).split();
    let renderer = Renderer::new(cons, shared.clone(), channels, rate, !float);
    let stream = match format {
        SampleFormat::F32 => build_stream::<f32>(device, config, renderer, shared),
        SampleFormat::F64 => build_stream::<f64>(device, config, renderer, shared),
        SampleFormat::I16 => build_stream::<i16>(device, config, renderer, shared),
        SampleFormat::I32 => build_stream::<i32>(device, config, renderer, shared),
        SampleFormat::I24 => build_stream::<cpal::I24>(device, config, renderer, shared),
        SampleFormat::U16 => build_stream::<u16>(device, config, renderer, shared),
        other => return Err(anyhow!("unsupported device sample format {other}")),
    }?;
    stream.play()?;
    Ok((stream, prod, rate, channels))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut renderer: Renderer,
    shared: Arc<Shared>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let mut scratch = vec![0f32; 16_384];
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            for chunk in data.chunks_mut(scratch.len()) {
                let buf = &mut scratch[..chunk.len()];
                renderer.render(buf);
                for (d, s) in chunk.iter_mut().zip(buf.iter()) {
                    *d = T::from_sample(*s);
                }
            }
        },
        move |err: cpal::Error| {
            log::warn!("audio stream error: {err}");
            if !matches!(err.kind(), cpal::ErrorKind::Xrun) {
                shared.failed.store(true, Ordering::Release);
            }
        },
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::Producer;

    fn setup(clamp: bool) -> (HeapProd<f32>, Renderer, Arc<Shared>) {
        let shared = Arc::new(Shared::new(1.0));
        let (prod, cons) = HeapRb::<f32>::new(48_000).split();
        let r = Renderer::new(cons, shared.clone(), 2, 48_000, clamp);
        (prod, r, shared)
    }

    #[test]
    fn float_output_is_not_clamped_and_unity_is_exact() {
        let (mut prod, mut r, shared) = setup(false);
        shared.playing.store(true, Ordering::Relaxed);
        r.fade = 1.0;
        prod.push_slice(&[1.2, -0.3, 0.25, 0.5]);
        let mut out = [0.0f32; 4];
        r.render(&mut out);
        assert_eq!(out, [1.2, -0.3, 0.25, 0.5]);
        assert!((shared.take_peak() - 1.2).abs() < 1e-6);
    }

    #[test]
    fn integer_output_is_clamped() {
        let (mut prod, mut r, shared) = setup(true);
        shared.playing.store(true, Ordering::Relaxed);
        r.fade = 1.0;
        prod.push_slice(&[1.2, -1.5]);
        let mut out = [0.0f32; 2];
        r.render(&mut out);
        assert_eq!(out, [1.0, -1.0]);
    }

    #[test]
    fn resume_fades_in() {
        let (mut prod, mut r, shared) = setup(false);
        shared.playing.store(true, Ordering::Relaxed);
        prod.push_slice(&[1.0; 2000]);
        let mut out = [0.0f32; 2000];
        r.render(&mut out);
        assert!(
            out[0] < 0.01 && out[1998] > 0.99,
            "{} {}",
            out[0],
            out[1998]
        );
    }

    #[test]
    fn skip_fades_out_smoothly_even_with_tiny_buffers() {
        let (mut prod, mut r, shared) = setup(true);
        shared.playing.store(true, Ordering::Relaxed);
        r.fade = 1.0;
        prod.push_slice(&vec![0.5f32; 20_000]);
        // 64-frame device buffers (~1.3 ms at 48 kHz), much shorter than the 12 ms fade.
        let mut out = [0.0f32; 128];
        r.render(&mut out);
        shared.flush.store(true, Ordering::Release);
        let mut last = out[126];
        while shared.flush.load(Ordering::Acquire) {
            r.render(&mut out);
            for s in out.iter().step_by(2) {
                assert!((last - s).abs() < 0.01, "jump from {last} to {s}");
                last = *s;
            }
        }
        assert!(last.abs() < 1e-6, "should end in (near) silence: {last}");
    }

    #[test]
    fn skip_fade_keeps_the_volume() {
        let (mut prod, mut r, shared) = setup(true);
        shared.volume.store(0.1f32.to_bits(), Ordering::Relaxed);
        shared.playing.store(true, Ordering::Relaxed);
        r.fade = 1.0;
        prod.push_slice(&vec![0.5f32; 20_000]);
        let mut out = [0.0f32; 128];
        r.render(&mut out);
        shared.flush.store(true, Ordering::Release);
        while shared.flush.load(Ordering::Acquire) {
            r.render(&mut out);
            let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak <= 0.05 + 1e-6, "fade-out got louder than the volume allows: {peak}");
        }
    }

    #[test]
    fn exclusive_gaps_are_never_exact_zero_but_music_is_untouched() {
        let (mut prod, mut r, shared) = setup(true);
        shared.playing.store(true, Ordering::Relaxed);
        r.fade = 1.0;
        prod.push_slice(&[0.25, -0.25, 0.0, 0.0]);
        let mut out = [9.0f32; 12];
        r.render(&mut out);
        assert_eq!(&out[..4], &[0.25, -0.25, 0.0, 0.0], "music (even its own silence) unchanged");
        assert!(out[4..].iter().all(|s| *s != 0.0 && s.abs() < 1e-6), "{:?}", &out[4..]);
    }
}
