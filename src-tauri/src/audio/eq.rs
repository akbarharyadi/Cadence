//! Equalizer built from RBJ "Audio EQ Cookbook" biquads, processed in f64.
//!
//! Two modes share one engine:
//! - **Graphic**: the classic 10 bands (31 Hz – 16 kHz), peaking filters at Q 1.41.
//! - **Parametric**: any list of peaking / shelf / pass filters with their own Q — e.g. a headphone
//!   correction imported from AutoEq or Equalizer APO (`ParametricEQ.txt`).
//!
//! The preamp is automatic: the combined response of all filters is measured (up to Nyquist) and the
//! level lowered by exactly the largest boost, so the EQ itself can never push audio past full scale.
//! A manual preamp comes on top of that: any cut is applied as it is; a boost only as far as the
//! combined response stays at or below 0 dB. When the EQ is off (or completely flat) it's bypassed
//! and the samples pass through untouched.

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
    /// Manual gain in dB, on top of the automatic headroom. A cut always applies; a boost only as
    /// far as the combined response (filters and preamp) stays at or below 0 dB, so it can't clip.
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
        self.active_slots().into_iter().map(|(_, f)| f).collect()
    }

    /// The filters in effect with their place in the settings (graphic band, parametric row), so
    /// a filter keeps its memory when others come and go.
    fn active_slots(&self) -> Vec<(usize, Filter)> {
        match self.mode {
            EqMode::Graphic => BANDS
                .iter()
                .zip(self.gains.iter())
                .enumerate()
                .filter(|(_, (_, g))| **g != 0.0)
                .map(|(i, (f, g))| {
                    (
                        i,
                        Filter {
                            kind: FilterKind::Peak,
                            freq: *f,
                            gain: *g,
                            q: GRAPHIC_Q,
                            enabled: true,
                        },
                    )
                })
                .collect(),
            EqMode::Parametric => self
                .filters
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, f)| f.enabled)
                .collect(),
        }
    }

    pub fn is_transparent(&self) -> bool {
        !self.enabled
            || (self.preamp == 0.0 && self.active_filters().iter().all(is_identity))
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

/// Which filter of the settings a running filter is (see [`Equalizer::rebuild`]).
#[derive(Clone, Copy)]
struct Slot {
    kind: FilterKind,
    freq: f64,
    q: f64,
    /// Place in the settings (graphic band / parametric row).
    pos: usize,
}

pub struct Equalizer {
    settings: EqSettings,
    channels: usize,
    rate: f64,
    /// Linear level before the filters: where it's going, where it is, and its step per frame
    /// (a new headroom glides over ~10 ms instead of stepping, which would click).
    preamp: f64,
    preamp_now: f64,
    preamp_step: f64,
    /// Automatic reduction (dB, ≤ 0) that keeps the combined response at or below 0 dB.
    auto_headroom: f64,
    filters: Vec<Coeffs>,
    slots: Vec<Slot>,
    /// One state per (filter, channel), for `state_channels` channels.
    states: Vec<State>,
    state_channels: usize,
}

impl Equalizer {
    pub fn new(settings: EqSettings, channels: usize, rate: u32) -> Self {
        let mut eq = Self {
            settings,
            channels,
            rate: rate as f64,
            preamp: 1.0,
            preamp_now: 1.0,
            preamp_step: 0.0,
            auto_headroom: 0.0,
            filters: Vec::new(),
            slots: Vec::new(),
            states: Vec::new(),
            state_channels: channels,
        };
        eq.rebuild();
        eq.preamp_now = eq.preamp;
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

    /// Like [`Self::set_settings`], but the level changes at once rather than gliding: for a new
    /// song, whose gain changes at the same sample.
    pub fn set_settings_now(&mut self, settings: EqSettings) {
        self.set_settings(settings);
        self.preamp_now = self.preamp;
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
        let active: Vec<(usize, Filter)> = self
            .settings
            .active_slots()
            .into_iter()
            .filter(|(_, f)| f.freq > 0.0 && f.freq < nyquist_ok && !is_identity(f))
            .collect();
        self.filters = active.iter().map(|(_, f)| Coeffs::design(f, self.rate)).collect();
        let slots: Vec<Slot> = active
            .iter()
            .map(|(pos, f)| Slot {
                kind: f.kind,
                freq: f.freq,
                q: f.q,
                pos: *pos,
            })
            .collect();
        // Measure the combined curve (log sweep 10 Hz – Nyquist: shelves reach their full gain at
        // the ends) and cancel its highest boost.
        let nyquist = self.rate / 2.0 * 0.999;
        let top = if self.filters.is_empty() {
            0.0
        } else {
            (0..=600)
                .map(|i| 10.0 * (nyquist / 10.0).powf(i as f64 / 600.0))
                .map(|f| self.filters.iter().map(|c| c.magnitude_db(f, self.rate)).sum::<f64>())
                .fold(f64::MIN, f64::max)
        };
        // The manual preamp on top; a boost only as far as the response stays at or below 0 dB.
        let total = (self.settings.preamp - top.max(0.0)).min(-top);
        self.auto_headroom = total - self.settings.preamp;
        let target = 10f64.powf(total / 20.0);
        if target != self.preamp {
            self.preamp = target;
            self.preamp_step = (target - self.preamp_now) / (self.rate * 0.01).max(1.0);
        }
        self.keep_states(slots);
    }

    /// Carries the filter memory over to the new set of filters, so the sound goes on smoothly
    /// instead of restarting from silence with a click: a filter that's still there (same kind,
    /// frequency and Q, or the same place in the settings) keeps its own; a new one starts as if it
    /// had been passing the signal through all along.
    fn keep_states(&mut self, slots: Vec<Slot>) {
        let ch = self.channels;
        let old_slots = std::mem::take(&mut self.slots);
        let old = std::mem::take(&mut self.states);
        let reuse = self.state_channels == ch && old.len() == old_slots.len() * ch;
        let mut used = vec![false; old_slots.len()];
        let mut states = Vec::with_capacity(slots.len() * ch);
        for (k, s) in slots.iter().enumerate() {
            let same = |o: &Slot| o.kind == s.kind && o.freq == s.freq && o.q == s.q;
            let j = reuse
                .then(|| {
                    (0..old_slots.len())
                        .find(|&j| !used[j] && same(&old_slots[j]))
                        .or_else(|| (0..old_slots.len()).find(|&j| !used[j] && old_slots[j].pos == s.pos && old_slots[j].kind == s.kind))
                })
                .flatten();
            if let Some(j) = j {
                used[j] = true;
            }
            for c in 0..ch {
                let st = match j {
                    Some(j) => old[j * ch + c],
                    None => {
                        // Its input so far: what the filter before it put out, or what came
                        // into the chain.
                        let (i1, i2) = if k > 0 {
                            let p: &State = &states[(k - 1) * ch + c];
                            (p.y1, p.y2)
                        } else if reuse && !old_slots.is_empty() {
                            (old[c].x1, old[c].x2)
                        } else {
                            (0.0, 0.0)
                        };
                        State { x1: i1, x2: i2, y1: i1, y2: i2 }
                    }
                };
                states.push(st);
            }
        }
        self.slots = slots;
        self.states = states;
        self.state_channels = ch;
    }

    /// Processes interleaved samples in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if self.settings.is_transparent() || self.channels == 0 {
            // Bypassed: when it comes back, it starts at its level (no glide from a stale one).
            self.preamp_now = self.preamp;
            return;
        }
        let ch = self.channels;
        for frame in samples.chunks_exact_mut(ch) {
            if self.preamp_now != self.preamp {
                self.preamp_now += self.preamp_step;
                let past = if self.preamp_step > 0.0 {
                    self.preamp_now >= self.preamp
                } else {
                    self.preamp_now <= self.preamp
                };
                if past || self.preamp_step == 0.0 {
                    self.preamp_now = self.preamp;
                }
            }
            let pre = self.preamp_now;
            for (c, sample) in frame.iter_mut().enumerate() {
                let mut x = *sample as f64 * pre;
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
        self.preamp_now = self.preamp;
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
        let on = tokens.contains(&"ON");
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
    fn headroom_counts_the_top_octave() {
        // A treble shelf reaches its full gain only near Nyquist (above 20 kHz at 44.1 kHz).
        let s = EqSettings {
            enabled: true,
            mode: EqMode::Parametric,
            filters: vec![Filter { kind: FilterKind::HighShelf, freq: 19_000.0, gain: 6.0, q: 0.707, enabled: true }],
            ..Default::default()
        };
        let eq = Equalizer::new(s, 2, 44_100);
        let c = Coeffs::design(&eq.settings().filters[0], 44_100.0);
        let at_nyquist = c.magnitude_db(22_000.0, 44_100.0);
        assert!(at_nyquist > c.magnitude_db(20_000.0, 44_100.0) + 0.3, "test needs a shelf still rising past 20 kHz");
        assert!(eq.auto_headroom_db() <= -at_nyquist + 0.05, "{} vs {at_nyquist}", eq.auto_headroom_db());
    }

    #[test]
    fn manual_preamp_cuts_but_cannot_clip() {
        let boost = |preamp| EqSettings {
            enabled: true,
            gains: [0.0, 0.0, 0.0, 0.0, 0.0, 6.0, 0.0, 0.0, 0.0, 0.0],
            preamp,
            ..Default::default()
        };
        let level = |eq: &Equalizer| 20.0 * eq.preamp.log10();
        let none = Equalizer::new(boost(0.0), 2, 48_000);
        assert!((level(&none) + 6.0).abs() < 0.05, "{}", level(&none));
        // A cut comes on top of the automatic headroom.
        let cut = Equalizer::new(boost(-3.0), 2, 48_000);
        assert!((level(&cut) + 9.0).abs() < 0.05, "{}", level(&cut));
        // A boost would push the 1 kHz peak over 0 dB: not applied.
        let up = Equalizer::new(boost(4.0), 2, 48_000);
        assert!((level(&up) + 6.0).abs() < 0.05, "{}", level(&up));
        // With only cuts in the curve, a boost is allowed as far as the curve's highest point
        // reaching 0 dB (here: next to nothing, as the curve is ~0 dB away from the cut).
        let mut s = boost(4.0);
        s.gains[5] = -3.0;
        let cut_only = Equalizer::new(s, 2, 48_000);
        assert!(level(&cut_only) > 0.0 && level(&cut_only) <= 4.0 + 1e-9, "{}", level(&cut_only));
        let top = (0..=600)
            .map(|i| 10.0 * (23_976.0f64 / 10.0).powf(i as f64 / 600.0))
            .map(|f| cut_only.filters.iter().map(|c| c.magnitude_db(f, 48_000.0)).sum::<f64>() + level(&cut_only))
            .fold(f64::MIN, f64::max);
        assert!(top <= 1e-9, "{top}");
    }

    /// Runs samples `from..from + n` of a 1 kHz sine through `eq`; returns the largest step between
    /// neighbouring output samples, counting from `last` (the previous block's last output).
    fn largest_step(eq: &mut Equalizer, from: usize, n: usize, last: &mut f32) -> f32 {
        let mut y: Vec<f32> = (from..from + n)
            .flat_map(|i| {
                let s = (0.25 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32;
                [s, s]
            })
            .collect();
        eq.process(&mut y);
        let mut step = 0.0f32;
        for s in y.iter().step_by(2) {
            step = step.max((s - *last).abs());
            *last = *s;
        }
        step
    }

    #[test]
    fn adding_and_removing_bands_keeps_the_sound_going() {
        let mut s = EqSettings {
            enabled: true,
            gains: [0.0, 0.0, 0.0, 0.0, 0.0, 9.0, 0.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let mut eq = Equalizer::new(s.clone(), 2, 48_000);
        // A 1 kHz sine at 0.25 through +9 dB at 1 kHz and -9 dB of headroom steps by at most
        // 2*pi*1000/48000*0.25 = 0.033 per sample; restarting the 1 kHz filter from silence drops
        // the level by two thirds at once. Each change lands on a crest of the sine.
        let mut last = 0.0;
        largest_step(&mut eq, 0, 48_012, &mut last);
        s.gains[2] = 2.0; // a band appears before the 1 kHz one (which moves up a place)
        eq.set_settings(s.clone());
        let step = largest_step(&mut eq, 48_012, 4_800, &mut last);
        assert!(step < 0.05, "adding a band: step {step}");
        s.gains[2] = 0.0; // and goes again
        eq.set_settings(s.clone());
        let step = largest_step(&mut eq, 52_812, 4_800, &mut last);
        assert!(step < 0.05, "removing a band: step {step}");
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
