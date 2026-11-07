//! Library health: checks on the music files themselves.
//!
//! - Duplicates: the same song twice (same album), or a lossy copy of a song that's also lossless.
//!   Metadata only, instant.
//! - Quality: does a lossless file have lossless content? Finds files made from a lossy source
//!   (the spectrum stops at the encoder's low-pass, 16 kHz for a typical MP3), CD-rate songs saved
//!   as hi-res (nothing above 22 kHz in a 96 kHz file) and 24-bit files that are really 16-bit.
//! - Integrity: decodes the whole file and checks it against the checksum inside it (FLAC).
//!
//! The two file checks run in the background on the low-priority threads and are kept in
//! `health.json` per song (until the file changes), so each file is read once.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::Mutex;
use rayon::prelude::*;
use realfft::RealFftPlanner;
use serde::{Deserialize, Serialize};

use crate::audio::Source;
use crate::ctx::Ctx;
use crate::jobs::{background_pool, settling};
use crate::library::{Track, short_hash};
use crate::state::match_key;

/// Audio read for the spectrum, from the middle of the song.
const ANALYSIS_SECS: f64 = 20.0;
/// A fall of this many dB within a kilohertz is an encoder's low-pass, not a gentle roll-off.
const CLIFF_DB: f32 = 15.0;
/// Songs of the same length, to within this many seconds, are the same recording.
const SAME_LENGTH: f64 = 2.5;

pub fn is_lossless(codec: &str) -> bool {
    let c = codec.to_uppercase();
    ["FLAC", "ALAC", "WAV", "AIFF", "PCM"].iter().any(|l| c.starts_with(l))
}

// ---------- quality ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Music up to the top of the band (or an ordinary CD-style cut-off).
    Good,
    /// Stops a little early: maybe a high-bitrate lossy source, maybe just an old recording.
    Suspect,
    /// Stops where lossy encoders cut: it was probably made from an MP3 or AAC.
    Lossy,
    /// A hi-res file with nothing above 22 kHz: made from CD-quality audio.
    Upsampled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    /// Where the music stops, in Hz (None: it goes to the top of the band).
    pub cutoff_hz: Option<f32>,
    pub verdict: Verdict,
    /// The real bit depth, when the file says more than it holds (24-bit that is 16-bit).
    pub real_bits: Option<u8>,
}

impl Quality {
    pub fn is_suspect(&self) -> bool {
        self.verdict != Verdict::Good || self.real_bits.is_some()
    }
}

/// What the spectrum's ceiling says about a file at this sample rate.
pub fn verdict(rate: u32, cutoff_hz: Option<f32>) -> Verdict {
    let Some(c) = cutoff_hz else { return Verdict::Good };
    if rate <= 50_000 {
        if c < 17_500.0 {
            Verdict::Lossy
        } else if c < 19_500.0 {
            Verdict::Suspect
        } else {
            Verdict::Good
        }
    } else if c < 19_500.0 {
        Verdict::Lossy
    } else if c < 23_500.0 {
        Verdict::Upsampled
    } else {
        Verdict::Good
    }
}

/// Where the average spectrum of `mono` falls off a cliff, in Hz. The outer `None`: not enough
/// music to tell (too quiet or too short); the inner `None`: no cliff, music to the top.
///
/// Levels are averaged over many windows, in 250 Hz bands. An encoder's low-pass drops the level by
/// tens of dB within a few hundred Hz and leaves a flat floor above it; a recording's own roll-off
/// is gradual. Above the cliff the level must stay down.
pub fn spectrum_cutoff(mono: &[f32], rate: u32) -> Option<Option<f32>> {
    let (bands, band_hz) = band_levels(mono, rate)?;
    Some(find_cliff(&bands, band_hz))
}

/// The average level (dB below full scale) of 250 Hz bands across the spectrum, and the width of
/// a band in Hz. None when there isn't enough music to average.
fn band_levels(mono: &[f32], rate: u32) -> Option<(Vec<f32>, f32)> {
    let n: usize = if rate > 50_000 { 8192 } else { 4096 };
    let bins = n / 2 + 1;
    let window: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect();
    let window_sum: f32 = window.iter().sum();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let mut power = vec![0f64; bins];
    let mut frames = 0u32;
    let mut start = 0;
    while start + n <= mono.len() {
        let chunk = &mono[start..start + n];
        start += n / 2;
        // Quiet passages (intros, fades) would drag the average down.
        let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt();
        if rms < 0.003 {
            continue;
        }
        for i in 0..n {
            input[i] = chunk[i] * window[i];
        }
        fft.process_with_scratch(&mut input, &mut output, &mut scratch).ok()?;
        for (p, c) in power.iter_mut().zip(&output) {
            let amp = 2.0 * c.norm() / window_sum;
            *p += f64::from(amp * amp);
        }
        frames += 1;
    }
    if frames < 20 {
        return None;
    }
    // 250 Hz bands, in dB below full scale.
    let hz_per_bin = rate as f32 / n as f32;
    let band_bins = (250.0 / hz_per_bin).round().max(1.0) as usize;
    let bands: Vec<f32> = power
        .chunks(band_bins)
        .filter(|c| c.len() == band_bins)
        .map(|c| (10.0 * (c.iter().sum::<f64>() / (c.len() as f64 * f64::from(frames)) + 1e-20).log10()) as f32)
        .collect();
    Some((bands, band_bins as f32 * hz_per_bin))
}

fn find_cliff(bands: &[f32], band_hz: f32) -> Option<f32> {
    let per_khz = (1000.0 / band_hz).round().max(1.0) as usize;
    let first = (10_000.0 / band_hz) as usize;
    let mean = |s: &[f32]| s.iter().sum::<f32>() / s.len() as f32;
    // The biggest fall from one kilohertz to the next, with a kilohertz of flat floor after it.
    let mut best: Option<(usize, f32)> = None;
    for i in first.max(per_khz)..bands.len().saturating_sub(per_khz * 2) {
        let before = mean(&bands[i - per_khz..i]);
        let after = mean(&bands[i..i + per_khz]);
        let drop = before - after;
        if drop < CLIFF_DB || before < -112.0 {
            continue;
        }
        // Above the cliff the level never comes back (a dip or a notch isn't a low-pass). It needn't
        // be flat: decoded lossy audio keeps sloping down from the window's leakage.
        let tail_top = bands[i + per_khz..].iter().copied().fold(f32::MIN, f32::max);
        if tail_top <= before - CLIFF_DB && best.is_none_or(|(_, d)| drop > d) {
            best = Some((i, drop));
        }
    }
    let (i, _) = best?;
    // The cut-off is where the level first falls well below what it was.
    let before = mean(&bands[i - per_khz..i]);
    let at = (i - per_khz..i + per_khz).find(|&j| bands[j] < before - 12.0).unwrap_or(i);
    Some(at as f32 * band_hz)
}

/// How many low bits are zero in every sample of a 24-bit file (the file is really 24 − this bits).
pub fn padded_bits(samples: &[f32], bits: u32) -> u32 {
    let scale = (1u64 << (bits - 1)) as f32;
    let mut all = 0i64;
    for &s in samples {
        all |= (s * scale).round() as i64;
        if all & 1 != 0 {
            return 0;
        }
    }
    if all == 0 { 0 } else { all.trailing_zeros().min(bits - 1) }
}

/// Reads 20 seconds from the middle of the song and judges it.
pub fn check_quality(path: &Path, duration: f64) -> Result<Quality> {
    let audio = read_for_analysis(path, duration)?;
    let (rate, bits) = (audio.rate, audio.bits);
    let cutoff = spectrum_cutoff(&audio.mono, rate).flatten();
    let real_bits = match bits {
        Some(24) => {
            let zeros = padded_bits(&audio.samples, 24);
            (zeros >= 8).then_some((24 - zeros) as u8)
        }
        _ => None,
    };
    Ok(Quality { cutoff_hz: cutoff, verdict: verdict(rate, cutoff), real_bits })
}

struct Audio {
    mono: Vec<f32>,
    /// Every sample as decoded (only kept for 24-bit files, to look for padding).
    samples: Vec<f32>,
    rate: u32,
    bits: Option<u32>,
}

fn read_for_analysis(path: &Path, duration: f64) -> Result<Audio> {
    let mut src = Source::open(path)?;
    let rate = src.rate;
    let bits = src.bits;
    if duration > ANALYSIS_SECS * 3.0 {
        // Not fatal: some files can't seek, and then the start of the song is read instead.
        let _ = src.seek(duration * 0.4);
    }
    let want = (f64::from(rate) * ANALYSIS_SECS) as usize;
    let mut mono: Vec<f32> = Vec::with_capacity(want);
    let mut all_samples: Vec<f32> = Vec::new();
    while mono.len() < want {
        let Some((buf, _, ch)) = src.next_chunk()? else { break };
        let ch = ch.max(1);
        for frame in buf.chunks_exact(ch) {
            mono.push(frame.iter().sum::<f32>() / ch as f32);
        }
        if bits == Some(24) {
            all_samples.extend_from_slice(buf);
        }
    }
    Ok(Audio { mono, samples: all_samples, rate, bits })
}

// ---------- integrity ----------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Integrity {
    /// The audio matched the checksum stored in the file (None: the format has none).
    pub verified: Option<bool>,
    /// Packets that couldn't be decoded.
    pub errors: u32,
    /// Shorter than the file's header says.
    pub truncated: bool,
    /// The file couldn't be opened or read to the end.
    pub failed: Option<String>,
}

impl Integrity {
    pub fn is_bad(&self) -> bool {
        self.verified == Some(false) || self.errors > 0 || self.truncated || self.failed.is_some()
    }
}

/// Decodes the whole file, verifying it against its checksum where it has one.
pub fn check_integrity(path: &Path) -> Integrity {
    let mut src = match Source::open_verified(path) {
        Ok(s) => s,
        Err(e) => return Integrity { failed: Some(format!("{e:#}")), ..Default::default() },
    };
    let mut frames = 0u64;
    let failed = loop {
        match src.next_chunk() {
            Ok(Some((buf, _, ch))) => frames += (buf.len() / ch.max(1)) as u64,
            Ok(None) => break None,
            Err(e) => break Some(format!("{e:#}")),
        }
    };
    let verified = if failed.is_none() { src.finalize() } else { None };
    // Lossy formats pad and trim at the ends, so only lossless files are held to their length.
    let truncated = is_lossless(&src.codec) && src.total_frames.is_some_and(|t| frames + 2 < t);
    Integrity { verified, errors: src.errors, truncated, failed }
}

// ---------- results kept between runs ----------

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Entry {
    /// Size and time of the file when it was checked: a changed file is checked again.
    stamp: String,
    quality: Option<Quality>,
    integrity: Option<Integrity>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    tracks: HashMap<String, Entry>,
}

/// The saved results, and whether a check is running.
#[derive(Default)]
pub struct Health {
    pub store: Mutex<Store>,
    running: Mutex<Option<Kind>>,
    stop: AtomicBool,
}

impl Health {
    pub fn new(store: Store) -> Self {
        Self { store: Mutex::new(store), ..Default::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Quality,
    Integrity,
}

fn stamp(t: &Track) -> String {
    format!("{}:{}", t.size, t.modified)
}

/// What a song's results are kept under: its id, or for the songs of a single-file album (CUE
/// sheet), who share one file, a key for that file (it is checked once).
fn key(t: &Track) -> String {
    if t.cue.is_some() { short_hash(&t.path) } else { t.id.clone() }
}

impl Store {
    fn needs(&self, t: &Track, kind: Kind) -> bool {
        match self.tracks.get(&key(t)).filter(|e| e.stamp == stamp(t)) {
            None => true,
            Some(e) => match kind {
                Kind::Quality => e.quality.is_none(),
                Kind::Integrity => e.integrity.is_none(),
            },
        }
    }

    fn entry(&mut self, t_id: &str, stamp: &str) -> &mut Entry {
        let e = self.tracks.entry(t_id.to_string()).or_default();
        if e.stamp != stamp {
            *e = Entry { stamp: stamp.to_string(), ..Default::default() };
        }
        e
    }
}

/// Starts a background check of every song that hasn't had it (or whose file changed). Returns
/// false when one is already running.
pub fn start(ctx: Ctx, kind: Kind) -> bool {
    let state = ctx.state.clone();
    {
        let mut running = state.health.running.lock();
        if running.is_some() {
            return false;
        }
        *running = Some(kind);
    }
    state.health.stop.store(false, Ordering::SeqCst);
    let todo: Vec<(String, String, String, f64)> = {
        let lib = state.library.lock();
        let store = state.health.store.lock();
        let mut seen = std::collections::HashSet::new();
        lib.snapshot
            .tracks
            .iter()
            .filter(|t| (kind == Kind::Integrity || is_lossless(&t.codec)) && !settling(t) && store.needs(t, kind))
            // One check per file (the songs of a single-file album share theirs).
            .filter(|t| seen.insert(key(t)))
            .map(|t| (key(t), t.path.clone(), stamp(t), t.duration))
            .collect()
    };
    std::thread::spawn(move || {
        let total = todo.len();
        let progress = |done: usize, running: bool| ctx.emit("health-progress", HealthProgress { kind, done, total, running });
        progress(0, true);
        let started = Instant::now();
        let (tx, rx) = crossbeam_channel::unbounded();
        let stop_state = state.clone();
        background_pool().spawn(move || {
            todo.into_par_iter().for_each_with(tx, |tx, (id, path, stamp, duration)| {
                if stop_state.health.stop.load(Ordering::Relaxed) {
                    return;
                }
                let result = match kind {
                    Kind::Quality => match check_quality(Path::new(&path), duration) {
                        Ok(q) => (Some(q), None),
                        Err(e) => {
                            log::warn!("quality check failed for {path}: {e:#}");
                            (None, None)
                        }
                    },
                    Kind::Integrity => (None, Some(check_integrity(Path::new(&path)))),
                };
                let _ = tx.send((id, stamp, result));
            });
        });
        let mut saved = Instant::now();
        let mut done = 0;
        for (id, stamp, (quality, integrity)) in rx {
            {
                let mut store = state.health.store.lock();
                let e = store.entry(&id, &stamp);
                if quality.is_some() {
                    e.quality = quality;
                }
                if integrity.is_some() {
                    e.integrity = integrity;
                }
            }
            done += 1;
            if saved.elapsed() > Duration::from_secs(30) {
                state.save_health();
                saved = Instant::now();
            }
            progress(done, true);
        }
        state.save_health();
        *state.health.running.lock() = None;
        log::info!("{kind:?} check of {done}/{total} songs took {:?}", started.elapsed());
        progress(done, false);
        ctx.emit("health-done", kind);
    });
    true
}

pub fn stop(state: &crate::state::AppState) {
    state.health.stop.store(true, Ordering::SeqCst);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthProgress {
    kind: Kind,
    done: usize,
    total: usize,
    running: bool,
}

// ---------- the report ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityRow {
    pub id: String,
    pub verdict: Verdict,
    pub cutoff_hz: Option<f32>,
    pub real_bits: Option<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityRow {
    pub id: String,
    #[serde(flatten)]
    pub result: Integrity,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub running: Option<Kind>,
    pub songs: usize,
    pub lossless: usize,
    pub quality_checked: usize,
    pub integrity_checked: usize,
    pub quality: Vec<QualityRow>,
    pub integrity: Vec<IntegrityRow>,
}

pub fn report(state: &crate::state::AppState) -> Report {
    let lib = state.library.lock();
    let store = state.health.store.lock();
    let mut r = Report {
        running: *state.health.running.lock(),
        songs: lib.snapshot.tracks.iter().map(key).collect::<std::collections::HashSet<_>>().len(),
        lossless: lib.snapshot.tracks.iter().filter(|t| is_lossless(&t.codec)).map(key).collect::<std::collections::HashSet<_>>().len(),
        quality_checked: 0,
        integrity_checked: 0,
        quality: Vec::new(),
        integrity: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    for t in &lib.snapshot.tracks {
        // A file once (the songs of a single-file album share it).
        if t.cue.is_some() && !seen.insert(key(t)) {
            continue;
        }
        let Some(e) = store.tracks.get(&key(t)).filter(|e| e.stamp == stamp(t)) else { continue };
        if let Some(q) = &e.quality {
            r.quality_checked += 1;
            if q.is_suspect() {
                r.quality.push(QualityRow { id: t.id.clone(), verdict: q.verdict, cutoff_hz: q.cutoff_hz, real_bits: q.real_bits });
            }
        }
        if let Some(i) = &e.integrity {
            r.integrity_checked += 1;
            if i.is_bad() {
                r.integrity.push(IntegrityRow { id: t.id.clone(), result: i.clone() });
            }
        }
    }
    // The worst first.
    r.quality.sort_by_key(|q| match q.verdict {
        Verdict::Lossy => 0,
        Verdict::Upsampled => 1,
        Verdict::Suspect => 2,
        Verdict::Good => 3,
    });
    r
}

// ---------- duplicates ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupSong {
    pub id: String,
    /// The copy to keep.
    pub keep: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
    /// "same": the same song twice on one album. "lossy": a lossy copy of a song that's also lossless.
    pub kind: &'static str,
    pub songs: Vec<DupSong>,
    /// Bytes the copies to remove take.
    pub wasted: u64,
}

/// How good a copy is: lossless first, then bit depth × rate, bitrate, size; a name that isn't a
/// re-download ("name_1730000000.flac") wins a tie.
fn rank(t: &Track) -> (bool, u64, u32, u64, bool) {
    let stem = Path::new(&t.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let redownload = stem.rsplit('_').next().is_some_and(|n| n.len() >= 9 && n.chars().all(|c| c.is_ascii_digit()));
    (
        is_lossless(&t.codec),
        u64::from(t.bit_depth.unwrap_or(16)) * u64::from(t.sample_rate.unwrap_or(44_100)),
        t.bitrate.unwrap_or(0),
        t.size,
        !redownload,
    )
}

/// Songs that exist more than once. Two copies on one album are duplicates; the same song on
/// different albums (single, album, compilation) is not, except that a lossy copy of a lossless
/// song is.
pub fn duplicates(tracks: &[Track]) -> Vec<DupGroup> {
    let mut by_song: HashMap<(String, String), Vec<&Track>> = HashMap::new();
    for t in tracks {
        let (artist, title) = (match_key(&t.artist), match_key(&t.title));
        // (Songs of a single-file album share their file with the rest of the album: deleting one
        // would delete them all.)
        if title.is_empty() || artist.is_empty() || artist == "unknownartist" || t.cue.is_some() {
            continue;
        }
        by_song.entry((artist, title)).or_default().push(t);
    }
    let mut groups = Vec::new();
    for (_, mut songs) in by_song {
        if songs.len() < 2 {
            continue;
        }
        // The same recording: of the same length.
        songs.sort_by(|a, b| a.duration.total_cmp(&b.duration));
        let mut recordings: Vec<Vec<&Track>> = Vec::new();
        for t in songs {
            match recordings.last_mut() {
                Some(r) if (t.duration - r[0].duration).abs() <= SAME_LENGTH => r.push(t),
                _ => recordings.push(vec![t]),
            }
        }
        for rec in recordings.into_iter().filter(|r| r.len() > 1) {
            let mut handled: Vec<&str> = Vec::new();
            // The same song on the same album (and disc).
            let mut by_album: HashMap<(String, u32), Vec<&Track>> = HashMap::new();
            for t in &rec {
                by_album.entry((match_key(&t.album), t.disc_no.unwrap_or(1))).or_default().push(t);
            }
            for (_, mut copies) in by_album.into_iter().filter(|(_, c)| c.len() > 1) {
                copies.sort_by(|a, b| rank(b).cmp(&rank(a)));
                handled.extend(copies.iter().map(|t| t.id.as_str()));
                groups.push(group("same", &copies));
            }
            // Lossy copies (not already in a group above) of a song that exists lossless.
            let best = rec.iter().filter(|t| is_lossless(&t.codec)).max_by_key(|t| rank(t));
            if let Some(best) = best {
                let lossy: Vec<&Track> = rec.iter().copied().filter(|t| !is_lossless(&t.codec) && !handled.contains(&t.id.as_str())).collect();
                if !lossy.is_empty() {
                    let mut copies = vec![*best];
                    copies.extend(lossy);
                    groups.push(group("lossy", &copies));
                }
            }
        }
    }
    groups.sort_by(|a, b| b.wasted.cmp(&a.wasted));
    groups
}

/// `copies` best first: the first is kept, the rest go.
fn group(kind: &'static str, copies: &[&Track]) -> DupGroup {
    DupGroup {
        kind,
        wasted: copies.iter().skip(1).map(|t| t.size).sum(),
        songs: copies.iter().enumerate().map(|(i, t)| DupSong { id: t.id.clone(), keep: i == 0 }).collect(),
    }
}

// ---------- removing copies ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub deleted: usize,
    pub freed: u64,
    /// Songs that were left alone, with the reason.
    pub skipped: Vec<String>,
}

/// Deletes the files of these songs (not to the Recycle Bin), but only the ones that are inside a
/// music folder, aren't playing, and have another copy of the same song staying in the library.
pub fn remove(ctx: &Ctx, ids: &[String]) -> Removed {
    let state = &ctx.state;
    let folders: Vec<String> = state.settings.lock().folders.clone();
    let playing = {
        let s = state.engine.status();
        s.index.and_then(|i| s.queue.get(i)).cloned()
    };
    let mut out = Removed { deleted: 0, freed: 0, skipped: Vec::new() };
    let doomed: Vec<Track> = {
        let lib = state.library.lock();
        let wanted: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let staying: Vec<&Track> = lib.snapshot.tracks.iter().filter(|t| !wanted.contains(t.id.as_str())).collect();
        let mut doomed = Vec::new();
        for id in ids {
            let Some(t) = lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]) else { continue };
            let name = format!("{} – {}", t.artist, t.title);
            let inside = folders.iter().any(|f| Path::new(&t.path).starts_with(f));
            let has_copy = staying.iter().any(|o| {
                match_key(&o.title) == match_key(&t.title) && match_key(&o.artist) == match_key(&t.artist) && (o.duration - t.duration).abs() <= SAME_LENGTH
            });
            if t.cue.is_some() {
                out.skipped.push(format!("{name}: part of a single-file album, its file holds the whole album"));
            } else if playing.as_deref() == Some(t.id.as_str()) {
                out.skipped.push(format!("{name}: it's playing"));
            } else if !inside {
                out.skipped.push(format!("{name}: not inside your music folders"));
            } else if !has_copy {
                out.skipped.push(format!("{name}: no other copy of it would be left"));
            } else {
                doomed.push(t.clone());
            }
        }
        doomed
    };
    for t in doomed {
        match std::fs::remove_file(&t.path) {
            Ok(()) => {
                out.deleted += 1;
                out.freed += t.size;
            }
            Err(e) => out.skipped.push(format!("{} – {}: {e}", t.artist, t.title)),
        }
    }
    if out.deleted > 0 {
        log::info!("removed {} duplicate files ({} bytes)", out.deleted, out.freed);
        crate::jobs::start_scan(ctx.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noise up to `cutoff` Hz (a brick wall, like an encoder's low-pass), rounded to 16 bits.
    fn noise(rate: u32, cutoff: f32, secs: usize) -> Vec<f32> {
        let len = rate as usize * secs;
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut white: Vec<f32> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
            })
            .collect();
        let mut planner = RealFftPlanner::<f32>::new();
        let (fwd, inv) = (planner.plan_fft_forward(len), planner.plan_fft_inverse(len));
        let mut spec = fwd.make_output_vec();
        fwd.process(&mut white, &mut spec).unwrap();
        let first_cut = (cutoff / rate as f32 * len as f32) as usize;
        for c in spec.iter_mut().skip(first_cut) {
            *c = Default::default();
        }
        // The inverse transform wants a real DC and Nyquist.
        spec[0].im = 0.0;
        spec.last_mut().unwrap().im = 0.0;
        let mut out = inv.make_output_vec();
        inv.process(&mut spec, &mut out).unwrap();
        let rms = (out.iter().map(|v| v * v).sum::<f32>() / len as f32).sqrt();
        out.iter().map(|v| ((v / rms * 0.1) * 32768.0).round() / 32768.0).collect()
    }

    #[test]
    fn finds_an_encoders_low_pass() {
        let lossy = spectrum_cutoff(&noise(44_100, 16_000.0, 4), 44_100).unwrap();
        let c = lossy.expect("a cliff");
        assert!((15_000.0..=16_500.0).contains(&c), "{c}");
        assert_eq!(verdict(44_100, Some(c)), Verdict::Lossy);
        assert_eq!(spectrum_cutoff(&noise(44_100, 22_050.0, 4), 44_100).unwrap(), None, "music to the top");
        // CD-rate audio in a 96 kHz file.
        let up = spectrum_cutoff(&noise(96_000, 21_500.0, 4), 96_000).unwrap().expect("a cliff");
        assert_eq!(verdict(96_000, Some(up)), Verdict::Upsampled, "{up}");
    }

    #[test]
    fn silence_tells_nothing() {
        assert_eq!(spectrum_cutoff(&vec![0.0; 44_100 * 4], 44_100), None);
    }

    #[test]
    fn verdicts_follow_the_rate() {
        assert_eq!(verdict(44_100, Some(20_500.0)), Verdict::Good, "an ordinary CD anti-alias filter");
        assert_eq!(verdict(44_100, Some(18_500.0)), Verdict::Suspect);
        assert_eq!(verdict(48_000, None), Verdict::Good);
        assert_eq!(verdict(192_000, Some(30_000.0)), Verdict::Good);
    }

    #[test]
    fn finds_padded_bits() {
        let scale = 8_388_608.0;
        let really_16: Vec<f32> = [3, -7, 120, 5000, -32000].iter().map(|&v| (v * 256) as f32 / scale).collect();
        assert_eq!(padded_bits(&really_16, 24), 8);
        let full: Vec<f32> = [3, -7, 121, 5001].iter().map(|&v| (v * 256 + 1) as f32 / scale).collect();
        assert_eq!(padded_bits(&full, 24), 0);
        assert_eq!(padded_bits(&[0.0; 10], 24), 0);
    }

    /// Run by hand on real files: CADENCE_HEALTH_FILES=list.tsv (path, tab, duration per line).
    #[test]
    #[ignore]
    fn real_files() {
        let Ok(list) = std::env::var("CADENCE_HEALTH_FILES") else { return };
        for line in std::fs::read_to_string(list).unwrap().lines() {
            let (path, duration) = line.split_once('\t').unwrap();
            let name = Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
            let started = Instant::now();
            let q = check_quality(Path::new(path), duration.parse().unwrap_or(0.0));
            let i = if std::env::var("CADENCE_HEALTH_SKIP_INTEGRITY").is_ok() { Integrity::default() } else { check_integrity(Path::new(path)) };
            // Levels (dB) around the interesting frequencies, to see why the verdict is what it is.
            let levels = read_for_analysis(Path::new(path), duration.parse().unwrap_or(0.0))
                .ok()
                .and_then(|a| band_levels(&a.mono, a.rate).map(|(b, hz)| (a.rate, b, hz)))
                .map(|(rate, b, hz)| {
                    let at = |khz: f32| b.get((khz * 1000.0 / hz) as usize).map_or(0, |v| v.round() as i32);
                    format!("{}k [10k {} 16k {} 19k {} 21k {} 23k {} 30k {}]", rate / 1000, at(10.0), at(16.0), at(19.0), at(21.0), at(23.0), at(30.0))
                });
            println!(
                "{:?} {} | i verified={:?} errors={} trunc={} failed={:?} | {:.1}s | {name}",
                q.as_ref().map(|q| (q.verdict, q.cutoff_hz.map(|c| (c / 100.0).round() / 10.0), q.real_bits)).map_err(|e| e.to_string()),
                levels.unwrap_or_default(),
                i.verified,
                i.errors,
                i.truncated,
                i.failed,
                started.elapsed().as_secs_f32(),
            );
        }
    }

    /// Run by hand: the duplicates in the real library (CADENCE_LIBRARY=...\library.json).
    #[test]
    #[ignore]
    fn real_duplicates() {
        let Ok(file) = std::env::var("CADENCE_LIBRARY") else { return };
        let cache: crate::library::LibraryCache = crate::state::read_json(Path::new(&file));
        let by_id: HashMap<&str, &Track> = cache.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
        let groups = duplicates(&cache.tracks);
        let wasted: u64 = groups.iter().map(|g| g.wasted).sum();
        println!("{} songs, {} groups ({} same, {} lossy), {} MB wasted", cache.tracks.len(), groups.len(), groups.iter().filter(|g| g.kind == "same").count(), groups.iter().filter(|g| g.kind == "lossy").count(), wasted / 1_000_000);
        for g in groups.iter().take(12) {
            println!("-- {} ({} MB)", g.kind, g.wasted / 1_000_000);
            for s in &g.songs {
                let t = by_id[s.id.as_str()];
                println!("   {} {} | {} | {} | {} {:?}/{:?} | {}", if s.keep { "KEEP" } else { "del " }, t.title, t.artist, t.album, t.codec, t.bit_depth, t.sample_rate, t.path);
            }
        }
    }

    fn song(id: &str, album: &str, codec: &str, size: u64, path: &str) -> Track {
        Track {
            id: id.into(),
            title: "Lemon".into(),
            artist: "Kenshi Yonezu".into(),
            album: album.into(),
            duration: 255.0,
            codec: codec.into(),
            size,
            path: path.into(),
            bit_depth: Some(16),
            sample_rate: Some(44_100),
            ..Default::default()
        }
    }

    #[test]
    fn finds_copies_and_keeps_the_best() {
        let tracks = vec![
            song("a", "Lemon", "FLAC", 30, "D:\\m\\lemon.flac"),
            song("b", "Lemon", "FLAC", 30, "D:\\m\\lemon_1730000000.flac"),
            song("c", "Lemon", "MP3", 8, "D:\\m\\lemon.mp3"),
            song("d", "Greatest Hits", "FLAC", 30, "D:\\m\\hits\\lemon.flac"),
            song("e", "Singles", "MP3", 8, "D:\\m\\singles\\lemon.mp3"),
        ];
        let groups = duplicates(&tracks);
        let same = groups.iter().find(|g| g.kind == "same").expect("copies on one album");
        let ids: Vec<&str> = same.songs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"], "best first; the re-download after the original; lossy last");
        assert!(same.songs[0].keep && !same.songs[1].keep && !same.songs[2].keep);
        assert_eq!(same.wasted, 38);
        // The lossy single of a song that exists lossless goes; the lossless one on another album stays.
        let lossy = groups.iter().find(|g| g.kind == "lossy").expect("lossy twin");
        assert!(lossy.songs.iter().any(|s| s.id == "e" && !s.keep));
        assert!(lossy.songs.iter().any(|s| s.keep));
        assert!(groups.iter().all(|g| g.songs.iter().all(|s| s.id != "d" || s.keep)));
    }

    #[test]
    fn different_discs_and_lengths_are_not_copies() {
        let mut a = song("a", "Live", "FLAC", 30, "a.flac");
        let mut b = song("b", "Live", "FLAC", 30, "b.flac");
        a.disc_no = Some(1);
        b.disc_no = Some(2);
        assert!(duplicates(&[a, b]).is_empty(), "an intro on each disc");
        let a = song("a", "Lemon", "FLAC", 30, "a.flac");
        let mut b = song("b", "Lemon", "FLAC", 30, "b.flac");
        b.duration = 300.0;
        assert!(duplicates(&[a, b]).is_empty(), "a live version is longer");
    }
}
