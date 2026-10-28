//! Background jobs: library scanning and loudness analysis.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use rayon::prelude::*;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{Cmd, measure_album};
use crate::library;
use crate::state::AppState;

/// Files written in the last few seconds are probably still being copied: measuring them now
/// fails or gives wrong results, so they wait (Windows sets a copy's time when it finishes).
const SETTLE_SECS: u64 = 20;

fn settling(t: &crate::library::Track) -> bool {
    crate::history::now().saturating_sub(t.modified) < SETTLE_SECS
}

/// Runs the measurements again once files that were still being copied have settled.
fn retry_later(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(SETTLE_SECS + 5));
        start_analysis(app);
    });
}

#[derive(Clone, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// Threads for the long background jobs (loudness and sound analysis): a quarter of the cores, at
/// most 2, each in background mode. The music, the UI and other apps come first, and memory stays
/// modest (every song being analysed holds its decoded audio).
fn background_pool() -> rayon::ThreadPool {
    let threads = std::thread::available_parallelism()
        .map(|n| (n.get() / 4).clamp(1, 2))
        .unwrap_or(1);
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("cadence-background-{i}"))
        .start_handler(|_| background_priority())
        .build()
        .expect("pool")
}

/// Lowers the calling thread's CPU, disk and memory priority: Windows' background mode (plus
/// EcoQoS on Windows 11, which keeps it on efficient cores at low clock speeds), or a nice value
/// on Linux and Android.
fn background_priority() {
    #[cfg(windows)]
    unsafe {
        use std::ffi::c_void;
        #[repr(C)]
        struct PowerThrottling {
            version: u32,
            control_mask: u32,
            state_mask: u32,
        }
        const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
        const THREAD_POWER_THROTTLING: i32 = 3;
        const EXECUTION_SPEED: u32 = 1;
        unsafe extern "system" {
            fn GetCurrentThread() -> *mut c_void;
            fn SetThreadPriority(thread: *mut c_void, priority: i32) -> i32;
            fn SetThreadInformation(thread: *mut c_void, class: i32, info: *const c_void, size: u32) -> i32;
        }
        SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
        let eco = PowerThrottling {
            version: 1,
            control_mask: EXECUTION_SPEED,
            state_mask: EXECUTION_SPEED,
        };
        SetThreadInformation(
            GetCurrentThread(),
            THREAD_POWER_THROTTLING,
            &eco as *const PowerThrottling as *const c_void,
            size_of::<PowerThrottling>() as u32,
        );
    }
    // Linux keeps a nice value per thread; 0 is the calling thread.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// Set at startup: the folders start being watched once the first scan is done.
static WATCH_AFTER_SCAN: AtomicBool = AtomicBool::new(false);

/// Rescans, then watches the folders for changes (at startup).
pub fn start_scan_then_watch(app: AppHandle) {
    WATCH_AFTER_SCAN.store(true, Ordering::SeqCst);
    start_scan(app);
}

/// Rescans every folder in the background and tells the UI when the library changed.
pub fn start_scan(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    {
        let mut lib = state.library.lock();
        if lib.scanning {
            lib.rescan_pending = true;
            return;
        }
        lib.scanning = true;
    }
    let _ = app.emit("library-progress", Progress { done: 0, total: 0 });
    std::thread::spawn(move || {
        let folders = state.settings.lock().folders.clone();
        let previous = state.library.lock().snapshot.tracks.clone();
        let started = std::time::Instant::now();
        let mut tracks = library::scan(&folders, previous, |done, total| {
            let _ = app.emit("library-progress", Progress { done, total });
        });
        let albums = library::build_albums(&mut tracks, &state.art_dir());
        log::info!(
            "scanned {} tracks, {} albums in {:?}",
            tracks.len(),
            albums.len(),
            started.elapsed()
        );
        let again = {
            let mut lib = state.library.lock();
            // Loudness measured while this scan ran isn't in `previous`: keep it.
            for t in tracks.iter_mut().filter(|t| !t.gain_measured) {
                if let Some(old) = lib.by_id.get(&t.id).map(|&i| &lib.snapshot.tracks[i]) {
                    if old.gain_measured && old.size == t.size && old.modified == t.modified {
                        t.gain = old.gain;
                        t.gain_measured = true;
                    }
                }
            }
            lib.set_tracks(tracks, albums);
            lib.scanning = false;
            std::mem::take(&mut lib.rescan_pending)
        };
        state.save_library();
        let _ = app.emit("library-changed", ());
        if WATCH_AFTER_SCAN.swap(false, Ordering::SeqCst) {
            watch_folders(app.clone());
        }
        if again {
            start_scan(app);
        } else {
            start_analysis(app);
        }
    });
}

/// Measures the loudness of every album that has songs without ReplayGain data (EBU R128).
/// Albums are measured whole so album gain is right; several albums run in parallel.
pub fn start_analysis(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let mut waiting = false;
    let todo: Vec<(String, Vec<(String, String)>)> = {
        let mut lib = state.library.lock();
        if lib.analyzing {
            return;
        }
        let missing: Vec<_> = lib
            .snapshot
            .albums
            .iter()
            .filter_map(|a| {
                let tracks: Vec<(String, String)> = a
                    .track_ids
                    .iter()
                    .filter_map(|id| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]))
                    .map(|t| (t.id.clone(), t.path.clone()))
                    .collect();
                let needs = a.track_ids.iter().any(|id| {
                    lib.by_id.get(id).is_some_and(|&i| {
                        let t = &lib.snapshot.tracks[i];
                        // Measured once already (even if it failed): don't retry on every start.
                        !t.gain_measured && (t.gain.track.is_none() || t.gain.album.is_none())
                    })
                });
                let copying = a.track_ids.iter().any(|id| {
                    lib.by_id.get(id).is_some_and(|&i| settling(&lib.snapshot.tracks[i]))
                });
                if needs && copying {
                    waiting = true;
                }
                (needs && !copying).then(|| (a.id.clone(), tracks))
            })
            .collect();
        if missing.is_empty() {
            drop(lib);
            if waiting {
                retry_later(app.clone());
            }
            start_listening(app);
            return;
        }
        lib.analyzing = true;
        missing
    };
    if waiting {
        retry_later(app.clone());
    }
    std::thread::spawn(move || {
        let total: usize = todo.iter().map(|(_, t)| t.len()).sum();
        let _ = app.emit("analysis-progress", Progress { done: 0, total });
        let started = std::time::Instant::now();
        // The background threads only measure; this thread files the results, so they never hold
        // a lock the UI or playback is waiting for.
        let (tx, rx) = crossbeam_channel::unbounded();
        background_pool().spawn(move || {
            todo.into_par_iter().for_each_with(tx, |tx, (_, tracks)| {
                let paths: Vec<&Path> = tracks.iter().map(|(_, p)| Path::new(p.as_str())).collect();
                let measured = measure_album(&paths);
                let _ = tx.send((tracks, measured));
            });
        });
        let mut done = 0;
        for (tracks, (per_track, album)) in rx {
            let mut updates = Vec::new();
            {
                let mut lib = state.library.lock();
                for ((id, _), measured) in tracks.iter().zip(per_track) {
                    let Some(&i) = lib.by_id.get(id) else {
                        continue;
                    };
                    let t = &mut lib.snapshot.tracks[i];
                    // Tags from other tools win; Cadence fills in only what's missing.
                    if t.gain.track.is_none() {
                        t.gain.track = measured;
                    }
                    if t.gain.album.is_none() {
                        t.gain.album = album;
                    }
                    t.gain_measured = true;
                    updates.push((id.clone(), t.gain));
                }
            }
            state.engine.send(Cmd::UpdateGains(updates));
            done += tracks.len();
            let _ = app.emit("analysis-progress", Progress { done, total });
        }
        log::info!(
            "loudness analysis of {total} songs took {:?}",
            started.elapsed()
        );
        state.library.lock().analyzing = false;
        state.save_library();
        let _ = app.emit("analysis-progress", Progress { done: total, total });
        let _ = app.emit("library-changed", ());
        start_listening(app);
    });
}

/// Runs sound analysis (bliss) on every song that hasn't had it, a few at a time in the background.
/// Results are saved every half minute so a long first run isn't lost if Cadence closes.
pub fn start_listening(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let todo: Vec<(String, String, String)> = {
        let mut lib = state.library.lock();
        if lib.listening || lib.scanning {
            return;
        }
        let sonic = state.sonic.lock();
        let todo: Vec<_> = lib
            .snapshot
            .tracks
            .iter()
            .filter(|t| sonic.needs(t) && !settling(t))
            .map(|t| (t.id.clone(), t.path.clone(), crate::sonic::stamp(t)))
            .collect();
        if todo.is_empty() {
            return;
        }
        lib.listening = true;
        todo
    };
    std::thread::spawn(move || {
        let total = todo.len();
        let _ = app.emit("sonic-progress", Progress { done: 0, total });
        let started = std::time::Instant::now();
        // As in start_analysis: the background threads analyse, this thread files the results.
        let (tx, rx) = crossbeam_channel::unbounded();
        background_pool().spawn(move || {
            todo.into_par_iter().for_each_with(tx, |tx, (id, path, stamp)| {
                let (f, t) = crate::sonic::analyze(Path::new(&path)).unwrap_or_else(|e| {
                    log::warn!("sound analysis failed for {path}: {e:#}");
                    (Vec::new(), Vec::new())
                });
                let _ = tx.send((id, crate::sonic::Entry { stamp, f, t }));
            });
        });
        let mut saved = std::time::Instant::now();
        for (n, (id, entry)) in rx.into_iter().enumerate() {
            state.sonic.lock().tracks.insert(id, entry);
            if saved.elapsed() > Duration::from_secs(30) {
                state.save_sonic();
                saved = std::time::Instant::now();
            }
            let _ = app.emit("sonic-progress", Progress { done: n + 1, total });
        }
        log::info!("sound analysis of {total} songs took {:?}", started.elapsed());
        // Forget songs that are no longer in the library.
        {
            let lib = state.library.lock();
            state.sonic.lock().tracks.retain(|id, _| lib.by_id.contains_key(id));
        }
        state.save_sonic();
        state.library.lock().listening = false;
        let _ = app.emit("sonic-progress", Progress { done: total, total });
    });
}

pub type FolderWatcher = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

/// Watches every music folder; when audio files appear, change or disappear, the library rescans
/// (only changed files are re-read). Call again whenever the folder list changes.
pub fn watch_folders(app: AppHandle) {
    // Watching a folder tree adds a watch for every folder in it, which takes seconds on a phone's
    // shared storage: done in the background, so startup (and adding a folder) doesn't wait for it.
    // When calls overlap, the latest one's watcher is kept.
    static LATEST: AtomicUsize = AtomicUsize::new(0);
    let generation = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let watcher = make_watcher(&app);
        if LATEST.load(Ordering::SeqCst) == generation {
            let state = app.state::<Arc<AppState>>();
            *state.watcher.lock() = watcher;
            log::info!("watching the music folders (set up in {:?})", started.elapsed());
        }
    });
}

/// A watcher on every music folder (None without folders), rescanning when they change.
fn make_watcher(app: &AppHandle) -> Option<FolderWatcher> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let folders = state.settings.lock().folders.clone();
    // Drop the old watcher first so removed folders stop being watched.
    *state.watcher.lock() = None;
    if folders.is_empty() {
        return None;
    }
    let handle = app.clone();
    let debouncer = new_debouncer(
        Duration::from_secs(3),
        None,
        move |result: DebounceEventResult| {
            let Ok(events) = result else { return };
            // Only real changes count. Linux and Android also report files being *read*, which
            // would make every scan (it reads the files) trigger the next one, forever.
            let relevant = events.iter().filter(|e| {
                matches!(
                    e.kind,
                    notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
                ) && !matches!(e.kind, notify::EventKind::Modify(notify::event::ModifyKind::Metadata(_)))
            }).any(|e| {
                e.paths
                    .iter()
                    .any(|p| p.is_dir() || crate::library::is_audio(p) || p.extension().is_none())
            });
            if relevant {
                log::info!("music folders changed; rescanning");
                start_scan(handle.clone());
            }
        },
    );
    match debouncer {
        Ok(mut d) => {
            for f in &folders {
                if let Err(e) = d.watch(Path::new(f), RecursiveMode::Recursive) {
                    log::warn!("can't watch {f}: {e}");
                }
            }
            Some(d)
        }
        Err(e) => {
            log::warn!("folder watching unavailable: {e}");
            None
        }
    }
}
