//! Starting Cadence's core and passing the engine's status on (for lib.rs): history, scrobbling,
//! saved session, media overlay, folder scanning and background analysis.

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
    // Temporary files of saves a crash or power cut interrupted (on a thread: startup doesn't wait).
    {
        let dir = data_dir.clone();
        let _ = std::thread::Builder::new().name("cadence-tidy".into()).spawn(move || {
            jobs::background_priority();
            crate::state::clear_temp_files(&dir);
        });
    }
    let settings: Settings = read_json(&data_dir.join("settings.json"));
    let cache: LibraryCache = read_json(&data_dir.join("library.json"));
    let history: History = read_json(&data_dir.join("history.json"));
    let playlists: crate::playlists::Playlists = read_json(&data_dir.join("playlists.json"));
    let sonic = read_json::<crate::sonic::Store>(&data_dir.join("sonic.json")).checked();
    // Song info edited while its song played, if Cadence closed before it ended
    // (written now, before the last session's song is opened again).
    crate::tagedit::write_leftovers(&data_dir);

    let (status_tx, status_rx) = crossbeam_channel::unbounded::<Status>();
    // On the chosen output device from the start (not the default first).
    let engine = Engine::start_on(settings.output_device.clone(), settings.volume, settings.dsp.clone(), settings.repeat, settings.shuffle, move |s| {
        let _ = status_tx.send(s.clone());
    });
    // (Before the session is put back: its queue was shuffled this way.)
    engine.send(crate::audio::Cmd::SetShuffleMode(settings.shuffle_mode));
    engine.send(crate::audio::Cmd::SetAutoplay(settings.autoplay));

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
        marks: Mutex::new(read_json(&data_dir.join("marks.json"))),
        sleep: Default::default(),
        health: crate::health::Health::new(read_json(&data_dir.join("health.json"))),
        playlists: Mutex::new(playlists),
        library: Mutex::new(lib),
        sonic: Mutex::new(sonic),
        settings: Mutex::new(settings),
        data_dir,
        watcher: Mutex::new(None),
    });
    let voice = Arc::new(voice::Voice::new(&state.data_dir, local_dir));
    // Sound settings per output device (the engine asks when it moves to another device).
    state.engine.send(crate::audio::Cmd::SetProfiles(Some(crate::profiles::source(Arc::downgrade(&state)))));
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
    let scrobbler = start_scrobbler(&ctx);
    let _ = std::thread::Builder::new()
        .name("cadence-status".into())
        .spawn(move || status_hub(ctx, rx, media, scrobbler, on_song));
}

fn start_scrobbler(ctx: &Ctx) -> &'static scrobble::Scrobbler {
    let (account, out) = (ctx.state.clone(), ctx.clone());
    scrobble::start(
        ctx.state.data_dir.join("scrobble-queue.json"),
        move || account.settings.lock().listenbrainz_user.clone(),
        // The token stopped working: Settings shows ListenBrainz as not connected, to connect again.
        move || {
            out.state.settings.lock().listenbrainz_user.clear();
            out.state.save_settings();
            out.emit("listenbrainz-signed-out", ());
        },
    )
}

/// The play being listened to now, shared with `shutdown` (which closes it when Cadence quits).
static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

fn with_tracker<R>(f: impl FnOnce(&mut Tracker) -> R) -> R {
    f(TRACKER.lock().get_or_insert_with(Tracker::default))
}

/// A finished play: into the history (and to ListenBrainz when it counts as a listen).
fn record_play(ctx: &Ctx, scrobbler: Option<&scrobble::Scrobbler>, event: history::PlayEvent) {
    let state = &ctx.state;
    if let Some(s) = scrobbler
        && scrobble::counts(event.listened, event.duration)
        && let Some(l) = listen_of(state, &event.track_id, event.at) {
            let user = state.settings.lock().listenbrainz_user.clone();
            s.listened(l, &user);
        }
    history::push(&mut state.history.lock(), event, &history::archive_dir(&state.data_dir));
    state.save_history();
    ctx.emit("history-changed", ());
}

fn listen_of(state: &AppState, id: &str, at: u64) -> Option<scrobble::Listen> {
    let t = {
        let lib = state.library.lock();
        lib.snapshot.tracks[*lib.by_id.get(id)?].clone()
    };
    // The ids in the song's tags (a small read, kept for the song's second use: it ends after it starts).
    let ids = crate::tagids::of_track(&t);
    Some(scrobble::Listen {
        artist: t.artist.clone(),
        title: t.title.clone(),
        album: t.album.clone(),
        duration: t.duration,
        at,
        user: String::new(),
        recording_mbid: ids.recording,
        release_mbid: ids.release,
        artist_mbids: ids.artists,
        track_no: t.track_no,
    })
}

fn album_of(state: &AppState, id: &str) -> String {
    let lib = state.library.lock();
    lib.by_id.get(id).map(|&i| lib.snapshot.tracks[i].album_id.clone()).unwrap_or_default()
}

/// Cadence is quitting: the song playing now goes into the history (and to ListenBrainz) as far
/// as it was heard, the session keeps the exact position, and settings waiting to be saved are.
pub fn shutdown(ctx: &Ctx) {
    let state = &ctx.state;
    let s = state.engine.status();
    // (An empty player, before the last session was put back, mustn't overwrite it.)
    if !s.queue.is_empty() {
        state.save_session(&s);
    }
    let finished = with_tracker(|t| {
        // The last stretch since the previous update counts too.
        let _ = t.observe(&s, |id| album_of(state, id));
        t.finish()
    });
    match finished {
        Some(event) => record_play(ctx, scrobble::running(), event),
        None => state.save_history(),
    }
    state.save_settings_now();
}

/// Background work at startup: carry on following the singer, then rescan and watch the folders.
pub fn start_background(ctx: &Ctx) {
    voice::resume_batch(ctx, crate::api::resolve_lyrics);
    // The day's snapshot of the user data (when one is due; a second or so, low priority), then
    // the rescan: one after the other, so they never run side by side.
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name("cadence-backup".into()).spawn(move || {
        jobs::background_priority();
        // The artists across scripts, from the saved library (kept up to date from now on).
        crate::identity::init(&ctx);
        crate::backup::snapshot_if_due(&ctx);
        if !ctx.state.settings.lock().folders.is_empty() {
            // Watching lists the same folders the scan does: once it's done, not alongside.
            jobs::start_scan_then_watch(ctx.clone());
        }
    });
}

fn status_hub(
    ctx: Ctx,
    rx: Receiver<Status>,
    media: crossbeam_channel::Sender<media::Update>,
    scrobbler: &'static scrobble::Scrobbler,
    on_song: impl Fn(Option<String>),
) {
    let state = ctx.state.clone();
    // The song ListenBrainz was told is playing: a session put back paused tells it once play starts.
    let mut told_playing: Option<String> = None;
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
    // The last update without its position (and peak level): when only those moved, windows hidden in the
    // tray or minimized aren't sent it (see winvis.rs); anything else goes to every window.
    let mut sent_headline: Option<serde_json::Map<String, serde_json::Value>> = None;
    // While nothing plays the engine still reports once a second: the same status again changes
    // nothing anywhere (no window is woken for it, nothing is saved or told).
    let mut last: Option<serde_json::Value> = None;
    for mut s in rx {
        let Some((update, queue_changed)) = update_of(&mut s, &mut last, &sent_queue) else { continue };
        if let Some(update) = update {
            let mut significant = false;
            if queue_changed {
                sent_queue = Some(s.queue.clone());
                significant = true;
            }
            let headline = crate::winvis::headline(&update);
            significant |= sent_headline.as_ref() != Some(&headline);
            sent_headline = Some(headline);
            match crate::winvis::targets(significant) {
                crate::winvis::Targets::All => ctx.emit("player-status", update),
                crate::winvis::Targets::Only(labels) => {
                    for label in labels {
                        ctx.host().emit_to(label, "player-status", update.clone());
                    }
                }
            }
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

        if let Some(event) = with_tracker(|t| t.observe(&s, |id| album_of(&state, id))) {
            record_play(&ctx, Some(scrobbler), event);
        }
        // The queue is about to run out: the next album of an album shuffle, or Autoplay's songs.
        crate::upnext::on_status(&ctx, &s);

        let current = s.index.and_then(|i| s.queue.get(i)).cloned();
        if s.playing && current.is_some() && current != told_playing {
            if let Some(l) = current.as_deref().and_then(|id| listen_of(&state, id, 0)) {
                scrobbler.now_playing(l);
            }
            told_playing = current.clone();
        }
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

/// Whether a status differs from the last one (one that can't be read always counts as new).
fn changed(last: &Option<serde_json::Value>, now: &Option<serde_json::Value>) -> bool {
    now.is_none() || last != now
}

/// What a status means for the front end: None when it's the same as the last one (skipped
/// everywhere), else the update to send (None: it couldn't be written as JSON) and whether the
/// queue is in it. Only the small part of the status is written as JSON, compared and kept
/// (`last`); the queue, thousands of ids with a big library, is compared as it is with the one
/// sent (`sent`), and written only when it differs.
fn update_of(s: &mut Status, last: &mut Option<serde_json::Value>, sent: &Option<Vec<String>>) -> Option<(Option<serde_json::Value>, bool)> {
    let queue_changed = sent.as_ref() != Some(&s.queue);
    let queue = std::mem::take(&mut s.queue);
    let rest = serde_json::to_value(&*s).ok();
    s.queue = queue;
    if !queue_changed && !changed(last, &rest) {
        return None;
    }
    last.clone_from(&rest);
    let update = rest.map(|mut u| {
        if let Some(o) = u.as_object_mut() {
            o.remove("queue");
            if queue_changed {
                o.insert("queue".into(), serde_json::to_value(&s.queue).unwrap_or_default());
            }
        }
        u
    });
    let queue_changed = queue_changed && update.is_some();
    Some((update, queue_changed))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn the_same_status_again_is_not_passed_on() {
        let paused = Some(json!({ "playing": false, "position": 12.5, "peakDb": -120 }));
        assert!(super::changed(&None, &paused), "the first one");
        assert!(!super::changed(&paused, &paused.clone()), "paused: once a second, the same");
        let moved = Some(json!({ "playing": false, "position": 30.0, "peakDb": -120 }));
        assert!(super::changed(&paused, &moved), "a seek while paused");
        assert!(super::changed(&paused, &None));
    }

    #[test]
    fn the_queue_is_sent_only_when_it_changed_and_never_kept_as_json() {
        let mut s = crate::audio::Status { queue: vec!["a".into(), "b".into()], ..Default::default() };
        let mut last = None;
        let (update, queue) = super::update_of(&mut s, &mut last, &None).expect("the first one");
        let update = update.unwrap();
        assert!(queue && update["queue"] == json!(["a", "b"]));
        assert_eq!(s.queue, ["a", "b"], "the status keeps its queue");
        assert!(last.as_ref().is_some_and(|l| l.get("queue").is_some_and(|q| q == &json!([]))), "what's kept has no ids");
        let sent = Some(s.queue.clone());
        // The same again: skipped.
        assert!(super::update_of(&mut s, &mut last, &sent).is_none());
        // Only the position moved: sent, without the queue.
        s.position = 3.0;
        let (update, queue) = super::update_of(&mut s, &mut last, &sent).unwrap();
        assert!(!queue && update.unwrap().get("queue").is_none());
        // Only the queue changed: sent with it.
        s.queue.push("c".into());
        let (update, queue) = super::update_of(&mut s, &mut last, &sent).unwrap();
        assert!(queue && update.unwrap()["queue"] == json!(["a", "b", "c"]));
    }
}
