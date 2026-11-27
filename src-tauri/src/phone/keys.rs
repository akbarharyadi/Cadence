//! Song keys: a fingerprint of each file's bytes (blake3), the same for a byte-identical copy
//! anywhere. They tie the phone's copy of a song to the computer's song without trusting names or
//! paths (a song's id is a hash of its path, so it differs on the phone).
//!
//! Worked out only for songs going to the phone, by a background job (`prepare`) on the shared
//! pool, and kept in `phone-keys.json` by path with the file's size and modified time: a cache,
//! rebuilt when lost (not in backups), and pruned to the library's files when saved.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::library::Track;

const FILE: &str = "phone-keys.json";
/// Hex characters of the blake3 hash kept (128 bits).
const KEY_LEN: usize = 32;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Stamp {
    size: u64,
    modified: u64,
    key: String,
}

/// path → the file's key as of its size and modified time.
type Cache = HashMap<String, Stamp>;

/// Loaded on first use.
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn with_cache<R>(data_dir: &Path, f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut c = CACHE.lock();
    let cache = c.get_or_insert_with(|| crate::state::read_json(&data_dir.join(FILE)));
    f(cache)
}

/// The key of a song whose file hasn't changed since it was worked out.
fn cached(cache: &Cache, t: &Track) -> Option<String> {
    cache.get(&t.path).filter(|s| s.size == t.size && s.modified == t.modified).map(|s| s.key.clone())
}

/// The keys known for these songs (id → key); songs without one are left out.
pub fn known(data_dir: &Path, tracks: &[&Track]) -> HashMap<String, String> {
    with_cache(data_dir, |cache| tracks.iter().filter_map(|t| Some((t.id.clone(), cached(cache, t)?))).collect())
}

/// The key of a file: blake3 of all its bytes, read in pieces (a song can be 100 MB).
pub fn file_key(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok(h.finalize().to_hex().get(..KEY_LEN).unwrap_or_default().to_string())
}

// ---------- the job ----------

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
    ctx.emit("phone-keys-progress", status());
}

pub fn pause(ctx: &Ctx, paused: bool) {
    PAUSED.store(paused, Ordering::SeqCst);
    report(ctx);
}

/// What one song needs: where it is and its stamp.
struct Todo {
    path: String,
    size: u64,
    modified: u64,
}

/// Works out the keys these songs (by id) don't have yet, one file at a time on the shared
/// background pool, waiting while paused or while the library is busy. False: it's running already
/// (it picks up the rest next time).
pub fn prepare(ctx: &Ctx, ids: &[String]) -> bool {
    let todo: Vec<Todo> = {
        let lib = ctx.state.library.lock();
        let want: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let tracks: Vec<&Track> = lib.snapshot.tracks.iter().filter(|t| want.contains(t.id.as_str())).collect();
        with_cache(&ctx.state.data_dir, |cache| {
            tracks.iter().filter(|t| cached(cache, t).is_none()).map(|t| Todo { path: t.path.clone(), size: t.size, modified: t.modified }).collect()
        })
    };
    if todo.is_empty() || RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    STOP.store(false, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    DONE.store(0, Ordering::SeqCst);
    TOTAL.store(todo.len(), Ordering::SeqCst);
    report(ctx);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        crate::jobs::background_priority();
        run(&ctx, &todo);
        save(&ctx);
        RUNNING.store(false, Ordering::SeqCst);
        PAUSED.store(false, Ordering::SeqCst);
        report(&ctx);
    });
    true
}

fn run(ctx: &Ctx, todo: &[Todo]) {
    let state = &ctx.state;
    let busy = || {
        let lib = state.library.lock();
        lib.analyzing || lib.listening || lib.scanning
    };
    let mut last = std::time::Instant::now();
    for t in todo {
        // Waiting happens here, never inside the pool (it would hold a thread other jobs need).
        while !STOP.load(Ordering::SeqCst) && (PAUSED.load(Ordering::SeqCst) || busy()) {
            std::thread::sleep(Duration::from_millis(500));
        }
        if STOP.load(Ordering::SeqCst) {
            return;
        }
        let key = crate::jobs::background_pool().install(|| {
            crate::jobs::take_turn();
            file_key(Path::new(&t.path))
        });
        match key {
            Ok(key) => with_cache(&state.data_dir, |c| {
                c.insert(t.path.clone(), Stamp { size: t.size, modified: t.modified, key });
            }),
            Err(e) => log::warn!("no song key for {}: {e}", t.path),
        }
        DONE.fetch_add(1, Ordering::SeqCst);
        if last.elapsed() >= Duration::from_millis(200) {
            last = std::time::Instant::now();
            report(ctx);
        }
    }
}

/// Saves the cache, keeping only the library's files (so it can't grow without bound).
fn save(ctx: &Ctx) {
    let paths: std::collections::HashSet<String> = ctx.state.library.lock().snapshot.tracks.iter().map(|t| t.path.clone()).collect();
    let kept: Cache = with_cache(&ctx.state.data_dir, |c| {
        c.retain(|p, _| paths.contains(p));
        c.clone()
    });
    // A cache: no `.bak` of it.
    crate::state::write_json_with(&ctx.state.data_dir.join(FILE), &kept, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_bytes_give_the_same_key_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = (dir.path().join("a.flac"), dir.path().join("sub b.flac"), dir.path().join("c.flac"));
        std::fs::write(&a, b"fLaC same bytes").unwrap();
        std::fs::write(&b, b"fLaC same bytes").unwrap();
        std::fs::write(&c, b"fLaC other bytes").unwrap();
        let ka = file_key(&a).unwrap();
        assert_eq!(ka.len(), KEY_LEN);
        assert_eq!(ka, file_key(&b).unwrap());
        assert_ne!(ka, file_key(&c).unwrap());
    }

    #[test]
    fn a_key_is_only_trusted_while_the_file_is_unchanged() {
        let mut cache = Cache::new();
        cache.insert("C:/m/a.flac".into(), Stamp { size: 10, modified: 5, key: "k".into() });
        let t = Track { path: "C:/m/a.flac".into(), size: 10, modified: 5, ..Default::default() };
        assert_eq!(cached(&cache, &t).as_deref(), Some("k"));
        let changed = Track { modified: 6, ..t };
        assert_eq!(cached(&cache, &changed), None);
    }

    #[test]
    fn a_missing_file_has_no_key() {
        assert!(file_key(Path::new("Z:/no/such/file.flac")).is_err());
    }
}
