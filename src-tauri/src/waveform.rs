//! Waveforms for the seek bar: stored, computed when missing, prepared for the whole library on
//! request.
//!
//! Storage: one small file per song, `waveforms/<song id>.wf` in the data folder, read only when
//! that song's waveform is asked for (never loaded all at once, never in `library.json`). Song ids
//! are hashes of the path, so a moved or replaced file gets a new id and its old file becomes an
//! orphan: `prune` drops those after a rescan that changed the library. The folder is a rebuildable
//! cache, so backups leave it out (`backup` keeps only what it lists).
//!
//! File format (little-endian), version 1:
//! ```text
//! 0  "CDWF"        magic
//! 4  u8            format version (1)
//! 5  u8            flags: bit 0 = an RMS row follows the peaks
//! 6  u16           buckets (n)
//! 8  u64           the audio file's size   } the file it was made from: a file replaced at the
//! 16 u64           its modified time       } same path no longer matches and is drawn again
//! 24 [u8; n]       peaks, 0..=255 of full scale
//! .. [u8; n]       RMS (with flag bit 0)
//! ```
//! Anything else (another version, a short or long file, a stamp of another file) reads as missing.
//!
//! Computing: the loudness job draws the waveforms of songs it measures, from the same decode
//! (`jobs::start_analysis`). Songs measured before waveforms existed are drawn when the window
//! asks for one (`for_track`): one song at a time on one background thread (`Lazy`), after a short
//! quiet spell so skipping through songs doesn't decode each, and the window is told
//! ("waveform-ready"). "Prepare waveforms for all songs" (`prepare_all`) does the rest of the
//! library as a visible, pausable job on the background pool.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use rayon::prelude::*;
use serde::Serialize;

use crate::audio::envelope::{self, Envelope};
use crate::ctx::Ctx;
use crate::library::Track;

pub const DIR: &str = "waveforms";
const MAGIC: &[u8; 4] = b"CDWF";
const VERSION: u8 = 1;
const HAS_RMS: u8 = 1;
const HEADER: usize = 24;

/// The file a waveform was made from, as the library knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stamp {
    pub size: u64,
    pub modified: u64,
}

impl Stamp {
    pub fn of(t: &Track) -> Self {
        Self { size: t.size, modified: t.modified }
    }
}

pub fn encode(e: &Envelope, stamp: Stamp) -> Vec<u8> {
    let n = e.peak.len().min(u16::MAX as usize);
    let rms = e.rms.len() == n;
    let mut b = Vec::with_capacity(HEADER + n * 2);
    b.extend_from_slice(MAGIC);
    b.push(VERSION);
    b.push(if rms { HAS_RMS } else { 0 });
    b.extend_from_slice(&(n as u16).to_le_bytes());
    b.extend_from_slice(&stamp.size.to_le_bytes());
    b.extend_from_slice(&stamp.modified.to_le_bytes());
    b.extend_from_slice(&e.peak[..n]);
    if rms {
        b.extend_from_slice(&e.rms[..n]);
    }
    b
}

/// Whether `b` is a whole waveform file of this format, made from the file `stamp` describes.
pub fn valid(b: &[u8], stamp: Stamp) -> bool {
    if b.len() < HEADER || &b[..4] != MAGIC || b[4] != VERSION {
        return false;
    }
    let n = u16::from_le_bytes([b[6], b[7]]) as usize;
    let rows = if b[5] & HAS_RMS != 0 { 2 } else { 1 };
    let size = u64::from_le_bytes(b[8..16].try_into().unwrap());
    let modified = u64::from_le_bytes(b[16..24].try_into().unwrap());
    n > 0 && b.len() == HEADER + n * rows && Stamp { size, modified } == stamp
}

/// The envelope in a waveform file (the window reads the file itself, see waveform.ts).
#[cfg(test)]
pub fn decode(b: &[u8], stamp: Stamp) -> Option<Envelope> {
    if !valid(b, stamp) {
        return None;
    }
    let n = u16::from_le_bytes([b[6], b[7]]) as usize;
    let peak = b[HEADER..HEADER + n].to_vec();
    let rms = b.get(HEADER + n..HEADER + 2 * n).map(<[u8]>::to_vec).unwrap_or_default();
    Some(Envelope { peak, rms })
}

/// Where a song's waveform lives; None for an id that isn't a plain file name.
pub fn file(data_dir: &Path, id: &str) -> Option<PathBuf> {
    let plain = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| data_dir.join(DIR).join(format!("{id}.wf")))
}

/// The stored waveform of a song, as its file's bytes; None when there's none for this file.
pub fn load(data_dir: &Path, id: &str, stamp: Stamp) -> Option<Vec<u8>> {
    let b = std::fs::read(file(data_dir, id)?).ok()?;
    valid(&b, stamp).then_some(b)
}

pub fn has(data_dir: &Path, id: &str, stamp: Stamp) -> bool {
    load(data_dir, id, stamp).is_some()
}

/// Stores a waveform: written beside its place and renamed over it, so a reader never sees half a
/// file. Not synced to the disk: a waveform lost in a power cut is simply drawn again.
pub fn save(data_dir: &Path, id: &str, stamp: Stamp, e: &Envelope) -> std::io::Result<()> {
    let path = file(data_dir, id).ok_or_else(|| std::io::Error::other("not a song id"))?;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{id}.wf.tmp"));
    std::fs::write(&tmp, encode(e, stamp))?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Removes the waveforms of songs no longer in the library (and left-over temporary files).
/// Returns how many files went.
pub fn prune(data_dir: &Path, keep: &HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(data_dir.join(DIR)) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let stale = match name.strip_suffix(".wf") {
            Some(id) => !keep.contains(id),
            None => name.ends_with(".wf.tmp"),
        };
        if stale && std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `prune` on a background thread, with the library as it is now.
pub fn prune_later(ctx: &Ctx) {
    let state = ctx.state.clone();
    if !state.data_dir.join(DIR).is_dir() {
        return;
    }
    std::thread::spawn(move || {
        crate::jobs::background_priority();
        let keep: HashSet<String> = state.library.lock().by_id.keys().cloned().collect();
        let removed = prune(&state.data_dir, &keep);
        if removed > 0 {
            log::info!("removed {removed} waveforms of songs no longer in the library");
        }
    });
}

/// One song to draw.
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub path: PathBuf,
    /// Its span in the file (a CUE-sheet song), in seconds.
    pub start: f64,
    pub end: Option<f64>,
    pub stamp: Stamp,
}

impl Job {
    pub fn of(t: &Track) -> Self {
        Self {
            id: t.id.clone(),
            path: PathBuf::from(&t.path),
            start: t.cue.map_or(0.0, |c| c.start),
            end: t.cue.and_then(|c| c.end),
            stamp: Stamp::of(t),
        }
    }

    /// Decodes the song and stores its waveform. Ok(false): it has no length to draw by.
    pub fn run(&self, data_dir: &Path) -> anyhow::Result<bool> {
        match envelope::compute(&self.path, self.start, self.end)? {
            Some(e) => {
                save(data_dir, &self.id, self.stamp, &e)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[derive(Default)]
struct LazyState {
    /// The latest song asked for (an older one waiting is replaced: only the song on screen matters).
    pending: Option<Job>,
    /// Requests so far, to tell whether a newer one came during the quiet spell.
    asked: u64,
    busy: Option<String>,
    /// Songs that couldn't be drawn (no length, unreadable): not tried again this session.
    failed: HashSet<(String, Stamp)>,
}

/// Draws one song at a time on its own background thread, a short while after it was asked for.
pub struct Lazy {
    shared: Arc<(Mutex<LazyState>, Condvar)>,
}

impl Lazy {
    /// `done(id, drawn)` is called on the worker thread after each song.
    pub fn new(data_dir: PathBuf, quiet: Duration, done: impl Fn(&str, bool) + Send + 'static) -> Self {
        let shared = Arc::new((Mutex::new(LazyState::default()), Condvar::new()));
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("cadence-waveform".into())
            .spawn(move || {
                crate::jobs::background_priority();
                let (lock, cv) = &*worker;
                loop {
                    let job = {
                        let mut s = lock.lock();
                        while s.pending.is_none() {
                            cv.wait(&mut s);
                        }
                        // Quiet first: the song just changed (a gapless or crossfade transition is
                        // under way), and the listener may be skipping on.
                        loop {
                            let asked = s.asked;
                            cv.wait_for(&mut s, quiet);
                            if s.asked == asked {
                                break;
                            }
                        }
                        let job = s.pending.take().unwrap();
                        s.busy = Some(job.id.clone());
                        job
                    };
                    let drawn = match job.run(&data_dir) {
                        Ok(d) => d,
                        Err(e) => {
                            log::warn!("can't draw the waveform of {}: {e:#}", job.path.display());
                            false
                        }
                    };
                    {
                        let mut s = lock.lock();
                        s.busy = None;
                        if !drawn {
                            s.failed.insert((job.id.clone(), job.stamp));
                        }
                    }
                    done(&job.id, drawn);
                }
            })
            .expect("waveform thread");
        Self { shared }
    }

    /// Asks for a song to be drawn; returns at once. False: it couldn't be drawn before.
    pub fn request(&self, job: Job) -> bool {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock();
        if s.failed.contains(&(job.id.clone(), job.stamp)) {
            return false;
        }
        let same = s.busy.as_deref() == Some(&job.id) || s.pending.as_ref().is_some_and(|p| p.id == job.id);
        if !same {
            s.pending = Some(job);
            s.asked += 1;
            cv.notify_all();
        }
        true
    }
}

static LAZY: OnceLock<Lazy> = OnceLock::new();

fn lazy(ctx: &Ctx) -> &'static Lazy {
    LAZY.get_or_init(|| {
        let c = ctx.clone();
        Lazy::new(ctx.state.data_dir.clone(), Duration::from_millis(1500), move |id, drawn| {
            c.emit("waveform-ready", Ready { track_id: id.to_string(), drawn });
        })
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Ready {
    track_id: String,
    drawn: bool,
}

/// A song's waveform file (see the format above), or nothing: no such song, or it isn't drawn yet
/// (it is being drawn now, and "waveform-ready" says when it is).
pub fn for_track(ctx: &Ctx, id: &str) -> Vec<u8> {
    let state = &ctx.state;
    let Some(job) = ({
        let lib = state.library.lock();
        lib.by_id.get(id).map(|&i| Job::of(&lib.snapshot.tracks[i]))
    }) else {
        return Vec::new();
    };
    if let Some(b) = load(&state.data_dir, id, job.stamp) {
        return b;
    }
    lazy(ctx).request(job);
    Vec::new()
}

// ---------- Prepare waveforms for all songs ----------

static RUNNING: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static DONE: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub running: bool,
    pub paused: bool,
    pub done: usize,
    pub total: usize,
}

pub fn status() -> JobStatus {
    JobStatus {
        running: RUNNING.load(Ordering::SeqCst),
        paused: PAUSED.load(Ordering::SeqCst),
        done: DONE.load(Ordering::SeqCst),
        total: TOTAL.load(Ordering::SeqCst),
    }
}

fn report(ctx: &Ctx) {
    ctx.emit("waveform-progress", status());
}

pub fn pause(ctx: &Ctx, paused: bool) {
    PAUSED.store(paused, Ordering::SeqCst);
    report(ctx);
}

pub fn stop(ctx: &Ctx) {
    STOP.store(true, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    report(ctx);
}

/// Draws the waveform of every song that has none, on the background pool (at most two threads,
/// background priority), waiting while the loudness or sound analysis runs (they use the pool's
/// threads) and while paused. False: it is already running.
pub fn prepare_all(ctx: &Ctx) -> bool {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    STOP.store(false, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    DONE.store(0, Ordering::SeqCst);
    TOTAL.store(0, Ordering::SeqCst);
    report(ctx);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        crate::jobs::background_priority();
        let state = ctx.state.clone();
        let tracks: Vec<Job> = state.library.lock().snapshot.tracks.iter().filter(|t| !crate::jobs::settling(t)).map(Job::of).collect();
        let todo: Vec<Job> = tracks.into_iter().filter(|j| !has(&state.data_dir, &j.id, j.stamp)).collect();
        TOTAL.store(todo.len(), Ordering::SeqCst);
        report(&ctx);
        let started = std::time::Instant::now();
        let last = AtomicU64::new(0);
        let busy = || {
            let lib = state.library.lock();
            lib.analyzing || lib.listening || lib.scanning
        };
        crate::jobs::background_pool().install(|| {
            todo.par_iter().for_each(|job| {
                while !STOP.load(Ordering::SeqCst) && (PAUSED.load(Ordering::SeqCst) || busy()) {
                    std::thread::sleep(Duration::from_millis(500));
                }
                if STOP.load(Ordering::SeqCst) {
                    return;
                }
                if let Err(e) = job.run(&state.data_dir) {
                    log::debug!("no waveform for {}: {e:#}", job.path.display());
                }
                let done = DONE.fetch_add(1, Ordering::SeqCst) + 1;
                // The sidebar's progress at most 5 times a second.
                let now = started.elapsed().as_millis() as u64;
                let was = last.load(Ordering::Relaxed);
                if now >= was + 200 && last.compare_exchange(was, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() || done == todo.len() {
                    report(&ctx);
                }
            });
        });
        log::info!("waveforms of {} songs drawn in {:?}", DONE.load(Ordering::SeqCst), started.elapsed());
        RUNNING.store(false, Ordering::SeqCst);
        PAUSED.store(false, Ordering::SeqCst);
        report(&ctx);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: Stamp = Stamp { size: 1234, modified: 1_700_000_000 };

    fn env(n: usize) -> Envelope {
        Envelope { peak: (0..n).map(|i| (i % 256) as u8).collect(), rms: (0..n).map(|i| (i / 4 % 256) as u8).collect() }
    }

    #[test]
    fn stored_waveforms_read_back_with_their_header() {
        let dir = tempfile::tempdir().unwrap();
        let e = env(1000);
        save(dir.path(), "abc123", STAMP, &e).unwrap();
        let b = load(dir.path(), "abc123", STAMP).unwrap();
        assert_eq!(&b[..6], b"CDWF\x01\x01");
        assert_eq!(b.len(), 24 + 2000);
        assert_eq!(decode(&b, STAMP), Some(e.clone()));
        // Made from another file (replaced at the same path): missing.
        assert!(load(dir.path(), "abc123", Stamp { size: 1235, ..STAMP }).is_none());
        assert!(!has(dir.path(), "nope", STAMP));
        // Ids that aren't plain names never reach the disk.
        assert!(file(dir.path(), "../x").is_none());
        assert!(save(dir.path(), "a/b", STAMP, &e).is_err());
    }

    #[test]
    fn a_damaged_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let good = encode(&env(500), STAMP);
        let mut other = good.clone();
        other[4] = 2; // a later format version
        let mut magic = good.clone();
        magic[0] = b'X';
        let mut count = good.clone();
        count[6] = 0xFF; // says more buckets than it has
        for (n, bad) in [good[..good.len() - 1].to_vec(), good[..10].to_vec(), Vec::new(), other, magic, count].into_iter().enumerate() {
            let id = format!("bad{n}");
            std::fs::create_dir_all(dir.path().join(DIR)).unwrap();
            std::fs::write(file(dir.path(), &id).unwrap(), &bad).unwrap();
            assert!(load(dir.path(), &id, STAMP).is_none(), "case {n}");
            assert!(decode(&bad, STAMP).is_none(), "case {n}");
        }
        // Without the RMS row it still reads.
        let peaks = encode(&Envelope { peak: vec![7; 10], rms: Vec::new() }, STAMP);
        assert_eq!(decode(&peaks, STAMP).unwrap().peak, vec![7; 10]);
    }

    #[test]
    fn orphans_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["keep1", "keep2", "gone1", "gone2"] {
            save(dir.path(), id, STAMP, &env(10)).unwrap();
        }
        std::fs::write(dir.path().join(DIR).join("keep1.wf.tmp"), b"half").unwrap();
        std::fs::write(dir.path().join(DIR).join("notes.txt"), b"left alone").unwrap();
        let keep: HashSet<String> = ["keep1", "keep2"].map(String::from).into();
        assert_eq!(prune(dir.path(), &keep), 3);
        assert!(has(dir.path(), "keep1", STAMP) && has(dir.path(), "keep2", STAMP));
        assert!(!has(dir.path(), "gone1", STAMP));
        assert!(dir.path().join(DIR).join("notes.txt").exists());
        // No folder yet: nothing to do.
        assert_eq!(prune(&dir.path().join("elsewhere"), &keep), 0);
    }

    #[test]
    fn drawing_later_does_not_hold_up_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("song.wav");
        crate::audio::envelope::test_wav(&wav, 44_100 * 20);
        let (tx, rx) = crossbeam_channel::unbounded();
        let lazy = Lazy::new(dir.path().to_path_buf(), Duration::from_millis(50), move |id, drawn| {
            let _ = tx.send((id.to_string(), drawn));
        });
        let job = |id: &str, path: &Path| Job { id: id.into(), path: path.to_path_buf(), start: 0.0, end: None, stamp: STAMP };
        let asked = std::time::Instant::now();
        assert!(lazy.request(job("s1", &wav)));
        assert!(lazy.request(job("s1", &wav)), "asking again while it waits is fine");
        assert!(asked.elapsed() < Duration::from_millis(40), "returns at once");
        assert!(!has(dir.path(), "s1", STAMP), "not drawn yet: it waits for a quiet moment");
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), ("s1".to_string(), true));
        let e = decode(&load(dir.path(), "s1", STAMP).unwrap(), STAMP).unwrap();
        assert_eq!(e.peak.len(), envelope::BUCKETS);
        // A file that can't be read: reported, and not tried again.
        assert!(lazy.request(job("s2", &dir.path().join("missing.flac"))));
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), ("s2".to_string(), false));
        assert!(!lazy.request(job("s2", &dir.path().join("missing.flac"))));
    }

    #[test]
    fn a_newer_request_replaces_one_still_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("song.wav");
        crate::audio::envelope::test_wav(&wav, 44_100);
        let (tx, rx) = crossbeam_channel::unbounded();
        let lazy = Lazy::new(dir.path().to_path_buf(), Duration::from_millis(200), move |id, _| {
            let _ = tx.send(id.to_string());
        });
        for id in ["a", "b", "c"] {
            lazy.request(Job { id: id.into(), path: wav.clone(), start: 0.0, end: None, stamp: STAMP });
        }
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), "c");
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err(), "the skipped songs aren't decoded");
    }
}
