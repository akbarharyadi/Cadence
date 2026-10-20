//! "Sounds like": local audio analysis with bliss (tempo, timbre, loudness and harmony), so
//! Cadence can find songs that sound alike, play a radio from any song and sort the library
//! into mixes. Everything runs on this computer; nothing is sent anywhere.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use bliss_audio::{FeaturesVersion, Song};
use serde::{Deserialize, Serialize};

use crate::audio::{Resampler, Source};
use crate::library::Track;

/// bliss analyses mono audio at this rate.
const RATE: u32 = 22_050;

/// bliss's own weighting of its features (tempo counts less, harmony shares a few dimensions).
static WEIGHTS: LazyLock<Vec<f32>> =
    LazyLock::new(|| FeaturesVersion::LATEST.feature_weights().diag().to_vec());

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    /// File size and modification time when analysed; a changed file is analysed again.
    pub stamp: String,
    /// The features, or empty if the file couldn't be analysed (not retried until it changes).
    pub f: Vec<f32>,
    /// Octave-band levels (dB) for Smart EQ; empty in files from before it existed.
    #[serde(default)]
    pub t: Vec<f32>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    pub version: u16,
    pub tracks: HashMap<String, Entry>,
}

impl Store {
    /// Drops everything if it was made by another version of the features.
    pub fn checked(mut self) -> Self {
        if self.version != FeaturesVersion::LATEST as u16 {
            self.tracks.clear();
            self.version = FeaturesVersion::LATEST as u16;
        }
        self
    }

    pub fn features(&self, t: &Track) -> Option<&[f32]> {
        self.tracks
            .get(&t.id)
            .filter(|e| e.stamp == stamp(t) && e.f.len() == WEIGHTS.len())
            .map(|e| e.f.as_slice())
    }

    pub fn needs(&self, t: &Track) -> bool {
        self.tracks
            .get(&t.id)
            .is_none_or(|e| e.stamp != stamp(t) || (e.t.is_empty() && !e.f.is_empty()))
    }

    /// The Smart EQ curve for a song, once it has been measured.
    pub fn tone(&self, t: &Track) -> Option<[f32; 10]> {
        self.tracks
            .get(&t.id)
            .filter(|e| e.stamp == stamp(t) && e.t.len() == 10)
            .map(|e| crate::audio::tone::smart_correction(&e.t))
    }
}

pub fn stamp(t: &Track) -> String {
    format!("{}:{}", t.size, t.modified)
}

/// Decodes a whole file once: bliss features (from 22.05 kHz mono) and octave-band levels for
/// Smart EQ (at the file's own rate).
pub fn analyze(path: &Path) -> Result<(Vec<f32>, Vec<f32>)> {
    let mut src = Source::open(path)?;
    let mut bands = crate::audio::tone::BandMeter::new(src.rate);
    let mut resampler: Option<Resampler> = None;
    let mut mono = Vec::new();
    let mut out = Vec::new();
    while let Some((buf, rate, ch)) = src.next_chunk()? {
        mono.clear();
        mono.extend(
            buf.chunks_exact(ch)
                .map(|f| f.iter().sum::<f32>() / ch as f32),
        );
        bands.push(&mono);
        if rate == RATE {
            out.extend_from_slice(&mono);
            continue;
        }
        if !resampler.as_ref().is_some_and(|r| r.matches(rate, RATE, 1)) {
            if let Some(r) = resampler.as_mut() {
                r.flush(&mut out)?;
            }
            resampler = Some(Resampler::new(rate, RATE, 1)?);
        }
        if let Some(r) = resampler.as_mut() {
            r.push(&mono, &mut out)?;
        }
    }
    if let Some(r) = resampler.as_mut() {
        r.flush(&mut out)?;
    }
    let analysis = Song::analyze(&out).map_err(|e| anyhow!("{e}"))?;
    Ok((analysis.as_vec(), bands.levels()))
}

pub fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .zip(WEIGHTS.iter())
        .map(|((x, y), w)| w * (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// A song with its features, as the functions below need it.
pub struct Item<'a> {
    pub track: &'a Track,
    pub f: &'a [f32],
}

/// The same recording on two albums (a single and the album, say) shouldn't fill a list twice.
fn song_key(t: &Track) -> String {
    format!("{}\u{1}{}", t.title.to_lowercase(), t.artist.to_lowercase())
}

/// Songs that sound most like `seed`, closest first.
pub fn similar<'a>(items: &[Item<'a>], seed: &Track, limit: usize) -> Vec<&'a Track> {
    let Some(sf) = items.iter().find(|i| i.track.id == seed.id).map(|i| i.f) else {
        return Vec::new();
    };
    let mut ranked: Vec<_> = items
        .iter()
        .filter(|i| i.track.id != seed.id)
        .map(|i| (distance(sf, i.f), i.track))
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut seen = HashSet::from([song_key(seed)]);
    ranked
        .into_iter()
        .filter(|(_, t)| seen.insert(song_key(t)))
        .take(limit)
        .map(|(_, t)| t)
        .collect()
}

/// A radio that starts at `seed` and drifts slowly: each next song is close to the one before
/// and still near the seed, and doesn't come from the same album twice in a row if it can help it.
pub fn radio<'a>(items: &[Item<'a>], seed: &'a Track, count: usize) -> Vec<&'a Track> {
    let Some(sf) = items.iter().find(|i| i.track.id == seed.id).map(|i| i.f) else {
        return vec![seed];
    };
    let mut out = vec![seed];
    let mut seen = HashSet::from([song_key(seed)]);
    let mut current = sf;
    while out.len() < count {
        let last_album = &out[out.len() - 1].album_id;
        let best = items
            .iter()
            .filter(|i| !seen.contains(&song_key(i.track)))
            .map(|i| {
                let mut score = 0.7 * distance(current, i.f) + 0.3 * distance(sf, i.f);
                if &i.track.album_id == last_album {
                    score *= 1.15;
                }
                (score, i)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, next)) = best else { break };
        seen.insert(song_key(next.track));
        current = next.f;
        out.push(next.track);
    }
    out
}

/// Puts songs in an order where each one flows into the next (nearest neighbour from the first).
fn flow<'a>(mut rest: Vec<&Item<'a>>) -> Vec<&'a Track> {
    let mut out = Vec::with_capacity(rest.len());
    if rest.is_empty() {
        return out;
    }
    let mut current = rest.remove(0);
    out.push(current.track);
    while !rest.is_empty() {
        let (i, _) = rest
            .iter()
            .enumerate()
            .map(|(i, x)| (i, distance(current.f, x.f)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("not empty");
        current = rest.remove(i);
        out.push(current.track);
    }
    out
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mix {
    pub name: String,
    pub description: String,
    pub track_ids: Vec<String>,
}

// Feature positions (bliss AnalysisIndex) used to describe a mix.
const TEMPO: usize = 0;
const BRIGHTNESS: usize = 2; // mean spectral centroid
const LOUDNESS: usize = 8;
const MAJOR: usize = 16;
const MINOR: usize = 17;

/// Groups the library into a few mixes of songs that sound alike (k-means on the features)
/// and names each by what stands out about it compared with the rest of the library.
pub fn mixes(items: &[Item<'_>]) -> Vec<Mix> {
    let n = items.len();
    if n < 6 {
        return Vec::new();
    }
    let k = (n / 10).clamp(2, 6);
    let dims = WEIGHTS.len();
    // Scale by √weight so plain Euclidean distance matches `distance`.
    let scale: Vec<f32> = WEIGHTS.iter().map(|w| w.sqrt()).collect();
    let points: Vec<Vec<f32>> = items
        .iter()
        .map(|i| i.f.iter().zip(&scale).map(|(x, s)| x * s).collect())
        .collect();
    let d2 = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>();

    // k-means++ style start, made deterministic: begin with the most central song, then keep
    // adding the song farthest from every centre chosen so far.
    let mean: Vec<f32> = (0..dims)
        .map(|d| points.iter().map(|p| p[d]).sum::<f32>() / n as f32)
        .collect();
    let first = (0..n)
        .min_by(|&a, &b| d2(&points[a], &mean).total_cmp(&d2(&points[b], &mean)))
        .expect("songs");
    let mut centres = vec![points[first].clone()];
    while centres.len() < k {
        let far = (0..n)
            .max_by(|&a, &b| {
                let da = centres.iter().map(|c| d2(&points[a], c)).fold(f32::MAX, f32::min);
                let db = centres.iter().map(|c| d2(&points[b], c)).fold(f32::MAX, f32::min);
                da.total_cmp(&db)
            })
            .expect("songs");
        centres.push(points[far].clone());
    }
    let mut assign = vec![0usize; n];
    for _ in 0..30 {
        let mut changed = false;
        for (i, p) in points.iter().enumerate() {
            let best = (0..k)
                .min_by(|&a, &b| d2(p, &centres[a]).total_cmp(&d2(p, &centres[b])))
                .expect("centres");
            if assign[i] != best {
                assign[i] = best;
                changed = true;
            }
        }
        for (c, centre) in centres.iter_mut().enumerate() {
            let members: Vec<&Vec<f32>> =
                points.iter().zip(&assign).filter(|(_, a)| **a == c).map(|(p, _)| p).collect();
            if !members.is_empty() {
                for (d, v) in centre.iter_mut().enumerate() {
                    *v = members.iter().map(|m| m[d]).sum::<f32>() / members.len() as f32;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // How each mix differs from the whole library, in standard deviations, on a few
    // descriptors people recognise.
    let raw = |i: usize, d: usize| items[i].f[d];
    let mood = |i: usize| raw(i, MINOR) - raw(i, MAJOR);
    let descriptors: [(&dyn Fn(usize) -> f32, &str, &str); 4] = [
        (&|i| raw(i, TEMPO), "Upbeat", "Slow"),
        (&|i| raw(i, LOUDNESS), "Full", "Quiet"),
        (&|i| raw(i, BRIGHTNESS), "Bright", "Warm"),
        (&mood, "Moody", "Uplifting"),
    ];
    let stats: Vec<(f32, f32)> = descriptors
        .iter()
        .map(|(f, _, _)| {
            let m = (0..n).map(|i| f(i)).sum::<f32>() / n as f32;
            let v = (0..n).map(|i| (f(i) - m).powi(2)).sum::<f32>() / n as f32;
            (m, v.sqrt().max(1e-6))
        })
        .collect();

    let mut used_names = HashSet::new();
    let mut out = Vec::new();
    for c in 0..k {
        let members: Vec<usize> = (0..n).filter(|&i| assign[i] == c).collect();
        if members.len() < 3 {
            continue;
        }
        let mut z: Vec<(f32, &str)> = descriptors
            .iter()
            .zip(&stats)
            .map(|((f, hi, lo), (m, sd))| {
                let avg = members.iter().map(|&i| f(i)).sum::<f32>() / members.len() as f32;
                let z = (avg - m) / sd;
                (z.abs(), if z >= 0.0 { *hi } else { *lo })
            })
            .collect();
        z.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut name = format!("{} and {}", z[0].1, z[1].1.to_lowercase());
        if !used_names.insert(name.clone()) {
            name = format!("{} and {}", z[0].1, z[2].1.to_lowercase());
            used_names.insert(name.clone());
        }
        let tempo = members.iter().map(|&i| raw(i, TEMPO)).sum::<f32>() / members.len() as f32;
        // bliss maps tempo 0–206 BPM onto −1…1.
        let bpm = ((tempo + 1.0) / 2.0 * 206.0).round();
        let description = format!("{} songs, around {bpm} BPM", members.len());

        // Start from the song nearest the centre, then let them flow.
        let mut ordered: Vec<&Item> = members.iter().map(|&i| &items[i]).collect();
        ordered.sort_by(|a, b| {
            let pa: Vec<f32> = a.f.iter().zip(&scale).map(|(x, s)| x * s).collect();
            let pb: Vec<f32> = b.f.iter().zip(&scale).map(|(x, s)| x * s).collect();
            d2(&pa, &centres[c]).total_cmp(&d2(&pb, &centres[c]))
        });
        ordered.truncate(60);
        out.push(Mix {
            name,
            description,
            track_ids: flow(ordered).into_iter().map(|t| t.id.clone()).collect(),
        });
    }
    out.sort_by(|a, b| b.track_ids.len().cmp(&a.track_ids.len()));
    out
}

/// A few plain words per song about how it sounds compared with the rest of the library
/// ("fast, bright, minor key, ~128 BPM"), for the AI DJ.
pub fn describe(items: &[Item<'_>]) -> HashMap<String, String> {
    let n = items.len().max(1) as f32;
    let stat = |d: &dyn Fn(&[f32]) -> f32| {
        let m = items.iter().map(|i| d(i.f)).sum::<f32>() / n;
        let sd = (items.iter().map(|i| (d(i.f) - m).powi(2)).sum::<f32>() / n).sqrt().max(1e-6);
        (m, sd)
    };
    let dims: [(&dyn Fn(&[f32]) -> f32, &str, &str); 4] = [
        (&|f| f[TEMPO], "fast", "slow"),
        (&|f| f[LOUDNESS], "loud", "soft"),
        (&|f| f[BRIGHTNESS], "bright", "dark"),
        (&|f| f[MINOR] - f[MAJOR], "minor key", "major key"),
    ];
    let stats: Vec<_> = dims.iter().map(|(d, _, _)| stat(*d)).collect();
    items
        .iter()
        .map(|i| {
            let mut words: Vec<String> = dims
                .iter()
                .zip(&stats)
                .filter_map(|((d, hi, lo), (m, sd))| {
                    let z = (d(i.f) - m) / sd;
                    (z.abs() >= 0.6).then(|| if z > 0.0 { *hi } else { *lo }.to_string())
                })
                .collect();
            words.push(format!("~{} BPM", ((i.f[TEMPO] + 1.0) / 2.0 * 206.0).round()));
            (i.track.id.clone(), words.join(", "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_match_features() {
        assert_eq!(WEIGHTS.len(), bliss_audio::NUMBER_FEATURES);
        let a = vec![0.0; WEIGHTS.len()];
        let mut b = a.clone();
        b[1] = 1.0; // zcr has weight 1
        assert!((distance(&a, &b) - 1.0).abs() < 1e-6);
        b[1] = 0.0;
        b[0] = 1.0; // tempo has weight 0.25
        assert!((distance(&a, &b) - 0.5).abs() < 1e-6);
    }
}
