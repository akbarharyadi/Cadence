//! Peak limiter for Sound Check.
//!
//! Playing songs at −14 LUFS means a quiet song with sharp peaks wants more gain than its peak
//! allows; without a limiter it is held back and stays quieter than the songs around it (788 of
//! 7,013 songs in the owner's library, 170 of them by more than 3 dB). Sound Check may now raise a
//! song up to [`ALLOWANCE_DB`] past its peak, and this limiter turns down the few peaks that would
//! then pass the ceiling.
//!
//! Stereo-linked, instant attack (no sample leaves above the ceiling, and no lookahead, so no delay
//! and nothing held back at the end of the queue), smooth release. While no peak passes the ceiling
//! the gain is exactly 1 and samples are not touched. It runs on the worker thread, not in the
//! render callback, and allocates nothing.

/// How far past its peak cap Sound Check may raise a song (dB): the most the limiter takes off a
/// peak. A few dB on short peaks is not heard as limiting.
pub const ALLOWANCE_DB: f32 = 3.0;
/// Highest output level: 1 dB below full scale, the same margin Sound Check keeps.
pub(super) const CEILING: f32 = 0.891_250_9; // 10^(−1/20)
/// Time for the gain to come back most of the way after a peak (seconds).
const RELEASE_S: f32 = 0.1;

pub struct Limiter {
    rate: u32,
    /// Fraction of the reduction let go per frame.
    release: f32,
    /// Current reduction, 1 − gain (kept instead of the gain: near 1, f32 can't hold the small
    /// steps of the release and the gain would never come back).
    cut: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Limiter {
    pub fn new() -> Self {
        let mut l = Self { rate: 0, release: 0.0, cut: 0.0 };
        l.set_rate(48_000);
        l
    }

    pub fn set_rate(&mut self, rate: u32) {
        if rate == self.rate || rate == 0 {
            return;
        }
        self.rate = rate;
        self.release = 1.0 - (-1.0 / (RELEASE_S * rate as f32)).exp();
    }

    /// Back to no reduction (a new song started from silence, or a seek).
    pub fn reset(&mut self) {
        self.cut = 0.0;
    }

    /// Limits interleaved `samples` with `ch` channels in place.
    pub fn process(&mut self, samples: &mut [f32], ch: usize) {
        for frame in samples.chunks_mut(ch.max(1)) {
            let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            // The most this frame may get. (NaN never comes from a decoder; it must not stick.)
            let most = if peak.is_finite() && peak > CEILING { CEILING / peak } else { 1.0 };
            let released = self.cut * (1.0 - self.release);
            // The release only nears 0; the last 0.001 dB is a step nobody hears.
            self.cut = (if released < 1e-4 { 0.0 } else { released }).max(1.0 - most);
            if self.cut == 0.0 {
                continue;
            }
            let gain = 1.0 - self.cut;
            frame.iter_mut().for_each(|s| *s *= gain);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = amp * (i as f32 * 0.05).sin();
                [v, v * 0.5]
            })
            .collect()
    }

    #[test]
    fn music_below_the_ceiling_is_left_exactly_as_it_was() {
        let original = sine(0.85, 4_800);
        let mut x = original.clone();
        Limiter::new().process(&mut x, 2);
        assert_eq!(x, original);
    }

    #[test]
    fn no_sample_leaves_above_the_ceiling() {
        let mut x = sine(1.4, 4_800); // 3 dB over full scale.
        Limiter::new().process(&mut x, 2);
        let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= CEILING + 1e-6, "peak {peak}");
    }

    #[test]
    fn the_level_comes_back_after_a_peak() {
        let mut l = Limiter::new();
        let mut hit = vec![2.0, 2.0];
        l.process(&mut hit, 2);
        assert!(l.cut > 0.5);
        // A second and a half of quiet music later the gain is back to 1 and leaves samples alone.
        let mut quiet = vec![0.1f32; 144_000];
        l.process(&mut quiet, 2);
        assert_eq!(l.cut, 0.0);
        assert_eq!(quiet.last(), Some(&0.1));
    }

    #[test]
    fn both_channels_are_turned_down_together() {
        let mut x = vec![1.8, 0.4];
        Limiter::new().process(&mut x, 2);
        assert!((x[0] / x[1] - 4.5).abs() < 1e-4, "the stereo image stays");
    }

    #[test]
    fn a_song_raised_past_its_peak_never_leaves_the_worker_above_the_ceiling() {
        use super::super::loudness::{GainInfo, LevelingMode, SongGain};
        use super::super::{Step, tests::real_worker};
        use ringbuf::traits::Consumer;
        // The test song ramps to 0.915 (−0.8 dBFS); Sound Check wants +3 dB on it, past its peak.
        let (mut w, mut cons, dir) = real_worker("limiter", &["a"], 40_000);
        w.leveling.mode = LevelingMode::Track;
        w.queue[0].gain = SongGain { track: Some(GainInfo { gain: -1.0, peak: 0.915 }), album: None };
        w.start_at(0, 0.0, true);
        let mut got = Vec::new();
        for _ in 0..10_000 {
            if !w.decoding && w.pending.is_empty() {
                break;
            }
            if matches!(w.step(), Step::EndOfSong) {
                break;
            }
            got.extend(cons.pop_iter());
        }
        got.extend(cons.pop_iter());
        let _ = std::fs::remove_dir_all(dir);
        let peak = got.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(got.len() > 10_000, "the song played");
        assert!(peak <= CEILING + 1e-4 && peak > 0.8, "peak {peak}");
    }
}