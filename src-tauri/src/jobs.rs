//! Background jobs: library scanning and loudness analysis.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
        let done = AtomicUsize::new(0);
        let _ = app.emit("analysis-progress", Progress { done: 0, total });
        let started = std::time::Instant::now();
        // Leave a core free so playback and the UI stay smooth.
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).max(1))
            .unwrap_or(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool");
        pool.install(|| {
            todo.par_iter().for_each(|(_, tracks)| {
                let paths: Vec<&Path> = tracks.iter().map(|(_, p)| Path::new(p.as_str())).collect();
                let (per_track, album) = measure_album(&paths);
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
                let n = done.fetch_add(tracks.len(), Ordering::Relaxed) + tracks.len();
                let _ = app.emit("analysis-progress", Progress { done: n, total });
            });
        });
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
/// Results are saved now and then so a long first run isn't lost if Cadence closes.
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
        let done = AtomicUsize::new(0);
        let _ = app.emit("sonic-progress", Progress { done: 0, total });
        let started = std::time::Instant::now();
        // Half the cores: this is a long, low-priority job and each song needs ~30 MB while it runs.
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).max(1))
            .unwrap_or(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool");
        pool.install(|| {
            todo.par_iter().for_each(|(id, path, stamp)| {
                let (f, t) = crate::sonic::analyze(Path::new(path)).unwrap_or_else(|e| {
                    log::warn!("sound analysis failed for {path}: {e:#}");
                    (Vec::new(), Vec::new())
                });
                state.sonic.lock().tracks.insert(
                    id.clone(),
                    crate::sonic::Entry {
                        stamp: stamp.clone(),
                        f,
                        t,
                    },
                );
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 25 == 0 {
                    state.save_sonic();
                }
                let _ = app.emit("sonic-progress", Progress { done: n, total });
            });
        });
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
    let state = app.state::<Arc<AppState>>().inner().clone();
    let folders = state.settings.lock().folders.clone();
    // Drop the old watcher first so removed folders stop being watched.
    *state.watcher.lock() = None;
    if folders.is_empty() {
        return;
    }
    let handle = app.clone();
    let debouncer = new_debouncer(
        Duration::from_secs(3),
        None,
        move |result: DebounceEventResult| {
            let Ok(events) = result else { return };
            let relevant = events.iter().any(|e| {
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
            *state.watcher.lock() = Some(d);
        }
        Err(e) => log::warn!("folder watching unavailable: {e}"),
    }
}
