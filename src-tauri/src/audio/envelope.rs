//! Waveform envelopes for the seek bar: a small picture of how loud a song is over its length.
//!
//! The song (or the CUE-sheet span inside its file) is cut into `BUCKETS` equal stretches of
//! frames, counted exactly from the decoder's frame count, so bucket `i` of `n` starts at frame
//! `i * total / n` and the picture lines up with the seek position. Each bucket keeps its peak
//! (the highest |sample| of any channel) and its RMS (over every sample of every channel), each
//! as one byte (0..=255 for 0.0..=1.0 of full scale). Cost per sample: an abs, a compare and a
//! multiply-add, next to decoding it.

use std::path::Path;

use anyhow::{Result, anyhow};

use super::source::Source;

/// Buckets per song (fewer for a song shorter than this many frames).
pub const BUCKETS: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Envelope {
    pub peak: Vec<u8>,
    pub rms: Vec<u8>,
}

/// Fills an envelope from decoded chunks of a span `total` frames long.
pub struct Builder {
    total: u64,
    n: usize,
    /// Frames taken so far.
    frame: u64,
    /// The bucket being filled, and the first frame of the one after it.
    bucket: usize,
    next_edge: u64,
    peak: f32,
    sum: f64,
    count: u64,
    out: Envelope,
}

/// A damaged packet can decode to NaN or infinity: counted as silence.
#[inline(always)]
fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

fn quantize(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl Builder {
    /// None for an empty span (nothing to draw).
    pub fn new(total: u64) -> Option<Self> {
        if total == 0 {
            return None;
        }
        let n = (BUCKETS as u64).min(total) as usize;
        let mut b = Self {
            total,
            n,
            frame: 0,
            bucket: 0,
            next_edge: 0,
            peak: 0.0,
            sum: 0.0,
            count: 0,
            out: Envelope { peak: Vec::with_capacity(n), rms: Vec::with_capacity(n) },
        };
        b.next_edge = b.edge(1);
        Some(b)
    }

    /// The first frame of bucket `b`.
    fn edge(&self, b: usize) -> u64 {
        (b as u128 * self.total as u128 / self.n as u128) as u64
    }

    /// Every bucket is filled: the rest of the stream isn't needed.
    pub fn done(&self) -> bool {
        self.bucket >= self.n
    }

    /// Interleaved samples, `channels` per frame. Frames past the span are ignored.
    pub fn add(&mut self, samples: &[f32], channels: usize) {
        let ch = channels.max(1);
        let mut rest = &samples[..samples.len() / ch * ch];
        while !rest.is_empty() && !self.done() {
            let room = (self.next_edge - self.frame) as usize;
            let take = room.min(rest.len() / ch);
            let (now, later) = rest.split_at(take * ch);
            // Eight lanes, so the compiler can use vector instructions (one running sum can't be).
            let mut peak = [0.0f32; 8];
            let mut sum = [0.0f32; 8];
            let mut lanes = now.chunks_exact(8);
            for c in &mut lanes {
                for k in 0..8 {
                    let x = finite(c[k]);
                    peak[k] = peak[k].max(x.abs());
                    sum[k] += x * x;
                }
            }
            for (k, &x) in lanes.remainder().iter().enumerate() {
                let x = finite(x);
                peak[k] = peak[k].max(x.abs());
                sum[k] += x * x;
            }
            self.peak = peak.iter().fold(self.peak, |a, &b| a.max(b));
            self.sum += sum.iter().map(|&s| f64::from(s)).sum::<f64>();
            self.count += now.len() as u64;
            self.frame += take as u64;
            rest = later;
            if self.frame == self.next_edge {
                self.close();
            }
        }
    }

    fn close(&mut self) {
        let rms = if self.count > 0 { (self.sum / self.count as f64).sqrt() as f32 } else { 0.0 };
        self.out.peak.push(quantize(self.peak));
        self.out.rms.push(quantize(rms));
        self.bucket += 1;
        self.next_edge = self.edge(self.bucket + 1);
        self.peak = 0.0;
        self.sum = 0.0;
        self.count = 0;
    }

    /// The envelope; buckets the stream never reached (it ended early) are silent.
    pub fn finish(mut self) -> Envelope {
        while !self.done() {
            self.close();
        }
        self.out
    }
}

/// The frames of a file a song covers: from `start` seconds to `end` (None: the end of the file,
/// `file_frames` long if the header says). None when the length isn't known (no waveform then).
pub fn span_frames(start: f64, end: Option<f64>, rate: u32, file_frames: Option<u64>) -> Option<(u64, u64)> {
    let first = super::cue_frame(start, rate);
    let last = match end {
        Some(e) => super::cue_frame(e, rate),
        None => file_frames?,
    };
    (last > first).then_some((first, last - first))
}

/// Decodes a song (a span of its file for a CUE-sheet song) on its own decoder and returns its
/// envelope; None when its length isn't known.
pub fn compute(path: &Path, start: f64, end: Option<f64>) -> Result<Option<Envelope>> {
    let mut src = Source::open(path)?;
    let (rate, channels) = (src.rate, src.channels);
    let Some((first, frames)) = span_frames(start, end, rate, src.total_frames) else {
        return Ok(None);
    };
    let Some(mut b) = Builder::new(frames) else {
        return Ok(None);
    };
    if first > 0 {
        let reached = src.seek_frame(first)?;
        if reached != first {
            return Err(anyhow!("can't find where the song starts"));
        }
    }
    while let Some((samples, r, c)) = src.next_chunk()? {
        if r != rate || c != channels {
            break;
        }
        b.add(samples, c);
        if b.done() {
            break;
        }
    }
    Ok(Some(b.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, amp: f32) -> Vec<f32> {
        (0..frames).map(|i| amp * (i as f32 * 2.0 * std::f32::consts::PI / 100.0).sin()).collect()
    }

    #[test]
    fn silence_is_all_zeros() {
        let mut b = Builder::new(44_100 * 3).unwrap();
        for chunk in vec![0.0f32; 44_100 * 3 * 2].chunks(4096 * 2) {
            b.add(chunk, 2);
        }
        let e = b.finish();
        assert_eq!(e.peak.len(), BUCKETS);
        assert!(e.peak.iter().chain(&e.rms).all(|&v| v == 0));
    }

    #[test]
    fn a_burst_in_the_second_half_fills_exactly_the_second_half() {
        let total = 44_100 * 4;
        let mut x = vec![0.0f32; total / 2];
        x.extend(sine(total / 2, 0.5));
        let mut b = Builder::new(total as u64).unwrap();
        // Odd chunk sizes, so bucket edges fall inside chunks.
        for chunk in x.chunks(1153) {
            b.add(chunk, 1);
        }
        let e = b.finish();
        assert_eq!(e.peak.len(), BUCKETS);
        assert!(e.peak[..BUCKETS / 2].iter().all(|&v| v == 0), "first half silent");
        assert!(e.peak[BUCKETS / 2..].iter().all(|&v| (126..=128).contains(&v)), "{:?}", &e.peak[BUCKETS / 2..BUCKETS / 2 + 4]);
        // RMS of a 0.5 sine: 0.354 of full scale (give or take the part-period at each edge).
        assert!(e.rms[BUCKETS / 2..].iter().all(|&v| (84..=97).contains(&v)), "{:?}", &e.rms[BUCKETS / 2..BUCKETS / 2 + 4]);
    }

    #[test]
    fn stereo_takes_the_loudest_channel_and_the_rms_of_both() {
        let mono = sine(44_100, 0.5);
        let both: Vec<f32> = mono.iter().flat_map(|&v| [v, v]).collect();
        let left: Vec<f32> = mono.iter().flat_map(|&v| [v, 0.0]).collect();
        let run = |x: &[f32], ch: usize| {
            let mut b = Builder::new(44_100).unwrap();
            b.add(x, ch);
            b.finish()
        };
        let (m, s, l) = (run(&mono, 1), run(&both, 2), run(&left, 2));
        assert_eq!(m, s, "the same signal on both channels draws like mono");
        assert_eq!(m.peak, l.peak, "peak: the louder channel");
        assert!(l.rms.iter().zip(&m.rms).all(|(a, b)| a < b), "a silent channel lowers the RMS");
    }

    #[test]
    fn a_very_short_song_has_one_bucket_per_frame() {
        let mut b = Builder::new(10).unwrap();
        let x = [0.0, 0.1, -0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 1.5, 9.0, 9.0];
        b.add(&x, 1);
        let e = b.finish();
        assert_eq!(e.peak, x[..10].iter().map(|v| quantize(v.abs())).collect::<Vec<_>>());
        assert_eq!(e.peak[9], 255, "clipped samples are full scale");
        assert!(Builder::new(0).is_none());
        // A stream that ends early: the rest is silent, still the full count.
        let mut b = Builder::new(20).unwrap();
        b.add(&[1.0; 5], 1);
        let e = b.finish();
        assert_eq!(e.peak.len(), 20);
        assert_eq!(&e.peak[..6], &[255, 255, 255, 255, 255, 0]);
    }

    #[test]
    fn unknown_length_has_no_waveform() {
        assert_eq!(span_frames(0.0, None, 44_100, None), None);
        assert_eq!(span_frames(0.0, None, 44_100, Some(1000)), Some((0, 1000)));
        // A CUE song: its own span, the end of the file not needed.
        assert_eq!(span_frames(1.0, Some(2.0), 44_100, None), Some((44_100, 44_100)));
        assert_eq!(span_frames(2.0, Some(2.0), 44_100, None), None);
    }

    /// The envelope of frames `first..first + frames` of `position_wav`'s signal, worked out frame
    /// by frame from the definition.
    fn expected(first: u64, frames: u64) -> Envelope {
        let n = (BUCKETS as u64).min(frames);
        let mut e = Envelope::default();
        for b in 0..n {
            let (a, z) = (first + b * frames / n, first + (b + 1) * frames / n);
            let mut peak = 0.0f32;
            let mut sum = 0.0f64;
            for f in a..z {
                let v = (f % 30_000) as f32 / 32_768.0;
                peak = peak.max(v);
                sum += 2.0 * f64::from(v * v);
            }
            e.peak.push(quantize(peak));
            e.rms.push(quantize((sum / (2 * (z - a)) as f64).sqrt() as f32));
        }
        e
    }

    #[test]
    fn a_cue_span_covers_exactly_its_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("album.wav");
        super::super::source::tests::position_wav(&path, 44_100 * 3);
        // The second second of the file (a CUE song from 0:01 to 0:02).
        let e = compute(&path, 1.0, Some(2.0)).unwrap().unwrap();
        assert_eq!(e.peak.len(), BUCKETS);
        let want = expected(44_100, 44_100);
        let off = e.peak.iter().zip(&want.peak).filter(|(a, b)| a.abs_diff(**b) > 1).count();
        assert_eq!(off, 0, "peaks line up with the span");
        assert!(e.rms.iter().zip(&want.rms).all(|(a, b)| a.abs_diff(*b) <= 1));
        // The last song of the file runs to its end.
        let last = compute(&path, 2.0, None).unwrap().unwrap();
        let want = expected(88_200, 44_100);
        assert!(last.peak.iter().zip(&want.peak).all(|(a, b)| a.abs_diff(*b) <= 1));
        // The whole file.
        let whole = compute(&path, 0.0, None).unwrap().unwrap();
        let want = expected(0, 44_100 * 3);
        assert!(whole.peak.iter().zip(&want.peak).all(|(a, b)| a.abs_diff(*b) <= 1));
    }
}

/// A test WAV (16-bit stereo 44.1 kHz, see `source::tests::position_wav`) for tests elsewhere.
#[cfg(test)]
pub(crate) fn test_wav(path: &Path, frames: u32) {
    super::source::tests::position_wav(path, frames);
}
