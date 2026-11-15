//! Background jobs: library scanning and loudness analysis.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use rayon::prelude::*;
use serde::Serialize;
use crate::ctx::Ctx;

use crate::audio::Cmd;
use crate::library;

/// Files written in the last few seconds are probably still being copied: measuring them now
/// fails or gives wrong results, so they wait (Windows sets a copy's time when it finishes).
const SETTLE_SECS: u64 = 20;

pub(crate) fn settling(t: &crate::library::Track) -> bool {
    crate::history::now().saturating_sub(t.modified) < SETTLE_SECS
}

/// Runs the measurements again once files that were still being copied have settled.
fn retry_later(ctx: Ctx) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(SETTLE_SECS + 5));
        start_analysis(ctx);
    });
}

#[derive(Clone, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// The threads for the heavy background jobs (loudness, sound analysis, library health, waveforms
/// for all songs): ONE pool for all of them, made the first time it's needed —
/// a quarter of the cores, at most 2, each in background mode — so however many of those jobs run
/// at once, no more than two heavy threads do. The music, the UI and other apps come first, and
/// memory stays modest (every song being analysed holds its decoded audio). A job never waits for
/// another job inside the pool (that would hold a thread the other needs): it waits on its own
/// thread, then hands the work over. (Light, bounded work that must not wait hours behind a long
/// job has a low-priority thread of its own: the lyrics index, the waveform of the song playing.)
pub(crate) fn background_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 4).clamp(1, 2))
            .unwrap_or(1);
        let built = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("cadence-background-{i}"))
            .start_handler(|_| background_priority())
            .build();
        match built {
            Ok(p) => p,
            Err(e) => {
                // (No thread could be made: one more try with a single one; that failing too is
                // a system out of threads, nothing Cadence can carry on without.)
                log::error!("no background threads: {e}");
                rayon::ThreadPoolBuilder::new().num_threads(1).start_handler(|_| background_priority()).build().expect("a background thread")
            }
        }
    })
}

#[cfg(test)]
mod pool_tests {
    #[test]
    fn one_shared_pool_of_at_most_two_threads() {
        let a = super::background_pool() as *const rayon::ThreadPool;
        let b = super::background_pool() as *const rayon::ThreadPool;
        assert_eq!(a, b, "every job gets the same pool");
        assert!((1..=2).contains(&super::background_pool().current_num_threads()));
    }
}

/// Lowers the calling thread's CPU and memory priority: the lowest thread priority, low memory
/// priority and EcoQoS on Windows (Windows 11 keeps it on efficient cores at low clock speeds), or
/// a nice value on Linux and Android.
///
/// Not Windows' "background mode" (THREAD_MODE_BACKGROUND_BEGIN): besides the CPU it puts every
/// disk read and write of the thread at Very Low I/O priority, which Windows serves only when no
/// other program is using the disk (a build, a download, a virus scan of new files), with a trickle
/// of a few I/Os a second to stop it starving completely. A job over small files is all I/O: the
/// loudness measuring of a 20,000-song library of tiny files went at 2 to 7 songs a second at 1 %
/// of a core for minutes, and at 110 a second in a quiet spell. The CPU priority already keeps the
/// job out of the way of the music and the UI; its reads are small next to what the disk can do.
pub(crate) fn background_priority() {
    #[cfg(windows)]
    unsafe {
        use std::ffi::c_void;
        #[repr(C)]
        struct PowerThrottling {
            version: u32,
            control_mask: u32,
            state_mask: u32,
        }
        const THREAD_PRIORITY_LOWEST: i32 = -2;
        const THREAD_MEMORY_PRIORITY: i32 = 0;
        const MEMORY_PRIORITY_LOW: u32 = 2;
        const THREAD_POWER_THROTTLING: i32 = 3;
        const EXECUTION_SPEED: u32 = 1;
        unsafe extern "system" {
            fn GetCurrentThread() -> *mut c_void;
            fn SetThreadPriority(thread: *mut c_void, priority: i32) -> i32;
            fn SetThreadInformation(thread: *mut c_void, class: i32, info: *const c_void, size: u32) -> i32;
        }
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
        // (Its pages are the first to go when memory is short: a decoded song is dropped anyway.)
        let memory = MEMORY_PRIORITY_LOW;
        SetThreadInformation(GetCurrentThread(), THREAD_MEMORY_PRIORITY, &memory as *const u32 as *const c_void, size_of::<u32>() as u32);
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
pub fn start_scan_then_watch(ctx: Ctx) {
    WATCH_AFTER_SCAN.store(true, Ordering::SeqCst);
    start_scan(ctx);
}

/// Rescans every folder in the background and tells the UI when the library changed.
pub fn start_scan(ctx: Ctx) {
    let state = ctx.state.clone();
    {
        let mut lib = state.library.lock();
        if lib.scanning {
            lib.rescan_pending = true;
            return;
        }
        lib.scanning = true;
    }
    ctx.emit("library-progress", Progress { done: 0, total: 0 });
    std::thread::spawn(move || {
        let folders = state.settings.lock().folders.clone();
        let previous = state.library.lock().snapshot.tracks.clone();
        // The queue's songs as they are now, to find again if their files move.
        let queued: Vec<library::Track> = {
            let ids: std::collections::HashSet<String> = state.engine.status().queue.into_iter().collect();
            previous.iter().filter(|t| ids.contains(&t.id)).cloned().collect()
        };
        // The same for the playlists' songs.
        let listed: Vec<library::Track> = {
            let ids: std::collections::HashSet<String> =
                state.playlists.lock().items.iter().flat_map(|p| p.track_ids.iter().cloned()).collect();
            previous.iter().filter(|t| ids.contains(&t.id)).cloned().collect()
        };
        let started = std::time::Instant::now();
        // Progress at most 5 times a second (it came every 25 files: ~280 sidebar updates a scan).
        let last_progress = std::sync::atomic::AtomicU64::new(0);
        let mut tracks = library::scan(&folders, previous, |done, total| {
            let now = started.elapsed().as_millis() as u64;
            let last = last_progress.load(Ordering::Relaxed);
            if done == total || now >= last + 200 && last_progress.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                ctx.emit("library-progress", Progress { done, total });
            }
        });
        let read = started.elapsed();
        crate::tagedit::show_held(&state.data_dir, &mut tracks);
        let albums = library::build_albums(&mut tracks, &state.art_dir());
        log::info!(
            "scanned {} tracks, {} albums in {:?} (files {:?}, albums {:?})",
            tracks.len(),
            albums.len(),
            started.elapsed(),
            read,
            started.elapsed() - read
        );
        let unchanged;
        let again = {
            let mut lib = state.library.lock();
            // Loudness measured while this scan ran isn't in `previous`: keep it.
            for t in tracks.iter_mut().filter(|t| !t.gain_measured) {
                if let Some(old) = lib.by_id.get(&t.id).map(|&i| &lib.snapshot.tracks[i])
                    && old.gain_measured && old.size == t.size && old.modified == t.modified {
                        t.gain = old.gain;
                        t.gain_measured = true;
                    }
            }
            // Nothing changed (a rescan found the same songs): the window keeps what it has instead
            // of reloading the whole library (4 MB for 7,000 songs) and redrawing every list.
            let fingerprint = |s: &library::Snapshot| serde_json::to_vec(s).map(|b| crate::library::short_hash_bytes(&b)).ok();
            let new = library::Snapshot { tracks, albums };
            unchanged = fingerprint(&new).is_some_and(|f| fingerprint(&lib.snapshot) == Some(f));
            lib.set_tracks(new.tracks, new.albums);
            lib.scanning = false;
            std::mem::take(&mut lib.rescan_pending)
        };
        // The artists of the new library (the moved songs below are matched by them).
        crate::identity::rebuild_now(&ctx);
        // The queue, the playlists and the favourites and ratings follow songs whose files moved
        // (only where it's certain which song is theirs now; the others keep their old ids).
        let followed = state.follow_moved(&queued, &listed);
        // Songs of a restored backup not found yet (nothing to do without any).
        crate::backup::retry_pending(&ctx);
        if followed.queue > 0 {
            log::info!("{} queued songs moved; the queue follows them", followed.queue);
        }
        if followed.playlists > 0 {
            log::info!("{} playlist entries moved; the playlists follow them", followed.playlists);
            ctx.emit("playlists-changed", ());
        }
        if followed.marks > 0 {
            log::info!("{} rated or favourite songs moved; their marks follow them", followed.marks);
        }
        let (found, relinked) = (followed.queue, followed.marks);
        if unchanged && found == 0 && relinked == 0 && followed.playlists == 0 {
            log::info!("rescan found no changes");
            ctx.emit("library-scanned", ());
        } else {
            state.save_library();
            ctx.emit("library-changed", ());
            // Waveforms of songs no longer in the library.
            crate::waveform::prune_later(&ctx);
        }
        if WATCH_AFTER_SCAN.swap(false, Ordering::SeqCst) {
            watch_folders(ctx.clone());
        }
        if again {
            start_scan(ctx);
        } else {
            // New artist names in Japanese or Chinese letters: is the artist also there under a Latin one?
            if state.settings.lock().fix_tags_online {
                crate::libfix::artist_check_auto(&ctx);
            }
            // Names MusicBrainz hasn't been asked about (it waits for the jobs above).
            crate::musicbrainz::auto(&ctx);
            start_analysis(ctx);
        }
    });
}

/// Measures the loudness of every album that has songs without ReplayGain data (EBU R128).
/// Albums are measured whole so album gain is right; several albums run in parallel.
pub fn start_analysis(ctx: Ctx) {
    let state = ctx.state.clone();
    let mut waiting = false;
    // Songs whose seek-bar waveform can be drawn from the same decode (`waveform`). Not CUE-sheet
    // songs: their path is the whole album's file; they're drawn span by span when needed.
    let waves: std::collections::HashMap<String, crate::waveform::Stamp>;
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
                retry_later(ctx.clone());
            }
            start_listening(ctx);
            return;
        }
        lib.analyzing = true;
        waves = missing
            .iter()
            .flat_map(|(_, tracks)| tracks)
            .filter_map(|(id, _)| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]))
            .filter(|t| t.cue.is_none())
            .map(|t| (t.id.clone(), crate::waveform::Stamp::of(t)))
            .collect();
        missing
    };
    if waiting {
        retry_later(ctx.clone());
    }
    std::thread::spawn(move || {
        let total: usize = todo.iter().map(|(_, t)| t.len()).sum();
        ctx.emit("analysis-progress", Progress { done: 0, total });
        let started = std::time::Instant::now();
        // The background threads only measure; this thread files the results, so they never hold
        // a lock the UI or playback is waiting for.
        let (tx, rx) = crossbeam_channel::unbounded();
        let data_dir = state.data_dir.clone();
        background_pool().spawn(move || {
            todo.into_par_iter().for_each_with(tx, |tx, (_, tracks)| {
                let paths: Vec<&Path> = tracks.iter().map(|(_, p)| Path::new(p.as_str())).collect();
                let want: Vec<bool> = tracks
                    .iter()
                    .map(|(id, _)| waves.get(id).is_some_and(|&s| !crate::waveform::has(&data_dir, id, s)))
                    .collect();
                let (per_track, album, envelopes) = crate::audio::loudness::measure_album_with(&paths, &want);
                for ((id, _), e) in tracks.iter().zip(&envelopes) {
                    if let (Some(e), Some(&s)) = (e, waves.get(id))
                        && let Err(err) = crate::waveform::save(&data_dir, id, s, e)
                    {
                        log::warn!("can't save the waveform of {id}: {err}");
                    }
                }
                let _ = tx.send((tracks, (per_track, album)));
            });
        });
        let mut done = 0;
        // Saved every half minute too (as sound analysis is): quitting halfway through a big
        // library keeps what was measured.
        let mut saved = std::time::Instant::now();
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
            if saved.elapsed() > Duration::from_secs(30) {
                state.save_library();
                saved = std::time::Instant::now();
            }
            ctx.emit("analysis-progress", Progress { done, total });
        }
        log::info!(
            "loudness analysis of {total} songs took {:?}",
            started.elapsed()
        );
        state.library.lock().analyzing = false;
        state.save_library();
        ctx.emit("analysis-progress", Progress { done: total, total });
        ctx.emit("library-changed", ());
        start_listening(ctx);
    });
}

/// Runs sound analysis (bliss) on every song that hasn't had it, a few at a time in the background.
/// Results are saved every half minute so a long first run isn't lost if Cadence closes.
pub fn start_listening(ctx: Ctx) {
    let state = ctx.state.clone();
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
        ctx.emit("sonic-progress", Progress { done: 0, total });
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
            ctx.emit("sonic-progress", Progress { done: n + 1, total });
        }
        log::info!("sound analysis of {total} songs took {:?}", started.elapsed());
        // Forget songs that are no longer in the library.
        {
            let lib = state.library.lock();
            state.sonic.lock().tracks.retain(|id, _| lib.by_id.contains_key(id));
        }
        state.save_sonic();
        state.library.lock().listening = false;
        ctx.emit("sonic-progress", Progress { done: total, total });
    });
}

pub type FolderWatcher = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

/// Watches every music folder; when audio files appear, change or disappear, the library rescans
/// (only changed files are re-read). Call again whenever the folder list changes.
pub fn watch_folders(ctx: Ctx) {
    // Watching a folder tree adds a watch for every folder in it, which takes seconds on a phone's
    // shared storage: done in the background, so startup (and adding a folder) doesn't wait for it.
    // When calls overlap, the latest one's watcher is kept.
    static LATEST: AtomicUsize = AtomicUsize::new(0);
    let generation = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let watcher = make_watcher(&ctx);
        if LATEST.load(Ordering::SeqCst) == generation {
            let state = &ctx.state;
            *state.watcher.lock() = watcher;
            log::info!("watching the music folders (set up in {:?})", started.elapsed());
        }
    });
}

/// Watched rescans come at most this often: a download finishing a song every few seconds would
/// otherwise rescan (and reload the window's library) every few seconds.
const WATCH_RESCAN_GAP: Duration = Duration::from_secs(15);
static WATCH_RESCAN_WAITING: AtomicBool = AtomicBool::new(false);
static LAST_WATCH_RESCAN: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// The music folders changed: rescan now, or once the gap since the last watched rescan is over
/// (one rescan then covers every change in between).
fn scan_for_changes(ctx: Ctx) {
    if WATCH_RESCAN_WAITING.swap(true, Ordering::SeqCst) {
        return; // A rescan is already waiting; it will see this change too.
    }
    std::thread::spawn(move || {
        let wait = LAST_WATCH_RESCAN
            .lock()
            .unwrap()
            .map_or(Duration::ZERO, |t| WATCH_RESCAN_GAP.saturating_sub(t.elapsed()));
        if !wait.is_zero() {
            log::info!("music folders changed; rescanning in {:.0} s", wait.as_secs_f32());
            std::thread::sleep(wait);
        } else {
            log::info!("music folders changed; rescanning");
        }
        *LAST_WATCH_RESCAN.lock().unwrap() = Some(std::time::Instant::now());
        WATCH_RESCAN_WAITING.store(false, Ordering::SeqCst);
        start_scan(ctx);
    });
}

/// A watcher on every music folder (None without folders), rescanning when they change.
fn make_watcher(ctx: &Ctx) -> Option<FolderWatcher> {
    let state = ctx.state.clone();
    let folders = state.settings.lock().folders.clone();
    // Drop the old watcher first so removed folders stop being watched.
    *state.watcher.lock() = None;
    if folders.is_empty() {
        return None;
    }
    let handle = ctx.clone();
    let roots: Vec<std::path::PathBuf> = folders.iter().map(std::path::PathBuf::from).collect();
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
                e.paths.iter().any(|p| {
                    // A download finished (aria2 deletes its control file): its songs are complete now.
                    // (Saving its progress, aria2 also deletes it and renames a new one into place,
                    // so it only counts when it's still gone once the events have settled.)
                    if crate::library::is_download_mark(p) {
                        return matches!(e.kind, notify::EventKind::Remove(_)) && !p.exists();
                    }
                    let music = p.is_dir() || crate::library::is_audio(p) || crate::library::is_cue(p) || p.extension().is_none();
                    // A download in progress writes its files every minute: not a change to rescan for.
                    let root = roots.iter().find(|r| p.starts_with(r));
                    music && !root.is_some_and(|r| crate::library::downloading_now(p, r))
                })
            });
            if relevant {
                scan_for_changes(handle.clone());
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

#[cfg(test)]
mod analysis_tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::library::{Album, Track};

    struct Quiet;
    impl crate::ctx::Host for Quiet {
        fn emit(&self, _: &str, _: serde_json::Value) {}
    }

    /// Waits up to `limit` for `done`; false if it never came.
    fn within(limit: Duration, done: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < limit {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    /// The whole chain on small files (loudness with waveforms, then sound analysis), with
    /// "Prepare waveforms" asked for and a scan starting and ending meanwhile: every job finishes,
    /// none waits for another forever, and none crawls (60 half-second songs decode in well under
    /// a second of work; the bound is generous for slow CI machines).
    #[test]
    fn small_songs_are_all_measured_quickly_with_other_jobs_beside() {
        let base = tempfile::tempdir().unwrap();
        let (ctx, _status) = crate::hub::open(base.path().join("data"), &base.path().join("local"), Arc::new(Quiet));
        let music = base.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let settled = crate::history::now().saturating_sub(3600);
        let (mut tracks, mut albums) = (Vec::new(), Vec::new());
        for a in 0..6 {
            let mut ids = Vec::new();
            for n in 0..10 {
                let path = music.join(format!("{a}-{n}.wav"));
                crate::audio::envelope::test_wav(&path, 22_050);
                let id = format!("s{a}x{n}");
                ids.push(id.clone());
                tracks.push(Track {
                    id,
                    path: path.to_string_lossy().into_owned(),
                    title: format!("Song {n}"),
                    album_id: format!("al{a}"),
                    duration: 0.5,
                    size: std::fs::metadata(&path).unwrap().len(),
                    modified: settled,
                    ..Default::default()
                });
            }
            albums.push(Album { id: format!("al{a}"), track_ids: ids, ..Default::default() });
        }
        ctx.state.library.lock().set_tracks(tracks, albums);

        super::start_analysis(ctx.clone());
        assert!(crate::waveform::prepare_all(&ctx), "asked while the loudness job runs");
        ctx.state.library.lock().scanning = true;
        std::thread::sleep(Duration::from_millis(100));
        ctx.state.library.lock().scanning = false;

        let state = ctx.state.clone();
        let measured = || {
            let lib = state.library.lock();
            !lib.analyzing && lib.snapshot.tracks.iter().all(|t| t.gain_measured)
        };
        assert!(within(Duration::from_secs(60), measured), "loudness of 60 small songs");
        let listened = || state.sonic.lock().tracks.len() == 60 && !state.library.lock().listening;
        assert!(within(Duration::from_secs(120), listened), "sound analysis after it");
        assert!(within(Duration::from_secs(60), || !crate::waveform::status().running), "Prepare waveforms after them");
        let drawn = std::fs::read_dir(state.data_dir.join(crate::waveform::DIR)).unwrap().count();
        assert_eq!(drawn, 60, "every song's waveform, drawn in the loudness pass");
    }
}
