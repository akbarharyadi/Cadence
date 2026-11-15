//! Streaming sample-rate conversion (FFT-based, from `rubato`) for when a song's rate
//! differs from the output device's rate. Only used when needed: matching rates pass through untouched.

use anyhow::Result;
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _, WindowFunction};

/// Input frames per FFT block (rounded up to a whole number of the rate pair's minimum block).
///
/// One block per chunk, and a large one: the anti-aliasing filter's cutoff moves closer to Nyquist
/// as the block grows. `Fft::new` with a 1024-frame chunk used ~300-frame blocks: flat to 20 kHz,
/// but −5.1 dB at 21 kHz and −34 dB at 21.5 kHz (44.1 ↔ 48 kHz, see the tests). 8192-frame blocks
/// stay flat to 21.5 kHz. The cost is delay (half a block, about 90 ms, dropped at the start so
/// songs stay in sync and gapless) and a little more CPU per sample, both irrelevant here: the
/// worker decodes ahead into a half-second ring anyway.
const BLOCK: usize = 8192;

pub struct Resampler {
    inner: Fft<f32>,
    channels: usize,
    pub rate_in: u32,
    pub rate_out: u32,
    pending: Vec<f32>,
    out: Vec<f32>,
    /// Output frames still to drop at the start (the filter's delay), so songs stay in sync and gapless.
    skip: usize,
    frames_in: u64,
    frames_out: u64,
}

impl Resampler {
    pub fn new(rate_in: u32, rate_out: u32, channels: usize) -> Result<Self> {
        let min_in = rate_in as usize / gcd(rate_in as usize, rate_out as usize).max(1);
        let chunk = BLOCK.div_ceil(min_in.max(1)) * min_in.max(1);
        Self::with_blocks(rate_in, rate_out, channels, chunk, 1)
    }

    /// A resampler taking `chunk` input frames at a time, split into `sub_chunks` FFT blocks.
    fn with_blocks(rate_in: u32, rate_out: u32, channels: usize, chunk: usize, sub_chunks: usize) -> Result<Self> {
        let inner = Fft::<f32>::new_custom(
            rate_in as usize,
            rate_out as usize,
            chunk,
            sub_chunks,
            channels,
            WindowFunction::BlackmanHarris2,
            FixedSync::Input,
        )?;
        let skip = inner.output_delay();
        Ok(Self {
            inner,
            channels,
            rate_in,
            rate_out,
            pending: Vec::new(),
            out: Vec::new(),
            skip,
            frames_in: 0,
            frames_out: 0,
        })
    }

    pub fn matches(&self, rate_in: u32, rate_out: u32, channels: usize) -> bool {
        self.rate_in == rate_in && self.rate_out == rate_out && self.channels == channels
    }

    /// Feeds interleaved input; appends whatever output is ready to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.pending.extend_from_slice(input);
        self.frames_in += (input.len() / self.channels) as u64;
        loop {
            let need = self.inner.input_frames_next();
            if self.pending.len() / self.channels < need {
                return Ok(());
            }
            self.run(need, None, out)?;
        }
    }

    /// Pushes out the remaining input and the filter tail (end of a stream).
    pub fn flush(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let expected =
            (self.frames_in as f64 * self.rate_out as f64 / self.rate_in as f64).round() as u64;
        let mut guard = 0;
        while self.frames_out < expected && guard < 16 {
            let have = (self.pending.len() / self.channels).min(self.inner.input_frames_next());
            self.run(have, Some(have), out)?;
            guard += 1;
        }
        // Drop anything produced beyond the real length (zero padding).
        let extra = self.frames_out.saturating_sub(expected) as usize * self.channels;
        out.truncate(out.len().saturating_sub(extra));
        self.reset();
        Ok(())
    }

    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
        self.skip = self.inner.output_delay();
        self.frames_in = 0;
        self.frames_out = 0;
    }

    fn run(&mut self, frames: usize, partial: Option<usize>, out: &mut Vec<f32>) -> Result<()> {
        let ch = self.channels;
        let need = self.inner.input_frames_next();
        // A partial chunk still needs a full-size input buffer; pad with silence.
        if self.pending.len() < need * ch {
            self.pending.resize(need * ch, 0.0);
        }
        let out_max = self.inner.output_frames_max();
        self.out.resize(out_max * ch, 0.0);
        let input = InterleavedSlice::new(&self.pending[..need * ch], ch, need)?;
        let mut output = InterleavedSlice::new_mut(&mut self.out[..], ch, out_max)?;
        let indexing = partial.map(|n| Indexing {
            partial_len: Some(n),
            ..Default::default()
        });
        let (_, produced) =
            self.inner
                .process_into_buffer(&input, &mut output, indexing.as_ref())?;
        self.pending
            .drain(..frames.min(self.pending.len() / ch) * ch);
        if partial.is_some() {
            self.pending.clear();
        }
        let drop = self.skip.min(produced);
        self.skip -= drop;
        out.extend_from_slice(&self.out[drop * ch..produced * ch]);
        self.frames_out += (produced - drop) as u64;
        Ok(())
    }
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Level (dB) of a sine at `freq` after resampling, relative to the input's, measured away from
    /// the edges.
    fn level_db(r: &mut Resampler, rate_in: u32, freq: f64) -> f64 {
        let n = rate_in as usize * 2;
        let input: Vec<f32> = (0..n)
            .flat_map(|i| {
                let s = (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / rate_in as f64).sin()) as f32;
                [s, s]
            })
            .collect();
        let mut out = Vec::new();
        for c in input.chunks(4096 * 2) {
            r.push(c, &mut out).unwrap();
        }
        r.flush(&mut out).unwrap();
        let rms = |x: &[f32]| (x.iter().step_by(2).map(|s| (*s as f64).powi(2)).sum::<f64>() / (x.len() / 2) as f64).sqrt();
        let frames = out.len() / 2;
        let mid = &out[frames / 4 * 2..frames * 3 / 4 * 2];
        let reference = 0.5 / 2f64.sqrt();
        20.0 * (rms(mid) / reference).log10()
    }

    const FREQS: [f64; 9] = [
        1_000.0, 10_000.0, 16_000.0, 18_000.0, 19_000.0, 20_000.0, 21_000.0, 21_500.0, 23_000.0,
    ];

    /// `old`: the previous configuration (`Fft::new` with 1024-frame chunks = 4 sub-chunks), kept
    /// for comparison (`cargo test treble -- --nocapture` prints both).
    fn sweep(old: bool, rate_in: u32, rate_out: u32) -> Vec<(f64, f64)> {
        FREQS
            .iter()
            .filter(|&&f| f < rate_in as f64 / 2.0)
            .map(|&f| {
                let mut r = if old {
                    Resampler::with_blocks(rate_in, rate_out, 2, 1024, 1024 / 256).unwrap()
                } else {
                    Resampler::new(rate_in, rate_out, 2).unwrap()
                };
                (f, level_db(&mut r, rate_in, f))
            })
            .collect()
    }

    #[test]
    fn treble_is_kept_and_nothing_folds_back() {
        for (rin, rout) in [(44_100, 48_000), (48_000, 44_100)] {
            let before = sweep(true, rin, rout);
            let after = sweep(false, rin, rout);
            for ((f, b), (_, a)) in before.iter().zip(after.iter()) {
                println!("{rin} -> {rout}: {f:>6} Hz  before {b:+.2} dB  after {a:+.2} dB");
            }
            for (f, a) in &after {
                if *f <= 21_000.0 {
                    assert!(a.abs() < 0.5, "{rin} -> {rout} at {f} Hz: {a:+.2} dB");
                }
                // Above the output's Nyquist: filtered out, not folded back into the audio.
                if *f > rout as f64 / 2.0 {
                    assert!(*a < -100.0, "{rin} -> {rout} at {f} Hz: {a:+.2} dB");
                }
            }
        }
    }

    #[test]
    fn length_is_preserved_for_gapless() {
        for (rin, rout) in [(44_100u32, 48_000u32), (48_000, 44_100), (96_000, 48_000), (44_100, 96_000)] {
            let mut r = Resampler::new(rin, rout, 2).unwrap();
            let n = 123_457usize;
            let input = vec![0.25f32; n * 2];
            let mut out = Vec::new();
            for c in input.chunks(3000 * 2) {
                r.push(c, &mut out).unwrap();
            }
            r.flush(&mut out).unwrap();
            let expected = (n as f64 * rout as f64 / rin as f64).round() as usize;
            assert_eq!(out.len() / 2, expected, "{rin} -> {rout}");
            // The filter delay was dropped: a DC input is at level right after the start.
            let k = (rout as usize / 100) * 2;
            assert!((out[k] - 0.25).abs() < 0.01, "{rin} -> {rout}: {}", out[k]);
        }
    }
}
