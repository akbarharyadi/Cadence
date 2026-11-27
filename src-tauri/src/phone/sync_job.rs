//! The phone's sync: reads the computer's list, downloads what's new into the sync folder
//! (`Music/Cadence`), removes what this phone copied that's no longer chosen, and records what it
//! holds in its ledger (`phone-sync.json`). One song at a time on a thread of its own (it waits on
//! the network, not the processor); it can be paused and stopped, and a stopped or dropped song
//! carries on next time.
//!
//! A file already at a song's place that isn't this phone's copy is never written over: when its
//! bytes are the song (a copy from before the ledger was lost) it's taken as the copy, else the
//! song gets another name beside it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use super::client::{self, Link};
use super::manifest::Entry;
use super::plan::{self, Held, Ledger};
use crate::ctx::Ctx;

const LEDGER: &str = "phone-sync.json";
/// Room left free on the phone after a sync.
const SPARE: u64 = 300 << 20;
/// A sync gives up after this many songs fail in a row (the computer is likely gone).
const MAX_FAILS_IN_A_ROW: u32 = 3;

/// Where synced music goes on the phone.
#[cfg(target_os = "android")]
pub const ROOT: Option<&str> = Some("/storage/emulated/0/Music/Cadence");
#[cfg(not(target_os = "android"))]
pub const ROOT: Option<&str> = None;

static RUNNING: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<SyncStatus> = Mutex::new(SyncStatus::new());

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub running: bool,
    pub paused: bool,
    /// "connecting", "copying", "removing", or "" when it isn't running.
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// The song being copied.
    pub current: String,
    /// How the last sync ended, in words (an error, or what it did).
    pub message: String,
    pub failed: bool,
}

impl SyncStatus {
    const fn new() -> Self {
        Self { running: false, paused: false, phase: "", done: 0, total: 0, bytes_done: 0, bytes_total: 0, current: String::new(), message: String::new(), failed: false }
    }
}

pub fn status() -> SyncStatus {
    let mut s = STATUS.lock().clone();
    s.running = RUNNING.load(Ordering::SeqCst);
    s.paused = PAUSED.load(Ordering::SeqCst);
    s
}

fn report(ctx: &Ctx) {
    ctx.emit("phone-sync-progress", status());
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

pub fn load_ledger(data_dir: &Path) -> Ledger {
    crate::state::read_json(&data_dir.join(LEDGER))
}

fn save_ledger(data_dir: &Path, l: &Ledger) {
    crate::state::write_json(&data_dir.join(LEDGER), l);
}

/// Starts a sync into `root`. False: one is running already.
pub fn start(ctx: &Ctx, root: PathBuf) -> bool {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    STOP.store(false, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    *STATUS.lock() = SyncStatus { phase: "connecting", ..SyncStatus::new() };
    report(ctx);
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("cadence-phone-sync".into()).spawn(move || {
        crate::jobs::background_priority();
        let result = run(&ctx, &root);
        let changed = matches!(&result, Ok((_, true)));
        {
            let mut s = STATUS.lock();
            s.phase = "";
            s.current.clear();
            (s.message, s.failed) = match result {
                Ok((words, _)) => (words, false),
                Err(e) => (e, true),
            };
        }
        RUNNING.store(false, Ordering::SeqCst);
        PAUSED.store(false, Ordering::SeqCst);
        report(&ctx);
        if changed {
            crate::api::rescan(&ctx);
        }
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

/// The sync. Ok: what it did in words, and whether any file changed.
fn run(ctx: &Ctx, root: &Path) -> Result<(String, bool), String> {
    let link = client::load_link();
    if !link.paired() {
        return Err("Pair this phone with your computer first.".into());
    }
    let list = client::fetch_list(&link)?;
    let data_dir = ctx.state.data_dir.clone();
    let mut ledger = load_ledger(&data_dir);
    let there = |rel: &str, size: u64| plan::safe_rel(root, rel).and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| m.len() == size);
    let p = plan::plan(&list, &ledger, there);
    if let Some(free) = free_space(root) {
        let need = p.bytes.saturating_add(SPARE);
        if free < need {
            return Err(format!("Not enough space on the phone: the new songs need {}, and {} is free. Keep fewer songs on the phone, or make room.", gb(need), gb(free)));
        }
    }
    {
        let mut s = STATUS.lock();
        s.phase = "copying";
        s.total = p.get.len();
        s.bytes_total = p.bytes;
    }
    report(ctx);
    let copied = copy_all(ctx, &link, root, &p.get, &mut ledger)?;
    if STOP.load(Ordering::SeqCst) {
        save_ledger(&data_dir, &ledger);
        return Ok((format!("Stopped after {}.", songs(copied.done)), copied.done > 0));
    }
    STATUS.lock().phase = "removing";
    report(ctx);
    let removed = remove_all(root, &p.drop, &mut ledger);
    let mut words = summary(copied.done, removed, list.pending, copied.failed);
    if let Some(e) = copied.last_error.filter(|_| copied.failed > 0) {
        words = format!("{words} {e}");
    }
    ledger.last_sync = super::now();
    ledger.last_result = words.clone();
    save_ledger(&data_dir, &ledger);
    Ok((words, copied.done + removed > 0))
}

struct Copied {
    done: usize,
    failed: usize,
    last_error: Option<String>,
}

fn copy_all(ctx: &Ctx, link: &Link, root: &Path, get: &[Entry], ledger: &mut Ledger) -> Result<Copied, String> {
    let http = client::download_client()?;
    let mut out = Copied { done: 0, failed: 0, last_error: None };
    let mut in_a_row = 0;
    let mut last_report = Instant::now();
    for e in get {
        while PAUSED.load(Ordering::SeqCst) && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(300));
        }
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        STATUS.lock().current = e.title.clone();
        report(ctx);
        let mut progress = |n: u64| {
            STATUS.lock().bytes_done += n;
            if last_report.elapsed() >= Duration::from_millis(250) {
                last_report = Instant::now();
                report(ctx);
            }
        };
        match copy_one(&http, link, root, e, &mut progress) {
            Ok(held) => {
                ledger.files.insert(e.key.clone(), held);
                // After every song: a sync cut short keeps what it copied.
                save_ledger(&ctx.state.data_dir, ledger);
                out.done += 1;
                in_a_row = 0;
            }
            Err(msg) if STOP.load(Ordering::SeqCst) => {
                log::info!("phone sync stopped during {}: {msg}", e.title);
                break;
            }
            Err(msg) => {
                log::warn!("phone sync: {}: {msg}", e.title);
                out.failed += 1;
                out.last_error = Some(msg.clone());
                in_a_row += 1;
                if in_a_row >= MAX_FAILS_IN_A_ROW {
                    return Err(format!("Stopped after {} because songs kept failing: {msg}", songs(out.done)));
                }
            }
        }
        STATUS.lock().done += 1;
    }
    Ok(out)
}

/// Copies one song; its ledger entry.
fn copy_one(http: &reqwest::blocking::Client, link: &Link, root: &Path, e: &Entry, progress: &mut dyn FnMut(u64)) -> Result<Held, String> {
    let wanted = plan::safe_rel(root, &e.path).ok_or_else(|| format!("“{}” has a name the phone can't use.", e.title))?;
    let target = match place_for(&wanted, e) {
        Place::Adopt(path) => return held(root, &path, e.size),
        Place::Write(path) => path,
    };
    client::download(http, link, e, &target, &STOP, progress)?;
    held(root, &target, e.size)
}

enum Place {
    /// The song is already there (byte for byte): take it as this phone's copy.
    Adopt(PathBuf),
    Write(PathBuf),
}

/// Where the song goes: its own name, unless another file has it.
fn place_for(wanted: &Path, e: &Entry) -> Place {
    let mut n = 1;
    let mut path = wanted.to_path_buf();
    while let Ok(m) = std::fs::metadata(&path) {
        if m.len() == e.size && super::keys::file_key(&path).is_ok_and(|k| k == e.key) {
            return Place::Adopt(path);
        }
        n += 1;
        path = plan::other_name(wanted, n);
    }
    Place::Write(path)
}

fn held(root: &Path, path: &Path, size: u64) -> Result<Held, String> {
    let rel = path.strip_prefix(root).map_err(|_| "A song landed outside the sync folder.".to_string())?;
    let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Ok(Held { path: rel.join("/"), size })
}

/// Removes this phone's copies of `keys`: a file the listener changed since (another size) is
/// left where it is, only forgotten. Empty folders left behind go too, up to the sync folder.
fn remove_all(root: &Path, keys: &[String], ledger: &mut Ledger) -> usize {
    let mut removed = 0;
    for key in keys {
        let Some(h) = ledger.files.remove(key) else { continue };
        let Some(path) = plan::safe_rel(root, &h.path) else { continue };
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == h.size) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
            remove_empty_parents(root, &path);
        }
    }
    removed
}

fn remove_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

fn songs(n: usize) -> String {
    format!("{n} {}", if n == 1 { "song" } else { "songs" })
}

fn gb(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{} MB", (b >> 20).max(1)) }
}

/// What a sync did, in a sentence.
fn summary(copied: usize, removed: usize, pending: usize, failed: usize) -> String {
    let mut parts = Vec::new();
    if copied > 0 {
        parts.push(format!("copied {}", songs(copied)));
    }
    if removed > 0 {
        parts.push(format!("removed {}", songs(removed)));
    }
    let mut s = if parts.is_empty() { "Up to date.".to_string() } else { format!("Done: {}.", parts.join(", ")) };
    if failed > 0 {
        s.push_str(&format!(" {} didn't come through and will be tried again.", songs(failed)));
    }
    if pending > 0 {
        let verb = if pending == 1 { "is" } else { "are" };
        s.push_str(&format!(" {} {verb} still being prepared on the computer; sync again later.", songs(pending)));
    }
    s
}

/// Free bytes where `root` is (or its nearest existing parent); None where it can't be told.
#[cfg(any(target_os = "android", target_os = "linux"))]
fn free_space(root: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut at = root;
    while !at.exists() {
        at = at.parent()?;
    }
    let c = std::ffi::CString::new(at.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `s` a properly sized, writable struct.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    Some((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
fn free_space(_root: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, size: u64) -> Entry {
        Entry { key: key.into(), size, title: "One".into(), path: "A/B/01 One.flac".into(), ..Default::default() }
    }

    #[test]
    fn a_file_in_the_way_is_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let wanted = dir.path().join("01 One.flac");
        std::fs::write(&wanted, b"the listener's own file").unwrap();
        match place_for(&wanted, &entry("k", 4)) {
            Place::Write(p) => assert_eq!(p, dir.path().join("01 One (2).flac")),
            Place::Adopt(_) => panic!("not the song"),
        }
    }

    #[test]
    fn a_copy_already_there_is_taken_as_ours() {
        let dir = tempfile::tempdir().unwrap();
        let wanted = dir.path().join("01 One.flac");
        std::fs::write(&wanted, b"song bytes").unwrap();
        let key = crate::phone::keys::file_key(&wanted).unwrap();
        assert!(matches!(place_for(&wanted, &entry(&key, 10)), Place::Adopt(p) if p == wanted));
    }

    #[test]
    fn removing_takes_only_our_unchanged_copies_and_their_empty_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("A/B")).unwrap();
        std::fs::create_dir_all(root.join("C")).unwrap();
        std::fs::write(root.join("A/B/1.flac"), b"12345").unwrap();
        std::fs::write(root.join("C/2.flac"), b"changed by the listener").unwrap();
        std::fs::write(root.join("C/mine.flac"), b"not ours").unwrap();
        let mut ledger = Ledger::default();
        ledger.files.insert("k1".into(), Held { path: "A/B/1.flac".into(), size: 5 });
        ledger.files.insert("k2".into(), Held { path: "C/2.flac".into(), size: 3 });
        let n = remove_all(root, &["k1".into(), "k2".into()], &mut ledger);
        assert_eq!(n, 1);
        assert!(!root.join("A").exists(), "empty folders go");
        assert!(root.join("C/2.flac").exists() && root.join("C/mine.flac").exists());
        assert!(ledger.files.is_empty(), "both are forgotten");
    }

    #[test]
    fn a_held_path_is_written_with_slashes_below_the_root() {
        let root = Path::new("/m/Cadence");
        assert_eq!(held(root, &root.join("A").join("01 One.flac"), 3).unwrap().path, "A/01 One.flac");
        assert!(held(root, Path::new("/elsewhere/x.flac"), 3).is_err());
    }

    #[test]
    fn the_summary_says_what_happened() {
        assert_eq!(summary(0, 0, 0, 0), "Up to date.");
        assert_eq!(summary(3, 1, 0, 0), "Done: copied 3 songs, removed 1 song.");
        assert!(summary(1, 0, 2, 1).contains("1 song didn't come through") && summary(1, 0, 2, 1).contains("2 songs are still being prepared"));
    }
}
