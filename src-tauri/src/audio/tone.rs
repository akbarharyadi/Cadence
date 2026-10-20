//! Per-song tone, applied before the listener's own EQ:
//!
//! - **Loudness compensation.** Sound Check turns loud songs down (often 8–12 dB). Our ears hear
//!   less deep bass and top air at lower levels (equal-loudness contours, ISO 226). This adds a
//!   little of both back, but only below ~100 Hz and above ~10 kHz: boosting closer to the voice
//!   range made vocals sound pushed back ("flat"), so the voice range is left untouched.
//! - **Smart EQ.** Each song's tonal balance is measured once (octave bands, during sound analysis)
//!   and gently nudged toward the balance of typical well-mastered music: half the difference,
//!   at most ±4 dB, as a smooth curve. Dull songs get a little air, boomy ones a little less bass.

use serde::{Deserialize, Serialize};

use super::eq::{BANDS, EqMode, EqSettings, Filter, FilterKind};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToneSettings {
    pub loudness: bool,
    pub smart: bool,
}

impl Default for ToneSettings {
    fn default() -> Self {
        Self {
            loudness: true,
            smart: true,
        }
    }
}

/// Octave-band level of typical modern commercial music, in dB relative to the 1 kHz band
/// (long-term average, energy per octave). Music falls off faster than pink noise above ~1 kHz.
const TARGET: [f32; 10] = [-4.0, 1.0, 2.0, 1.5, 1.0, 0.0, -2.0, -4.0, -7.0, -13.0];

/// The gentle correction (dB per band) that moves a song's measured balance toward `TARGET`.
/// `levels` are octave-band levels in dB (any reference); NaN bands (above Nyquist) are left alone.
pub fn smart_correction(levels: &[f32]) -> [f32; 10] {
    let mut out = [0.0f32; 10];
    if levels.len() != 10 {
        return out;
    }
    // Compare shapes, not levels: line both up on the 250 Hz – 4 kHz bands.
    let mids = 3..=7;
    let m: f32 = mids.clone().map(|i| levels[i]).sum::<f32>() / 5.0;
    let t: f32 = mids.map(|i| TARGET[i]).sum::<f32>() / 5.0;
    let raw: Vec<f32> = (0..10)
        .map(|i| {
            if levels[i].is_finite() {
                (0.5 * ((TARGET[i] - t) - (levels[i] - m))).clamp(-4.0, 4.0)
            } else {
                0.0
            }
        })
        .collect();
    // Smooth across neighbours so the curve stays broad and musical.
    for i in 0..10usize {
        let a = raw[i.saturating_sub(1)];
        let c = raw[i];
        let b = raw[(i + 1).min(9)];
        out[i] = 0.25 * a + 0.5 * c + 0.25 * b;
    }
    // Keep the overall level: centre the curve on the mid bands.
    let centre = out[3..=7].iter().sum::<f32>() / 5.0;
    out.iter_mut().for_each(|g| *g -= centre);
    out
}

/// The bass/treble shelves (dB) that make up for turning a song down by `attenuation` dB.
pub fn loudness_shelves(attenuation: f32) -> (f32, f32) {
    let a = attenuation.max(0.0);
    ((a * 0.2).min(3.0), (a * 0.1).min(1.5))
}

/// Where the loudness shelves sit: low enough and high enough to stay clear of the voice.
const BASS_SHELF_HZ: f64 = 60.0;
const TREBLE_SHELF_HZ: f64 = 12_000.0;

/// The EQ for one song. `gain_db` is Sound Check's gain for it (None = off).
pub fn settings_for(t: &ToneSettings, gain_db: Option<f32>, smart: Option<&[f32; 10]>) -> EqSettings {
    let mut filters = Vec::new();
    if t.loudness {
        let (bass, treble) = loudness_shelves(-gain_db.unwrap_or(0.0));
        if bass > 0.05 {
            filters.push(shelf(FilterKind::LowShelf, BASS_SHELF_HZ, bass));
            filters.push(shelf(FilterKind::HighShelf, TREBLE_SHELF_HZ, treble));
        }
    }
    if t.smart {
        if let Some(g) = smart {
            filters.extend(BANDS.iter().zip(g).filter(|(_, g)| g.abs() >= 0.1).map(|(f, g)| Filter {
                kind: FilterKind::Peak,
                freq: *f,
                gain: *g as f64,
                q: 1.0,
                enabled: true,
            }));
        }
    }
    EqSettings {
        enabled: !filters.is_empty(),
        mode: EqMode::Parametric,
        filters,
        ..Default::default()
    }
}

fn shelf(kind: FilterKind, freq: f64, gain: f32) -> Filter {
    Filter {
        kind,
        freq,
        gain: gain as f64,
        q: 0.7,
        enabled: true,
    }
}

/// Measures octave-band energy (the `BANDS` centres) of a mono signal with band-pass filters.
pub struct BandMeter {
    rate: f64,
    filters: Vec<[f64; 5]>,
    state: Vec<[f64; 4]>,
    energy: [f64; 10],
}

impl BandMeter {
    pub fn new(rate: u32) -> Self {
        let rate = rate as f64;
        let filters = BANDS
            .iter()
            .map(|&f| {
                // RBJ band-pass, 0 dB peak gain, one octave wide.
                let w = 2.0 * std::f64::consts::PI * f.min(rate * 0.45) / rate;
                let alpha = w.sin() / (2.0 * std::f64::consts::SQRT_2);
                let a0 = 1.0 + alpha;
                [alpha / a0, 0.0, -alpha / a0, -2.0 * w.cos() / a0, (1.0 - alpha) / a0]
            })
            .collect();
        Self {
            rate,
            filters,
            state: vec![[0.0; 4]; 10],
            energy: [0.0; 10],
        }
    }

    pub fn push(&mut self, mono: &[f32]) {
        for (b, f) in self.filters.iter().enumerate() {
            let [x1, x2, y1, y2] = &mut self.state[b];
            let mut e = 0.0;
            for &x in mono {
                let x = x as f64;
                let y = f[0] * x + f[1] * *x1 + f[2] * *x2 - f[3] * *y1 - f[4] * *y2;
                *x2 = *x1;
                *x1 = x;
                *y2 = *y1;
                *y1 = y;
                e += y * y;
            }
            self.energy[b] += e;
        }
    }

    /// Band levels in dB (NaN for bands the sample rate can't hold).
    pub fn levels(&self) -> Vec<f32> {
        BANDS
            .iter()
            .zip(self.energy)
            .map(|(f, e)| {
                if *f > self.rate * 0.42 || e <= 0.0 {
                    f32::NAN
                } else {
                    (10.0 * e.log10()) as f32
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_shaped_song_needs_nothing() {
        let c = smart_correction(&TARGET.map(|t| t + 37.0));
        assert!(c.iter().all(|g| g.abs() < 1e-4), "{c:?}");
    }

    #[test]
    fn dull_song_gets_treble() {
        let mut dull = TARGET;
        dull[8] -= 6.0;
        dull[9] -= 8.0;
        let c = smart_correction(&dull);
        assert!(c[9] > 1.5 && c[8] > 1.5, "{c:?}");
        assert!(c[5].abs() < 0.8, "{c:?}");
    }

    #[test]
    fn shelves_scale_with_attenuation() {
        assert_eq!(loudness_shelves(0.0), (0.0, 0.0));
        assert_eq!(loudness_shelves(-3.0), (0.0, 0.0)); // turned up: nothing
        let (b, t) = loudness_shelves(10.0);
        assert!((b - 2.0).abs() < 1e-6 && (t - 1.0).abs() < 1e-6);
        assert_eq!(loudness_shelves(40.0), (3.0, 1.5));
    }

    /// Level (dB) of a sine at `freq` after the EQ, measured after it settles.
    fn level_through(eq: &mut crate::audio::eq::Equalizer, freq: f64) -> f64 {
        let rate = 48_000.0;
        eq.reset();
        let mut x: Vec<f32> = (0..96_000)
            .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin() as f32 * 0.25)
            .collect();
        eq.process(&mut x);
        let tail = &x[48_000..];
        let rms = (tail.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        20.0 * (rms / (0.25 / std::f64::consts::SQRT_2)).log10()
    }

    #[test]
    fn loudness_leaves_the_voice_alone() {
        // A song turned down 12 dB: the strongest case for these songs.
        let t = ToneSettings { loudness: true, smart: false };
        let mut eq = crate::audio::eq::Equalizer::new(settings_for(&t, Some(-12.0), None), 1, 48_000);
        let mid = level_through(&mut eq, 1_000.0);
        for f in [200.0, 500.0, 2_000.0, 4_000.0, 6_000.0] {
            let d = level_through(&mut eq, f) - mid;
            assert!(d.abs() < 0.5, "{f} Hz moved {d:.2} dB relative to 1 kHz");
        }
        assert!(level_through(&mut eq, 35.0) - mid > 1.5, "deep bass should rise");
    }
}
