//! Starting Cadence's core and passing the engine's status on. Shared by the Tauri app (lib.rs)
//! and the native Windows app (ffi.rs), so both get the same history, scrobbling, saved session,
//! media overlay, folder scanning and background analysis.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Receiver;
use parking_lot::Mutex;

use crate::audio::{Engine, Status};
use crate::ctx::{Ctx, Host};
use crate::history::{History, Tracker};
use crate::library::{self, LibraryCache};
use crate::state::{AppState, Library, Settings, read_json};
use crate::{autotag, history, jobs, media, scrobble, voice};

/// Reads the saved data, starts the engine and puts the last session back (paused).
pub fn open(data_dir: PathBuf, local_dir: &Path, host: Arc<dyn Host>) -> (Ctx, Receiver<Status>) {
    let started = std::time::Instant::now();
    let _ = std::fs::create_dir_all(data_dir.join("art"));
    let settings: Settings = read_json(&data_dir.join("settings.json"));
    let cache: LibraryCache = read_json(&data_dir.join("library.json"));
    let history: History = read_json(&data_dir.join("history.json"));
    let playlists: crate::playlists::Playlists = read_json(&data_dir.join("playlists.json"));
    let sonic = read_json::<crate::sonic::Store>(&data_dir.join("sonic.json")).checked();
    // Song info edited while its song played, if Cadence closed before it ended
    // (written now, before the last session's song is opened again).
    crate::tagedit::write_leftovers(&data_dir);

    let (status_tx, status_rx) = crossbeam_channel::unbounded::<Status>();
    let engine = Engine::start(settings.volume, settings.dsp.clone(), settings.repeat, move |s| {
        let _ = status_tx.send(s.clone());
    });

    let read = started.elapsed();
    // Show the cached library immediately; a background rescan picks up changes.
    let mut tracks = cache.tracks;
    let albums = library::build_albums(&mut tracks, &data_dir.join("art"));
    let built = started.elapsed();
    let mut lib = Library::default();
    lib.set_tracks(tracks, albums);
    let state = Arc::new(AppState {
        engine,
        history: Mutex::new(history),
        playlists: Mutex::new(playlists),
        library: Mutex::new(lib),
        sonic: Mutex::new(sonic),
        settings: Mutex::new(settings),
        data_dir,
        watcher: Mutex::new(None),
    });
    let voice = Arc::new(voice::Voice::new(&state.data_dir, local_dir));
    // Carry on where the last session left off (paused).
    state.restore_session();
    log::info!("core opened in {:?} (saved data read by {read:?}, albums built by {built:?})", started.elapsed());
    (Ctx::new(state, voice, host), status_rx)
}

/// Starts the status thread: every engine status goes to the front end, the listening history,
/// ListenBrainz, the saved session and the Windows media overlay (tied to window `hwnd`).
/// `on_song` gets "Title — Artist" when the song changes (the tray's tooltip).
pub fn start_status_hub(ctx: Ctx, rx: Receiver<Status>, hwnd: isize, on_song: impl Fn(Option<String>) + Send + 'static) {
    let media_state = ctx.state.clone();
    let media = media::start(hwnd, move |cmd| media_state.engine.send(cmd));
    let scrobbler = scrobble::start(ctx.state.data_dir.join("scrobble-queue.json"));
    let _ = scrobbler.send(scrobble::Job::Flush);
    let _ = std::thread::Builder::new()
        .name("cadence-status".into())
        .spawn(move || status_hub(ctx, rx, media, scrobbler, on_song));
}

/// Background work at startup: carry on following the singer, then rescan and watch the folders.
pub fn start_background(ctx: &Ctx) {
    voice::resume_batch(ctx, crate::api::resolve_lyrics);
    if !ctx.state.settings.lock().folders.is_empty() {
        // Watching lists the same folders the scan does: once it's done, not alongside.
        jobs::start_scan_then_watch(ctx.clone());
    }
}

fn status_hub(
    ctx: Ctx,
    rx: Receiver<Status>,
    media: crossbeam_channel::Sender<media::Update>,
    scrobbler: crossbeam_channel::Sender<scrobble::Job>,
    on_song: impl Fn(Option<String>),
) {
    let state = ctx.state.clone();
    let listen_for = |id: &str, at: u64| {
        let lib = state.library.lock();
        let t = &lib.snapshot.tracks[*lib.by_id.get(id)?];
        Some(scrobble::Listen {
            artist: t.artist.clone(),
            title: t.title.clone(),
            album: t.album.clone(),
            duration: t.duration,
            at,
        })
    };
    let mut tracker = Tracker::default();
    // The session is saved when the song, the queue or play/pause changes, and every 10 s while
    // playing. Nothing is saved until there has been a queue, so the empty player of a fresh
    // start never overwrites the session it's about to restore.
    let mut had_queue = false;
    let mut saved: (Option<usize>, usize, bool) = (None, 0, false);
    let mut saved_at = std::time::Instant::now();
    let mut saved_pos = 0.0f64;
    let mut shown_track: Option<String> = None;
    let mut shown_playing = false;
    let mut shown_at = std::time::Instant::now();
    // The front end gets the queue (thousands of ids with a big library) only when it changed,
    // not four times a second; it keeps the last one.
    let mut sent_queue: Option<Vec<String>> = None;
    for s in rx {
        if let Ok(mut update) = serde_json::to_value(&s) {
            if sent_queue.as_ref() == Some(&s.queue) {
                if let Some(o) = update.as_object_mut() {
                    o.remove("queue");
                }
            } else {
                sent_queue = Some(s.queue.clone());
            }
            ctx.emit("player-status", update);
        }

        had_queue |= !s.queue.is_empty();
        let key = (s.index, s.queue.len(), s.playing);
        let moved = (s.position - saved_pos).abs() > 1.0; // e.g. a seek while paused
        if had_queue && (key != saved || (moved && (!s.playing || saved_at.elapsed() > std::time::Duration::from_secs(10)))) {
            state.save_session(&s);
            saved = key;
            saved_at = std::time::Instant::now();
            saved_pos = s.position;
        }

        let album_of = |id: &str| {
            let lib = state.library.lock();
            lib.by_id
                .get(id)
                .map(|&i| lib.snapshot.tracks[i].album_id.clone())
                .unwrap_or_default()
        };
        if let Some(event) = tracker.observe(&s, album_of) {
            if scrobble::counts(event.listened, event.duration) {
                if let Some(l) = listen_for(&event.track_id, event.at) {
                    let _ = scrobbler.send(scrobble::Job::Listened(l));
                }
            }
            history::push(&mut state.history.lock(), event);
            state.save_history();
            ctx.emit("history-changed", ());
        }

        let current = s.index.and_then(|i| s.queue.get(i)).cloned();
        if current != shown_track {
            let now_playing = current.as_ref().and_then(|id| {
                let lib = state.library.lock();
                let t = &lib.snapshot.tracks[*lib.by_id.get(id)?];
                let art = lib
                    .snapshot
                    .albums
                    .iter()
                    .find(|a| a.id == t.album_id)
                    .and_then(|a| a.art.clone());
                Some(media::NowPlaying {
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    album: t.album.clone(),
                    cover: art.map(|p| format!("file://{p}")),
                    duration: t.duration,
                })
            });
            on_song(now_playing.as_ref().map(|n| format!("{} — {}", n.title, n.artist)));
            let _ = media.send(media::Update::Track(now_playing));
            if let Some(l) = current.as_deref().and_then(|id| listen_for(id, 0)) {
                if s.playing {
                    let _ = scrobbler.send(scrobble::Job::NowPlaying(l));
                }
            }
            autotag::on_track_change(&ctx, shown_track.as_deref(), current.as_deref());
            shown_track = current;
            shown_at = std::time::Instant::now() - std::time::Duration::from_secs(60);
        }
        // Windows advances the overlay's timeline itself; refresh on play/pause and every few seconds.
        if s.playing != shown_playing || shown_at.elapsed() > std::time::Duration::from_secs(5) {
            let _ = media.send(media::Update::Playback {
                playing: s.playing,
                position: s.position,
            });
            shown_playing = s.playing;
            shown_at = std::time::Instant::now();
        }
    }
}
