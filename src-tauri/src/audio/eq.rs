//! Equalizer built from RBJ "Audio EQ Cookbook" biquads, processed in f64.
//!
//! Two modes share one engine:
//! - **Graphic**: the classic 10 bands (31 Hz – 16 kHz), peaking filters at Q 1.41.
//! - **Parametric**: any list of peaking / shelf / pass filters with their own Q — e.g. a headphone
//!   correction imported from AutoEq or Equalizer APO (`ParametricEQ.txt`).
//!
//! The preamp is automatic: the combined response of all filters is measured and the level lowered by
//! exactly the largest boost, so the EQ itself can never push audio past full scale. When the EQ is
//! off (or completely flat) it's bypassed and the samples pass through untouched.

use serde::{Deserialize, Serialize};

pub const BANDS: [f64; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
const GRAPHIC_Q: f64 = 1.41;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EqMode {
    #[default]
    Graphic,
    Parametric,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterKind {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub kind: FilterKind,
    pub freq: f64,
    pub gain: f64,
    pub q: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EqSettings {
    pub enabled: bool,
    pub mode: EqMode,
    /// Manual gain in dB, on top of the automatic headroom.
    pub preamp: f64,
    /// Graphic mode: gain per band in dB, matching [`BANDS`].
    pub gains: [f64; 10],
    /// Graphic mode: name of the selected preset ("Custom" once edited).
    pub preset: String,
    /// Parametric mode filters.
    pub filters: Vec<Filter>,
    /// Parametric mode: where the filters came from (e.g. "Sennheiser HD 650 (AutoEq)").
    pub profile: String,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: EqMode::Graphic,
            preamp: 0.0,
            gains: [0.0; 10],
            preset: "Flat".into(),
            filters: Vec::new(),
            profile: String::new(),
        }
    }
}

impl EqSettings {
    /// The filters currently in effect.
    pub fn active_filters(&self) -> Vec<Filter> {
        match self.mode {
            EqMode::Graphic => BANDS
                .iter()
                .zip(self.gains.iter())
                .filter(|(_, g)| **g != 0.0)
                .map(|(f, g)| Filter {
                    kind: FilterKind::Peak,
                    freq: *f,
                    gain: *g,
                    q: GRAPHIC_Q,
                    enabled: true,
                })
                .collect(),
            EqMode::Parametric => self.filters.iter().copied().filter(|f| f.enabled).collect(),
        }
    }

    pub fn is_transparent(&self) -> bool {
        !self.enabled
            || (self.preamp == 0.0 && self.active_filters().iter().all(|f| is_identity(f)))
    }
}

fn is_identity(f: &Filter) -> bool {
    matches!(
        f.kind,
        FilterKind::Peak | FilterKind::LowShelf | FilterKind::HighShelf
    ) && f.gain == 0.0
}

#[derive(Clone, Copy, Default, Debug)]
struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    /// RBJ cookbook formulas.
    fn design(f: &Filter, rate: f64) -> Self {
        let a = 10f64.powf(f.gain / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * f.freq / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * f.q.max(0.01));
        let (b0, b1, b2, a0, a1, a2) = match f.kind {
            FilterKind::Peak => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            FilterKind::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
            FilterKind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Magnitude response in dB at `freq`.
    fn magnitude_db(&self, freq: f64, rate: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / rate;
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let (nr, ni) = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            self.b1 * s1 + self.b2 * s2,
        );
        let (dr, di) = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            self.a1 * s1 + self.a2 * s2,
        );
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
    }
}

#[derive(Clone, Copy, Default)]
struct State {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

pub struct Equalizer {
    settings: EqSettings,
    channels: usize,
    rate: f64,
    preamp: f64,
    /// Automatic reduction (dB, ≤ 0) that keeps the combined response at or below 0 dB.
    auto_headroom: f64,
    filters: Vec<Coeffs>,
    /// One state per (filter, channel).
    states: Vec<State>,
}

impl Equalizer {
    pub fn new(settings: EqSettings, channels: usize, rate: u32) -> Self {
        let mut eq = Self {
            settings,
            channels,
            rate: rate as f64,
            preamp: 1.0,
            auto_headroom: 0.0,
            filters: Vec::new(),
            states: Vec::new(),
        };
        eq.rebuild();
        eq
    }

    pub fn settings(&self) -> &EqSettings {
        &self.settings
    }

    /// The automatic level reduction currently applied (dB, ≤ 0).
    pub fn auto_headroom_db(&self) -> f64 {
        if self.settings.is_transparent() {
            0.0
        } else {
            self.auto_headroom
        }
    }

    pub fn set_settings(&mut self, settings: EqSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.rebuild();
        }
    }

    /// Re-targets the filters for a new stream format (keeps the settings).
    pub fn set_format(&mut self, channels: usize, rate: u32) {
        if channels != self.channels || rate as f64 != self.rate {
            self.channels = channels;
            self.rate = rate as f64;
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let nyquist_ok = self.rate * 0.45;
        self.filters = self
            .settings
            .active_filters()
            .iter()
            .filter(|f| f.freq > 0.0 && f.freq < nyquist_ok && !is_identity(f))
            .map(|f| Coeffs::design(f, self.rate))
            .collect();
        // Measure the combined curve (log sweep 20 Hz – 20 kHz) and cancel its highest boost.
        let top = (0..=480)
            .map(|i| 20.0 * 1000f64.powf(i as f64 / 480.0))
            .filter(|f| *f < self.rate / 2.0)
            .map(|f| {
                self.filters
                    .iter()
                    .map(|c| c.magnitude_db(f, self.rate))
                    .sum::<f64>()
                    + self.settings.preamp
            })
            .fold(f64::MIN, f64::max);
        self.auto_headroom = if top > 0.0 { -top } else { 0.0 };
        self.preamp = 10f64.powf((self.settings.preamp + self.auto_headroom) / 20.0);
        // Keep the filter memory when only the curve changes (e.g. the next song's tone), so the
        // sound carries on smoothly instead of restarting from silence with a click.
        let n = self.filters.len() * self.channels;
        if self.states.len() != n {
            self.states = vec![State::default(); n];
        }
    }

    /// Processes interleaved samples in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if self.settings.is_transparent() || self.channels == 0 {
            return;
        }
        let ch = self.channels;
        for frame in samples.chunks_exact_mut(ch) {
            for (c, sample) in frame.iter_mut().enumerate() {
                let mut x = *sample as f64 * self.preamp;
                for (b, f) in self.filters.iter().enumerate() {
                    let s = &mut self.states[b * ch + c];
                    let y = f.b0 * x + f.b1 * s.x1 + f.b2 * s.x2 - f.a1 * s.y1 - f.a2 * s.y2;
                    s.x2 = s.x1;
                    s.x1 = x;
                    s.y2 = s.y1;
                    s.y1 = y;
                    x = y;
                }
                *sample = x as f32;
            }
        }
    }

    /// Clears filter memory (after a seek or a new track that doesn't follow on).
    pub fn reset(&mut self) {
        self.states.iter_mut().for_each(|s| *s = State::default());
    }
}

/// Parses an Equalizer APO / AutoEq `ParametricEQ.txt`:
/// ```text
/// Preamp: -6.1 dB
/// Filter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.70
/// Filter 2: ON PK Fc 205 Hz Gain -3.6 dB Q 0.43
/// ```
/// Returns (preamp dB, filters). Unknown lines are ignored.
pub fn parse_parametric(text: &str) -> Result<(f64, Vec<Filter>), String> {
    let mut preamp = 0.0;
    let mut filters = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Preamp:") {
            preamp = number_before(rest, "dB").ok_or("Bad preamp line")?;
            continue;
        }
        if !line.starts_with("Filter") {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let on = tokens.iter().any(|t| *t == "ON");
        let kind = tokens.iter().find_map(|t| match *t {
            "PK" | "PEQ" => Some(FilterKind::Peak),
            "LSC" | "LS" | "LSQ" => Some(FilterKind::LowShelf),
            "HSC" | "HS" | "HSQ" => Some(FilterKind::HighShelf),
            "LP" | "LPQ" => Some(FilterKind::LowPass),
            "HP" | "HPQ" => Some(FilterKind::HighPass),
            _ => None,
        });
        let value_after = |key: &str| {
            tokens
                .iter()
                .position(|t| *t == key)
                .and_then(|i| tokens.get(i + 1))
                .and_then(|v| v.parse::<f64>().ok())
        };
        let (Some(kind), Some(freq)) = (kind, value_after("Fc")) else {
            continue;
        };
        filters.push(Filter {
            kind,
            freq,
            gain: value_after("Gain").unwrap_or(0.0),
            q: value_after("Q").unwrap_or(match kind {
                FilterKind::LowShelf
                | FilterKind::HighShelf
                | FilterKind::LowPass
                | FilterKind::HighPass => 0.707,
                FilterKind::Peak => 1.0,
            }),
            enabled: on,
        });
    }
    if filters.is_empty() {
        return Err(
            "No filters found. Expected lines like “Filter 1: ON PK Fc 100 Hz Gain -3 dB Q 1.0”."
                .into(),
        );
    }
    Ok((preamp, filters))
}

fn number_before(s: &str, unit: &str) -> Option<f64> {
    s.replace(unit, "").trim().parse().ok()
}

/// Built-in graphic presets: (name, band gains dB). The automatic preamp handles headroom.
pub fn presets() -> Vec<(&'static str, [f64; 10])> {
    vec![
        ("Flat", [0.0; 10]),
        (
            "Bass Booster",
            [5.0, 4.5, 3.5, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "Bass Reducer",
            [-5.0, -4.0, -3.0, -2.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "Treble Booster",
            [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.5, 3.5, 4.5, 5.0],
        ),
        (
            "Treble Reducer",
            [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.5, -3.5, -4.5, -5.0],
        ),
        (
            "Vocal Booster",
            [-1.5, -2.0, -1.0, 1.0, 3.0, 3.5, 3.0, 1.5, 0.0, -1.0],
        ),
        (
            "Loudness",
            [6.0, 4.0, 0.0, 0.0, -2.0, 0.0, -1.0, -3.0, 5.0, 1.0],
        ),
        (
            "Acoustic",
            [4.0, 4.0, 3.0, 1.0, 2.0, 1.5, 3.0, 3.5, 3.0, 2.0],
        ),
        (
            "Classical",
            [4.0, 3.0, 2.5, 2.0, -1.5, -1.5, 0.0, 2.0, 3.0, 4.0],
        ),
        (
            "Electronic",
            [4.0, 3.5, 1.0, 0.0, -2.0, 2.0, 1.0, 1.5, 3.5, 4.0],
        ),
        (
            "Hip-Hop",
            [4.5, 4.0, 1.5, 3.0, -1.0, -1.0, 1.5, -0.5, 2.0, 3.0],
        ),
        ("Jazz", [3.5, 2.5, 1.0, 2.0, -1.5, -1.5, 0.0, 1.5, 3.0, 3.5]),
        (
            "Pop",
            [-1.5, -1.0, 0.0, 2.0, 4.0, 4.0, 2.0, 0.0, -1.0, -1.5],
        ),
        ("Rock", [4.5, 4.0, 3.0, 1.5, -0.5, -1.0, 0.5, 2.5, 3.5, 4.5]),
        (
            "Small Speakers",
            [5.5, 4.5, 4.0, 2.5, 1.0, 0.0, -1.5, -2.5, -3.5, -4.0],
        ),
        (
            "Late Night",
            [-3.0, -2.0, -1.0, 0.0, 1.5, 2.0, 1.5, 0.5, -1.0, -2.0],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_headroom_cancels_overlapping_boosts() {
        let s = EqSettings {
            enabled: true,
            gains: [4.0, 4.0, 3.0, 1.0, 2.0, 1.5, 3.0, 3.5, 3.0, 2.0],
            ..Default::default()
        };
        let eq = Equalizer::new(s, 2, 48_000);
        // Overlapping bands sum to more than the largest single band (4 dB).
        assert!(eq.auto_headroom_db() < -4.0, "{}", eq.auto_headroom_db());
    }

    #[test]
    fn flat_is_bypassed() {
        let s = EqSettings {
            enabled: true,
            ..Default::default()
        };
        let mut eq = Equalizer::new(s, 2, 48_000);
        let mut buf = vec![0.5f32, -0.25, 0.125, 1.0];
        eq.process(&mut buf);
        assert_eq!(buf, vec![0.5, -0.25, 0.125, 1.0]);
        assert_eq!(eq.auto_headroom_db(), 0.0);
    }

    #[test]
    fn parses_autoeq_file() {
        let text = "Preamp: -6.1 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.70\nFilter 2: ON PK Fc 205 Hz Gain -3.6 dB Q 0.43\nFilter 3: OFF HSC Fc 10000 Hz Gain 2 dB Q 0.7\n";
        let (pre, f) = parse_parametric(text).unwrap();
        assert_eq!(pre, -6.1);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].kind, FilterKind::LowShelf);
        assert_eq!(f[1].gain, -3.6);
        assert!(!f[2].enabled);
    }

    #[test]
    fn shelf_reaches_its_gain() {
        let c = Coeffs::design(
            &Filter {
                kind: FilterKind::LowShelf,
                freq: 100.0,
                gain: 6.0,
                q: 0.707,
                enabled: true,
            },
            48_000.0,
        );
        assert!((c.magnitude_db(20.0, 48_000.0) - 6.0).abs() < 0.3);
        assert!(c.magnitude_db(5000.0, 48_000.0).abs() < 0.1);
    }
}
