//! Streaming sample-rate conversion (FFT-based, from `rubato`) for when a song's rate
//! differs from the output device's rate. Only used when needed: matching rates pass through untouched.

use anyhow::Result;
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

const CHUNK: usize = 1024;

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
        let inner = Fft::<f32>::new(
            rate_in as usize,
            rate_out as usize,
            CHUNK,
            channels,
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
