//! Listening to a song: the vocals are separated from the music (MDX-Net, UVR's Voc_FT model),
//! then a speech model (MMS forced aligner) says every 20 ms how likely each letter is being sung.
//! The result is saved per song, so a song is only ever listened to once.
//!
//! The models run one after the other and are let go right after, so memory goes back to normal
//! when listening is done (the separator needs about 1.1 GB while it works on a piece, on the
//! processor; on the graphics card about 2 GB of its memory and 0.6 GB here).

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use ort::session::Session;
use ort::value::TensorRef;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::align::{Emissions, VOCAB};
use crate::audio::{Resampler, Source};

/// MDX-Net hears stereo 44.1 kHz as a spectrogram (6144-point FFT, hop 1024) of 256 frames and
/// 3072 bins (up to 11 kHz). Pieces of 5.9 s overlap by half an FFT on each side.
const SEP_RATE: u32 = 44_100;
const NFFT: usize = 6144;
const HOP: usize = 1024;
const BINS: usize = 3072;
const FRAMES: usize = 256;
const CHUNK: usize = HOP * (FRAMES - 1);
const TRIM: usize = NFFT / 2;
const STEP: usize = CHUNK - 2 * TRIM;

/// The letter model hears 16 kHz mono, a frame every 320 samples, in 30 s windows with 2 s of
/// context on each side (about 0.6 GB while it runs, less than the separator before it).
const ASR_RATE: u32 = 16_000;
const ASR_HOP: usize = 320;
const WINDOW: usize = 30 * ASR_RATE as usize;
const CONTEXT: usize = 2 * ASR_RATE as usize;

/// Songs longer than this aren't listened to (a whole decoded song is held in memory).
pub const LONGEST_SECS: f64 = 15.0 * 60.0;

/// On the graphics card, each piece is followed by a rest as long as the card worked on it, so it
/// stays about half free for the screen, videos and games while a song (or all of them) is
/// listened to. The processor already works at background priority with 1–2 threads.
fn rest_after(gpu: bool, worked: std::time::Duration) {
    if gpu {
        std::thread::sleep(worked);
    }
}

/// How many steps listening to a song of this length takes (for progress).
pub fn steps(seconds: f64) -> usize {
    let n = (seconds * SEP_RATE as f64) as usize;
    let asr = (seconds * ASR_RATE as f64) as usize;
    (n + TRIM).div_ceil(STEP).max(1) + asr.div_ceil(WINDOW).max(1)
}

/// The whole song as interleaved stereo at 44.1 kHz.
fn decode(path: &Path) -> Result<Vec<f32>> {
    let mut src = Source::open(path)?;
    let mut out = Vec::new();
    let mut stereo = Vec::new();
    let mut resampler: Option<Resampler> = None;
    let longest = (LONGEST_SECS * SEP_RATE as f64) as usize * 2;
    while let Some((buf, rate, ch)) = src.next_chunk()? {
        if out.len() > longest {
            bail!("too long");
        }
        stereo.clear();
        match ch {
            1 => stereo.extend(buf.iter().flat_map(|&s| [s, s])),
            2 => stereo.extend_from_slice(buf),
            // Surround: the centre (where vocals live) into both sides.
            _ => stereo.extend(buf.chunks_exact(ch).flat_map(|f| [f[0] + 0.707 * f[2], f[1] + 0.707 * f[2]])),
        }
        if rate == SEP_RATE {
            out.extend_from_slice(&stereo);
            continue;
        }
        if !resampler.as_ref().is_some_and(|r| r.matches(rate, SEP_RATE, 2)) {
            if let Some(r) = resampler.as_mut() {
                r.flush(&mut out)?;
            }
            resampler = Some(Resampler::new(rate, SEP_RATE, 2)?);
        }
        if let Some(r) = resampler.as_mut() {
            r.push(&stereo, &mut out)?;
        }
    }
    if let Some(r) = resampler.as_mut() {
        r.flush(&mut out)?;
    }
    if out.len() < 2 * SEP_RATE as usize {
        bail!("no audio");
    }
    Ok(out)
}

/// The spectrogram MDX-Net works on (`torch.stft` with `center=True`: frame `f` is centred on
/// sample `f·HOP` of the piece, reflected at its edges; periodic Hann window; not normalised).
struct Stft {
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// Sum of the squared windows at every sample of a piece.
    envelope: Vec<f32>,
    time: Vec<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
}

impl Stft {
    fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(NFFT);
        let inv = planner.plan_fft_inverse(NFFT);
        let window: Vec<f32> = (0..NFFT)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / NFFT as f64).cos()) as f32)
            .collect();
        let mut envelope = vec![0f32; CHUNK];
        for f in 0..FRAMES {
            let start = (f * HOP) as isize - TRIM as isize;
            for (j, w) in window.iter().enumerate() {
                let at = start + j as isize;
                if (0..CHUNK as isize).contains(&at) {
                    envelope[at as usize] += w * w;
                }
            }
        }
        let scratch = vec![Complex32::default(); fwd.get_scratch_len().max(inv.get_scratch_len())];
        Self {
            fwd,
            inv,
            window,
            envelope,
            time: vec![0.0; NFFT],
            spec: vec![Complex32::default(); NFFT / 2 + 1],
            scratch,
        }
    }

    /// Spectrogram of one channel of a piece (CHUNK samples): real and imaginary planes,
    /// `[BINS][FRAMES]` each.
    fn forward(&mut self, x: &[f32], re: &mut [f32], im: &mut [f32]) -> Result<()> {
        let n = x.len() as isize;
        for f in 0..FRAMES {
            let start = (f * HOP) as isize - TRIM as isize;
            for j in 0..NFFT {
                let mut at = start + j as isize;
                // torch "reflect": mirror without repeating the edge sample.
                if at < 0 {
                    at = -at;
                }
                if at >= n {
                    at = 2 * (n - 1) - at;
                }
                self.time[j] = x[at.clamp(0, n - 1) as usize] * self.window[j];
            }
            self.fwd
                .process_with_scratch(&mut self.time, &mut self.spec, &mut self.scratch)
                .map_err(|e| anyhow!("fft: {e}"))?;
            for b in 0..BINS {
                re[b * FRAMES + f] = self.spec[b].re;
                im[b * FRAMES + f] = self.spec[b].im;
            }
        }
        Ok(())
    }

    /// Back to samples: the middle of the piece (`TRIM .. TRIM + STEP`), added into `out`.
    fn inverse(&mut self, re: &[f32], im: &[f32], out: &mut [f32], gain: f32) -> Result<()> {
        let mut acc = vec![0f32; CHUNK];
        let scale = 1.0 / NFFT as f32;
        for f in 0..FRAMES {
            for b in 0..BINS {
                self.spec[b] = Complex32::new(re[b * FRAMES + f], im[b * FRAMES + f]);
            }
            self.spec[0].im = 0.0;
            for c in &mut self.spec[BINS..] {
                *c = Complex32::default();
            }
            self.inv
                .process_with_scratch(&mut self.spec, &mut self.time, &mut self.scratch)
                .map_err(|e| anyhow!("ifft: {e}"))?;
            let start = (f * HOP) as isize - TRIM as isize;
            for j in 0..NFFT {
                let at = start + j as isize;
                if (TRIM as isize..(TRIM + STEP) as isize).contains(&at) {
                    acc[at as usize] += self.time[j] * scale * self.window[j];
                }
            }
        }
        for (j, o) in out.iter_mut().enumerate() {
            let at = TRIM + j;
            *o += gain * acc[at] / self.envelope[at].max(1e-8);
        }
        Ok(())
    }
}

/// The vocals of a whole song (interleaved stereo at 44.1 kHz in), as mono at 44.1 kHz.
fn separate(model: &mut Session, gpu: bool, mix: &[f32], mut step: impl FnMut() -> bool) -> Result<Vec<f32>> {
    let n = mix.len() / 2;
    let mut vocals = vec![0f32; n];
    let mut stft = Stft::new();
    let plane = BINS * FRAMES;
    let mut piece = vec![vec![0f32; CHUNK]; 2];
    let mut spec = vec![0f32; 4 * plane];
    // The song is padded with silence: TRIM before it, and up to a whole step after.
    let mut start = 0usize;
    while start < n {
        for (c, ch) in piece.iter_mut().enumerate() {
            for (j, v) in ch.iter_mut().enumerate() {
                let at = (start + j) as isize - TRIM as isize;
                *v = if (0..n as isize).contains(&at) { mix[at as usize * 2 + c] } else { 0.0 };
            }
        }
        for c in 0..2 {
            let (re, rest) = spec[2 * c * plane..].split_at_mut(plane);
            stft.forward(&piece[c], re, &mut rest[..plane])?;
        }
        let started = std::time::Instant::now();
        let outputs = model.run(ort::inputs![
            "input" => TensorRef::from_array_view(([1usize, 4, BINS, FRAMES], &spec[..]))?,
        ])?;
        let worked = started.elapsed();
        let (_, out) = outputs["output"].try_extract_tensor::<f32>()?;
        if out.len() < 4 * plane {
            bail!("unexpected separation output");
        }
        let len = STEP.min(n - start);
        let mut mono = vec![0f32; STEP];
        for c in 0..2 {
            let re = &out[2 * c * plane..(2 * c + 1) * plane];
            let im = &out[(2 * c + 1) * plane..(2 * c + 2) * plane];
            // UVR's level correction for this model, halved for the mono mix of both channels.
            stft.inverse(re, im, &mut mono, 1.021 * 0.5)?;
        }
        vocals[start..start + len].copy_from_slice(&mono[..len]);
        drop(outputs);
        rest_after(gpu, worked);
        if !step() {
            bail!("stopped");
        }
        start += STEP;
    }
    Ok(vocals)
}

/// Letter log-probabilities for the vocals (mono 44.1 kHz).
fn hear(model: &mut Session, gpu: bool, vocals: &[f32], mut step: impl FnMut() -> bool) -> Result<Emissions> {
    let mut audio = Vec::with_capacity(vocals.len() * 4 / 11 + 1024);
    let mut r = Resampler::new(SEP_RATE, ASR_RATE, 1)?;
    // A piece at a time: the resampler moves what's left of its input after every 1024 samples,
    // so the whole song at once took 7–12 s.
    for piece in vocals.chunks(1 << 16) {
        r.push(piece, &mut audio)?;
    }
    r.flush(&mut audio)?;
    let v = VOCAB.len();
    let mut logp = Vec::with_capacity(audio.len() / ASR_HOP * v);
    let mut frames = 0;
    let mut start = 0;
    while start < audio.len() {
        let a = start.saturating_sub(CONTEXT);
        let b = (start + WINDOW + CONTEXT).min(audio.len());
        let started = std::time::Instant::now();
        let outputs = model.run(ort::inputs![
            "audio" => TensorRef::from_array_view(([1usize, b - a], &audio[a..b]))?,
        ])?;
        let worked = started.elapsed();
        let (shape, out) = outputs["logprobs"].try_extract_tensor::<f32>()?;
        if shape.len() != 3 || shape[2] as usize != v {
            bail!("unexpected letter model output {shape:?}");
        }
        let got = shape[1] as usize;
        let f0 = (start - a) / ASR_HOP;
        let want = ((start + WINDOW).min(audio.len()) - start) / ASR_HOP;
        for f in f0..(f0 + want).min(got) {
            logp.extend_from_slice(&out[f * v..(f + 1) * v]);
            frames += 1;
        }
        drop(outputs);
        rest_after(gpu, worked);
        if !step() {
            bail!("stopped");
        }
        start += WINDOW;
    }
    Ok(Emissions { frames, logp })
}

/// Listens to a whole song: `open(true)` gives the separator, `open(false)` the letter model,
/// each loaded only while it's needed, and whether it's on the graphics card. `step` is called
/// after each piece of work; returning false stops.
pub fn listen(
    path: &Path,
    mut open: impl FnMut(bool) -> Result<(Session, bool)>,
    mut step: impl FnMut() -> bool,
) -> Result<Emissions> {
    let mix = decode(path).context("couldn't read the song")?;
    let vocals = {
        let (mut separator, gpu) = open(true)?;
        separate(&mut separator, gpu, &mix, &mut step)?
    };
    drop(mix);
    let (mut letters, gpu) = open(false)?;
    hear(&mut letters, gpu, &vocals, &mut step)
}

// ---------- saved listening ----------

const MAGIC: &[u8; 4] = b"CDVX";
const FILE_VERSION: u32 = 1;

/// Saves the letter probabilities (a tenth of a nat per step, about 1.4 KB per second of song).
pub fn save(path: &Path, stamp: &str, em: &Emissions) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = Vec::with_capacity(16 + stamp.len() + em.logp.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FILE_VERSION.to_le_bytes());
    out.extend_from_slice(&(em.frames as u32).to_le_bytes());
    out.extend_from_slice(&(VOCAB.len() as u32).to_le_bytes());
    out.extend_from_slice(&(stamp.len() as u32).to_le_bytes());
    out.extend_from_slice(stamp.as_bytes());
    out.extend(em.logp.iter().map(|&p| (-p * 10.0).round().clamp(0.0, 255.0) as u8));
    let tmp = path.with_extension("part");
    std::fs::File::create(&tmp)?.write_all(&out)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Whether a song's listening is saved (for this same file), reading only the file's header.
pub fn is_saved(path: &Path, stamp: &str) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = vec![0u8; 20 + stamp.len()];
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    let u32_at = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    &head[..4] == MAGIC
        && u32_at(4) == FILE_VERSION
        && u32_at(12) as usize == VOCAB.len()
        && u32_at(16) as usize == stamp.len()
        && &head[20..] == stamp.as_bytes()
}

/// The saved listening of a song, if it was made from the same file (`stamp`).
pub fn load(path: &Path, stamp: &str) -> Option<Emissions> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    let u32_at = |i: usize| Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?));
    if bytes.get(..4)? != MAGIC || u32_at(4)? != FILE_VERSION {
        return None;
    }
    let frames = u32_at(8)? as usize;
    let vocab = u32_at(12)? as usize;
    let stamp_len = u32_at(16)? as usize;
    let saved_stamp = bytes.get(20..20 + stamp_len)?;
    if vocab != VOCAB.len() || saved_stamp != stamp.as_bytes() {
        return None;
    }
    let body = bytes.get(20 + stamp_len..)?;
    if body.len() != frames * vocab {
        return None;
    }
    Some(Emissions {
        frames,
        logp: body.iter().map(|&q| -(q as f32) / 10.0).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrogram_round_trip() {
        let mut stft = Stft::new();
        // Below 11 kHz (the bins the model sees), a piece comes back as it was.
        let x: Vec<f32> = (0..CHUNK).map(|i| ((i as f32 * 0.013).sin() + (i as f32 * 0.0007).cos()) * 0.3).collect();
        let mut re = vec![0f32; BINS * FRAMES];
        let mut im = vec![0f32; BINS * FRAMES];
        stft.forward(&x, &mut re, &mut im).unwrap();
        let mut y = vec![0f32; STEP];
        stft.inverse(&re, &im, &mut y, 1.0).unwrap();
        let err: f32 = x[TRIM..TRIM + STEP].iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-3, "max error {err}");
    }

    #[test]
    fn saved_listening_round_trip() {
        let dir = std::env::temp_dir().join(format!("cadence-voice-test-{}", std::process::id()));
        let path = dir.join("x.bin");
        let em = Emissions {
            frames: 2,
            logp: (0..2 * VOCAB.len()).map(|i| -(i as f32) * 0.37).collect(),
        };
        save(&path, "10:20", &em).unwrap();
        let back = load(&path, "10:20").unwrap();
        assert_eq!(back.frames, 2);
        assert!(back.logp.iter().zip(&em.logp).all(|(a, b)| (a - b).abs() <= 0.051 || *b < -25.5));
        assert!(load(&path, "10:21").is_none());
        assert!(is_saved(&path, "10:20"));
        assert!(!is_saved(&path, "10:21") && !is_saved(&path, "10:200"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
