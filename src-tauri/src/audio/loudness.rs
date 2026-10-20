//! Loudness leveling (ReplayGain 2.0 / EBU R128).
//!
//! Every song is measured once (integrated loudness + sample peak). During playback, each song gets a
//! gain that brings it to the −18 LUFS reference, capped so its peak stays 1 dB below full scale.
//! Besides matching levels between songs, the reduction gives modern "hot" masters the headroom they
//! need: without it, resampling and EQ push their peaks over full scale and they clip.

use std::path::Path;

use anyhow::{Result, anyhow};
use ebur128::{EbuR128, Mode};
use serde::{Deserialize, Serialize};

use super::source::Source;

/// ReplayGain 2.0 reference loudness.
pub const REFERENCE_LUFS: f64 = -18.0;
/// Headroom applied to songs that haven't been measured yet.
pub const UNMEASURED_HEADROOM_DB: f32 = -3.0;
/// Margin kept below full scale when limiting a gain by the song's peak.
const PEAK_MARGIN_DB: f32 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GainInfo {
    /// Gain to the reference level, in dB.
    pub gain: f32,
    /// Highest sample magnitude (1.0 = full scale).
    pub peak: f32,
}

/// Measured or tagged gains for one song.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongGain {
    pub track: Option<GainInfo>,
    pub album: Option<GainInfo>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LevelingMode {
    Off,
    Track,
    Album,
    /// Album gain while an album plays in order, track gain otherwise (shuffle, mixes).
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Leveling {
    pub mode: LevelingMode,
    /// Extra gain on top of ReplayGain, in dB (ReplayGain's "preamp").
    pub preamp: f32,
}

impl Default for Leveling {
    fn default() -> Self {
        Self {
            mode: LevelingMode::Auto,
            preamp: 0.0,
        }
    }
}

impl Leveling {
    /// The gain (dB) to apply to a song. `album_context` = the song is played as part of its album.
    pub fn gain_db(&self, gain: &SongGain, album_context: bool) -> f32 {
        let pick = match self.mode {
            LevelingMode::Off => return 0.0,
            LevelingMode::Track => gain.track.or(gain.album),
            LevelingMode::Album => gain.album.or(gain.track),
            LevelingMode::Auto => {
                if album_context {
                    gain.album.or(gain.track)
                } else {
                    gain.track.or(gain.album)
                }
            }
        };
        match pick {
            Some(g) => {
                let wanted = g.gain + self.preamp;
                let ceiling = if g.peak > 0.0 {
                    -20.0 * g.peak.log10() - PEAK_MARGIN_DB
                } else {
                    wanted
                };
                wanted.min(ceiling)
            }
            None => UNMEASURED_HEADROOM_DB,
        }
    }
}

/// Parses ReplayGain tag values like "-6.52 dB" or "0.988525".
pub fn parse_tag_number(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() || c.is_whitespace())
        .trim()
        .parse()
        .ok()
}

/// Measures one song. Returns its gain info and the analyzer (kept to compute album loudness).
pub fn measure(path: &Path) -> Result<(GainInfo, EbuR128)> {
    let mut src = Source::open(path)?;
    let (rate, channels) = (src.rate, src.channels);
    let mut meter = EbuR128::new(channels as u32, rate, Mode::I | Mode::SAMPLE_PEAK)?;
    while let Some((samples, r, c)) = src.next_chunk()? {
        if r != rate || c != channels {
            // Format changed mid-stream (rare); measure what we have.
            break;
        }
        meter.add_frames_f32(samples)?;
    }
    let lufs = meter.loudness_global()?;
    if !lufs.is_finite() {
        return Err(anyhow!("silent or too short to measure"));
    }
    let peak = (0..channels as u32)
        .filter_map(|c| meter.sample_peak(c).ok())
        .fold(0.0f64, f64::max);
    Ok((
        GainInfo {
            gain: (REFERENCE_LUFS - lufs) as f32,
            peak: peak as f32,
        },
        meter,
    ))
}

/// Measures every song of an album; returns per-song track gains and the album gain.
pub fn measure_album(paths: &[&Path]) -> (Vec<Option<GainInfo>>, Option<GainInfo>) {
    let mut tracks = Vec::with_capacity(paths.len());
    let mut meters = Vec::new();
    for p in paths {
        match measure(p) {
            Ok((g, m)) => {
                tracks.push(Some(g));
                meters.push(m);
            }
            Err(e) => {
                log::warn!("loudness scan failed for {}: {e:#}", p.display());
                tracks.push(None);
            }
        }
    }
    let album = EbuR128::loudness_global_multiple(meters.iter())
        .ok()
        .filter(|l| l.is_finite())
        .map(|lufs| GainInfo {
            gain: (REFERENCE_LUFS - lufs) as f32,
            peak: tracks.iter().flatten().map(|g| g.peak).fold(0.0, f32::max),
        });
    (tracks, album)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tag_values() {
        assert_eq!(parse_tag_number("-6.52 dB"), Some(-6.52));
        assert_eq!(parse_tag_number("+1.5 dB"), Some(1.5));
        assert_eq!(parse_tag_number("0.988525"), Some(0.988525));
        assert_eq!(parse_tag_number("abc"), None);
    }

    #[test]
    fn gain_is_capped_by_peak() {
        let l = Leveling {
            mode: LevelingMode::Auto,
            ..Default::default()
        };
        // A quiet song wants +6 dB but peaks at -2 dBFS: may only rise to -1 dBFS.
        let g = SongGain {
            track: Some(GainInfo {
                gain: 6.0,
                peak: 10f32.powf(-2.0 / 20.0),
            }),
            album: None,
        };
        assert!((l.gain_db(&g, false) - 1.0).abs() < 0.01);
        // Unmeasured songs get safety headroom.
        assert_eq!(
            l.gain_db(&SongGain::default(), false),
            UNMEASURED_HEADROOM_DB
        );
        // Off really is off.
        let off = Leveling {
            mode: LevelingMode::Off,
            preamp: 0.0,
        };
        assert_eq!(off.gain_db(&g, false), 0.0);
    }
}
