//! Who sings which line: while a song is listened to, the separated vocals are also described
//! every 20 ms by their pitch, loudness and tone colour (MFCCs). Once the lyrics are placed on the
//! singing, each line gets the typical pitch and tone of its own stretch of singing, and the lines
//! are put into two groups. When the groups really are different voices (well apart in pitch or
//! tone, and each singing a fair share of the lines), the song has two singers.
//!
//! Saved per song next to its listening (`voice/<id>.sing`, 14 bytes per 20 ms, ~0.7 KB/s).

use std::io::{Read, Write};
use std::path::Path;

#[cfg(desktop)]
use realfft::RealFftPlanner;
use serde::{Deserialize, Serialize};

use crate::lyrics::Line;

/// Frames per second (the letter model's rate, so a frame is the same 20 ms there).
pub const FPS: f64 = 50.0;
/// The vocals are described at 16 kHz.
pub const RATE: usize = 16_000;
const HOP: usize = RATE / FPS as usize;
const WIN: usize = 1024;
const TONES: usize = 12;
const MELS: usize = 26;

/// Pitch is kept as quarter semitones above this (0: no clear pitch).
const PITCH_BASE_HZ: f32 = 55.0;

/// The vocals of a song, frame by frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Voices {
    /// Quarter semitones above 55 Hz; 0 where there's no clear pitch.
    pub pitch: Vec<u8>,
    /// Loudness, dB + 100.
    pub level: Vec<u8>,
    /// Tone colour: MFCC 1–12, halves.
    pub tone: Vec<[i8; TONES]>,
}

impl Voices {
    pub fn frames(&self) -> usize {
        self.pitch.len()
    }
    fn semitones(&self, f: usize) -> Option<f32> {
        (self.pitch[f] > 0).then(|| self.pitch[f] as f32 / 4.0)
    }
}

/// Describes mono vocals at 16 kHz.
#[cfg(desktop)]
pub fn analyze(audio: &[f32]) -> Voices {
    let frames = audio.len() / HOP;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(WIN);
    let window: Vec<f32> = (0..WIN)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WIN as f32).cos())
        .collect();
    let bank = mel_bank();
    let mut time = vec![0f32; WIN];
    let mut spec = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let mut out = Voices {
        pitch: Vec::with_capacity(frames),
        level: Vec::with_capacity(frames),
        tone: Vec::with_capacity(frames),
    };
    let mut d = vec![0f32; MAX_LAG + 1];
    for f in 0..frames {
        // Frame f is centred on sample f·HOP + HOP/2 (the letter model's frame).
        let centre = f * HOP + HOP / 2;
        let start = centre as isize - WIN as isize / 2;
        for (j, t) in time.iter_mut().enumerate() {
            let at = start + j as isize;
            *t = if (0..audio.len() as isize).contains(&at) { audio[at as usize] } else { 0.0 };
        }
        let energy = time.iter().map(|x| x * x).sum::<f32>() / WIN as f32;
        let db = 10.0 * energy.max(1e-10).log10();
        out.level.push((db + 100.0).round().clamp(0.0, 255.0) as u8);
        out.pitch.push(if db > -55.0 { yin(&time, &mut d).map_or(0, quarter_semitones) } else { 0 });
        for (j, t) in time.iter_mut().enumerate() {
            *t *= window[j];
        }
        let _ = fft.process_with_scratch(&mut time, &mut spec, &mut scratch);
        let power: Vec<f32> = spec.iter().map(|c| c.norm_sqr()).collect();
        let logmel: Vec<f32> = bank
            .iter()
            .map(|(lo, w)| (w.iter().enumerate().map(|(k, w)| w * power[lo + k]).sum::<f32>() + 1e-8).ln())
            .collect();
        let mut tone = [0i8; TONES];
        for (c, v) in tone.iter_mut().enumerate() {
            let n = (c + 1) as f32;
            let x: f32 = logmel
                .iter()
                .enumerate()
                .map(|(m, l)| l * (std::f32::consts::PI * n * (m as f32 + 0.5) / MELS as f32).cos())
                .sum::<f32>()
                * (2.0 / MELS as f32).sqrt();
            *v = (x * 2.0).round().clamp(-127.0, 127.0) as i8;
        }
        out.tone.push(tone);
    }
    out
}

#[cfg(desktop)]
fn quarter_semitones(hz: f32) -> u8 {
    (12.0 * (hz / PITCH_BASE_HZ).log2() * 4.0).round().clamp(1.0, 255.0) as u8
}

/// Singing between about 70 Hz and 1 kHz.
const MIN_LAG: usize = RATE / 1000;
const MAX_LAG: usize = RATE / 70;

/// YIN pitch (Hz) of a frame, if it has a clear one.
#[cfg(desktop)]
fn yin(x: &[f32], d: &mut [f32]) -> Option<f32> {
    let n = x.len() - MAX_LAG;
    for (lag, v) in d.iter_mut().enumerate().skip(1) {
        let mut s = 0f32;
        for i in 0..n {
            let e = x[i] - x[i + lag];
            s += e * e;
        }
        *v = s;
    }
    // Cumulative mean normalised difference.
    let mut sum = 0f32;
    d[0] = 1.0;
    for lag in 1..=MAX_LAG {
        sum += d[lag];
        d[lag] = if sum > 0.0 { d[lag] * lag as f32 / sum } else { 1.0 };
    }
    let mut lag = MIN_LAG;
    while lag < MAX_LAG {
        if d[lag] < 0.15 {
            while lag + 1 < MAX_LAG && d[lag + 1] < d[lag] {
                lag += 1;
            }
            // Parabolic interpolation around the dip.
            let (a, b, c) = (d[lag - 1], d[lag], d[lag + 1]);
            let shift = if a + c - 2.0 * b != 0.0 { 0.5 * (a - c) / (a + c - 2.0 * b) } else { 0.0 };
            return Some(RATE as f32 / (lag as f32 + shift.clamp(-1.0, 1.0)));
        }
        lag += 1;
    }
    None
}

/// Triangular mel filters from 80 Hz to 7.6 kHz: (first bin, weights).
#[cfg(desktop)]
fn mel_bank() -> Vec<(usize, Vec<f32>)> {
    let mel = |hz: f32| 2595.0 * (1.0 + hz / 700.0).log10();
    let hz = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let (lo, hi) = (mel(80.0), mel(7600.0));
    let edges: Vec<f32> = (0..MELS + 2).map(|i| hz(lo + (hi - lo) * i as f32 / (MELS + 1) as f32)).collect();
    let bin_hz = RATE as f32 / WIN as f32;
    (0..MELS)
        .map(|m| {
            let (a, b, c) = (edges[m], edges[m + 1], edges[m + 2]);
            let first = (a / bin_hz).ceil() as usize;
            let last = ((c / bin_hz).floor() as usize).min(WIN / 2);
            let w = (first..=last)
                .map(|k| {
                    let f = k as f32 * bin_hz;
                    if f <= b { (f - a) / (b - a) } else { (c - f) / (c - b) }.max(0.0)
                })
                .collect();
            (first, w)
        })
        .collect()
}

// ---------- telling the singers apart ----------

/// Who sings each line, as found in the singing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Split {
    /// Per line: the singer (0 = the lower voice, 1 = the higher, 2 = both together), or None
    /// when not sure (instrumental gaps, too little singing, or a line between both voices).
    pub lines: Vec<Option<u8>>,
    /// Each voice's typical pitch, in semitones above 55 Hz.
    pub pitch: [f32; 2],
}

/// The stretch of singing of each line (start, end in seconds).
fn spans(lines: &[Line]) -> Vec<Option<(f64, f64)>> {
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.text.trim().is_empty() {
                return None;
            }
            let next = lines.get(i + 1).map(|n| n.time);
            match (l.words.first(), l.words.last()) {
                (Some(a), Some(b)) => {
                    let end = b.end.or(next).unwrap_or(b.time + 1.0).min(b.time + 3.0);
                    Some((a.time, end.max(a.time + 0.2)))
                }
                _ => Some((l.time, next.unwrap_or(l.time + 4.0).min(l.time + 8.0))),
            }
        })
        .collect()
}

/// What a line sounds like: its median pitch and average tone colour, from its loud, pitched
/// frames. None with too little of them.
fn describe(v: &Voices, span: (f64, f64), tone_mean: &[f32; TONES], loud: u8) -> Option<(f32, [f32; TONES])> {
    let a = ((span.0 * FPS) as usize).min(v.frames());
    let b = ((span.1 * FPS).ceil() as usize).min(v.frames());
    let mut pitches = Vec::new();
    let mut tone = [0f32; TONES];
    for f in a..b {
        if v.level[f] < loud {
            continue;
        }
        let Some(p) = v.semitones(f) else { continue };
        pitches.push(p);
        for (t, x) in tone.iter_mut().zip(v.tone[f]) {
            *t += x as f32 / 2.0;
        }
    }
    if pitches.len() < 8 {
        return None;
    }
    pitches.sort_by(f32::total_cmp);
    let n = pitches.len() as f32;
    for (t, m) in tone.iter_mut().zip(tone_mean) {
        *t = *t / n - m;
    }
    Some((pitches[pitches.len() / 2], tone))
}

/// Of a line's loud, pitched frames: how many are below `low`, above `high`, and in all.
fn ranges(v: &Voices, span: (f64, f64), loud: u8, low: f32, high: f32) -> (usize, usize, usize) {
    let a = ((span.0 * FPS) as usize).min(v.frames());
    let b = ((span.1 * FPS).ceil() as usize).min(v.frames());
    let (mut l, mut h, mut n) = (0, 0, 0);
    for f in a..b {
        let Some(p) = v.semitones(f).filter(|_| v.level[f] >= loud) else { continue };
        n += 1;
        l += (p <= low) as usize;
        h += (p >= high) as usize;
    }
    (l, h, n)
}

/// Splits the lines between two voices, if the song has two clearly different ones.
pub fn split(v: &Voices, lines: &[Line]) -> Option<Split> {
    if v.frames() == 0 {
        return None;
    }
    // Loud frames: within 25 dB of the song's loud singing.
    let mut levels: Vec<u8> = v.level.iter().copied().filter(|&l| l > 30).collect();
    levels.sort_unstable();
    let top = *levels.get(levels.len() * 95 / 100)?;
    let loud = top.saturating_sub(25);
    let mut tone_mean = [0f32; TONES];
    let mut count = 0f32;
    for f in 0..v.frames() {
        if v.level[f] >= loud && v.pitch[f] > 0 {
            for (m, x) in tone_mean.iter_mut().zip(v.tone[f]) {
                *m += x as f32 / 2.0;
            }
            count += 1.0;
        }
    }
    if count < 50.0 {
        return None;
    }
    tone_mean.iter_mut().for_each(|m| *m /= count);

    let described: Vec<Option<(f32, [f32; TONES])>> =
        spans(lines).into_iter().map(|s| s.and_then(|s| describe(v, s, &tone_mean, loud))).collect();
    let known: Vec<(usize, Vec<f32>)> = described
        .iter()
        .enumerate()
        .filter_map(|(i, d)| d.as_ref().map(|(p, t)| (i, std::iter::once(*p).chain(t.iter().copied()).collect())))
        .collect();
    if known.len() < 6 {
        return None;
    }
    // Every dimension on the same scale; pitch counts as much as the 12 tone numbers together
    // would on their own, since it tells voices apart best.
    let dims = 1 + TONES;
    let mut mean = vec![0f32; dims];
    let mut sd = vec![0f32; dims];
    for (_, x) in &known {
        for d in 0..dims {
            mean[d] += x[d] / known.len() as f32;
        }
    }
    for (_, x) in &known {
        for d in 0..dims {
            sd[d] += (x[d] - mean[d]).powi(2) / known.len() as f32;
        }
    }
    let weight = |d: usize| if d == 0 { 2.0 } else { (1.0 / TONES as f32).sqrt() * 1.6 };
    let z: Vec<Vec<f32>> = known
        .iter()
        .map(|(_, x)| (0..dims).map(|d| (x[d] - mean[d]) / sd[d].sqrt().max(1e-3) * weight(d)).collect())
        .collect();
    let dist = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>();

    // Two groups (k-means), started from the lowest and the highest line.
    let lowest = (0..z.len()).min_by(|&a, &b| known[a].1[0].total_cmp(&known[b].1[0]))?;
    let highest = (0..z.len()).max_by(|&a, &b| known[a].1[0].total_cmp(&known[b].1[0]))?;
    let mut centre = [z[lowest].clone(), z[highest].clone()];
    let mut group = vec![0u8; z.len()];
    for _ in 0..30 {
        let mut changed = false;
        for (i, x) in z.iter().enumerate() {
            let g = (dist(x, &centre[1]) < dist(x, &centre[0])) as u8;
            changed |= g != group[i];
            group[i] = g;
        }
        for (g, c) in centre.iter_mut().enumerate() {
            let members: Vec<&Vec<f32>> = z.iter().zip(&group).filter(|(_, k)| **k as usize == g).map(|(x, _)| x).collect();
            if members.is_empty() {
                return None;
            }
            for d in 0..dims {
                c[d] = members.iter().map(|x| x[d]).sum::<f32>() / members.len() as f32;
            }
        }
        if !changed {
            break;
        }
    }
    // The groups' typical pitch (semitones), lower voice first.
    let median_pitch = |g: u8| {
        let mut p: Vec<f32> = known.iter().zip(&group).filter(|(_, k)| **k == g).map(|((_, x), _)| x[0]).collect();
        p.sort_by(f32::total_cmp);
        p[p.len() / 2]
    };
    let (mut p0, mut p1) = (median_pitch(0), median_pitch(1));
    if p0 > p1 {
        group.iter_mut().for_each(|g| *g = 1 - *g);
        centre.swap(0, 1);
        std::mem::swap(&mut p0, &mut p1);
    }
    let n1 = group.iter().filter(|&&g| g == 1).count();
    let n0 = group.len() - n1;
    let share = n0.min(n1) as f32 / group.len() as f32;
    // How far apart the groups are, against how spread out each one is (silhouette).
    let silhouette = z
        .iter()
        .zip(&group)
        .map(|(x, &g)| {
            let own = dist(x, &centre[g as usize]).sqrt();
            let other = dist(x, &centre[1 - g as usize]).sqrt();
            (other - own) / other.max(own).max(1e-6)
        })
        .sum::<f32>()
        / z.len() as f32;
    let gap = p1 - p0;
    log::info!(
        "singers: {} lines described, groups {n0}/{n1}, pitch {:.1}/{:.1} st (gap {gap:.1}), silhouette {silhouette:.2}",
        z.len(),
        p0,
        p1
    );
    // One voice (or one singer moving between registers): no split.
    if share < 0.15 || n0.min(n1) < 3 || gap < 3.0 || silhouette < 0.25 {
        return None;
    }
    let spans = spans(lines);
    let mut out = vec![None; lines.len()];
    for (k, (i, _)) in known.iter().enumerate() {
        let x = &z[k];
        let (d0, d1) = (dist(x, &centre[0]).sqrt(), dist(x, &centre[1]).sqrt());
        // Sung together: a good share of the line clearly in each voice's range (a third
        // each; pitch trackers sometimes jump an octave, so a little of the other doesn't count).
        if let Some(span) = spans[*i] {
            let (low, high, all) = ranges(v, span, loud, p0 + 3.0, p1 - 3.0);
            if all >= 8 && low * 10 >= all * 3 && high * 10 >= all * 3 {
                out[*i] = Some(2);
                continue;
            }
        }
        // Between both voices: not sure.
        if (d0 - d1).abs() / d0.max(d1).max(1e-6) >= 0.12 {
            out[*i] = Some(group[k]);
        }
    }
    Some(Split { lines: out, pitch: [p0, p1] })
}

// ---------- saved ----------

const MAGIC: &[u8; 4] = b"CDVS";
const FILE_VERSION: u32 = 1;

pub fn save(path: &Path, stamp: &str, v: &Voices) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = Vec::with_capacity(16 + stamp.len() + v.frames() * (2 + TONES));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FILE_VERSION.to_le_bytes());
    out.extend_from_slice(&(v.frames() as u32).to_le_bytes());
    out.extend_from_slice(&(stamp.len() as u32).to_le_bytes());
    out.extend_from_slice(stamp.as_bytes());
    for f in 0..v.frames() {
        out.push(v.pitch[f]);
        out.push(v.level[f]);
        out.extend(v.tone[f].iter().map(|&t| t as u8));
    }
    let tmp = path.with_extension("part");
    std::fs::File::create(&tmp)?.write_all(&out)?;
    std::fs::rename(&tmp, path)
}

pub fn load(path: &Path, stamp: &str) -> Option<Voices> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    let u32_at = |i: usize| Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?));
    if bytes.get(..4)? != MAGIC || u32_at(4)? != FILE_VERSION {
        return None;
    }
    let frames = u32_at(8)? as usize;
    let stamp_len = u32_at(12)? as usize;
    if bytes.get(16..16 + stamp_len)? != stamp.as_bytes() {
        return None;
    }
    let body = bytes.get(16 + stamp_len..)?;
    if body.len() != frames * (2 + TONES) {
        return None;
    }
    let mut v = Voices::default();
    for f in body.chunks_exact(2 + TONES) {
        v.pitch.push(f[0]);
        v.level.push(f[1]);
        let mut tone = [0i8; TONES];
        for (t, &b) in tone.iter_mut().zip(&f[2..]) {
            *t = b as i8;
        }
        v.tone.push(tone);
    }
    Some(v)
}

pub fn is_saved(path: &Path, stamp: &str) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = vec![0u8; 16 + stamp.len()];
    f.read_exact(&mut head).is_ok()
        && &head[..4] == MAGIC
        && head[4..8] == FILE_VERSION.to_le_bytes()
        && head[12..16] == (stamp.len() as u32).to_le_bytes()
        && &head[16..] == stamp.as_bytes()
}

#[cfg(all(test, desktop))]
mod tests {
    use super::*;

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        (0..(secs * RATE as f32) as usize)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                // A voice-like tone: a few harmonics.
                (1..5).map(|h| (2.0 * std::f32::consts::PI * hz * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.2
            })
            .collect()
    }

    #[test]
    fn finds_the_pitch() {
        for hz in [110.0, 220.0, 440.0] {
            let v = analyze(&tone(hz, 1.0));
            let mid = v.semitones(25).unwrap();
            let want = 12.0 * (hz / PITCH_BASE_HZ).log2();
            assert!((mid - want).abs() < 0.5, "{hz} Hz: {mid} st, want {want}");
        }
    }

    #[test]
    fn two_voices_are_told_apart_and_one_is_not() {
        // Lines alternating between a low voice (130 Hz) and a high one (330 Hz).
        let mut audio = Vec::new();
        let mut lines = Vec::new();
        for i in 0..12 {
            lines.push(Line { time: i as f64 * 2.0, text: format!("line {i}"), words: vec![], singers: vec![] });
            audio.extend(tone(if i % 3 == 0 { 330.0 } else { 130.0 }, 2.0));
        }
        let v = analyze(&audio);
        let s = split(&v, &lines).expect("two voices");
        for (i, g) in s.lines.iter().enumerate() {
            assert_eq!(*g, Some(if i % 3 == 0 { 1 } else { 0 }), "line {i}");
        }
        // The same voice all along: one singer.
        let v = analyze(&tone(200.0, 24.0));
        assert!(split(&v, &lines).is_none());
    }

    #[test]
    fn saved_round_trip() {
        let dir = std::env::temp_dir().join(format!("cadence-sing-test-{}", std::process::id()));
        let path = dir.join("x.sing");
        let v = analyze(&tone(220.0, 0.5));
        save(&path, "1:2", &v).unwrap();
        assert!(is_saved(&path, "1:2") && !is_saved(&path, "1:3"));
        assert_eq!(load(&path, "1:2").unwrap(), v);
        let _ = std::fs::remove_dir_all(dir);
    }
}
