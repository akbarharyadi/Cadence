mod ai;
mod artists;
mod autotag;
mod catalog;
mod covers;
pub mod audio;
mod commands;
pub mod history;
mod jobs;
pub mod library;
pub mod lyrics;
mod media;
mod net;
mod romanize;
pub mod playlists;
mod scrobble;
mod secrets;
pub mod sonic;
mod state;
pub mod tagedit;
#[cfg(desktop)]
mod tray;

use std::sync::Arc;

use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use audio::{Engine, Status};
use history::{History, Tracker};
use library::LibraryCache;
use state::{AppState, Library, Settings, read_json};

/// Every engine status update passes through here: to the UI, into the listening history,
/// and to the Windows media overlay and tray.
fn status_hub(
    app: AppHandle,
    state: Arc<AppState>,
    rx: Receiver<Status>,
    media: crossbeam_channel::Sender<media::Update>,
    scrobbler: crossbeam_channel::Sender<scrobble::Job>,
) {
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
    for s in rx {
        let _ = app.emit("player-status", &s);

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
            let _ = app.emit("history-changed", ());
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
            #[cfg(desktop)]
            tray::set_tooltip(
                &app,
                now_playing
                    .as_ref()
                    .map(|n| format!("{} — {}", n.title, n.artist)),
            );
            let _ = media.send(media::Update::Track(now_playing));
            if let Some(l) = current.as_deref().and_then(|id| listen_for(id, 0)) {
                if s.playing {
                    let _ = scrobbler.send(scrobble::Job::NowPlaying(l));
                }
            }
            autotag::on_track_change(&app, shown_track.as_deref(), current.as_deref());
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // Opening Cadence again brings the running window forward instead of starting a second player.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        tray::show_main(app)
    }));
    // Updates (desktop): download a signed new version from GitHub Releases, then restart into it.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());
    builder
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("symphonia_core", log::LevelFilter::Warn)
                .level_for("symphonia_bundle_flac", log::LevelFilter::Warn)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let setup_started = std::time::Instant::now();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(data_dir.join("art"))?;
            #[cfg(target_os = "android")]
            secrets::init(&data_dir);
            let settings: Settings = read_json(&data_dir.join("settings.json"));
            let cache: LibraryCache = read_json(&data_dir.join("library.json"));
            let history: History = read_json(&data_dir.join("history.json"));
            let playlists: playlists::Playlists = read_json(&data_dir.join("playlists.json"));
            let sonic = read_json::<sonic::Store>(&data_dir.join("sonic.json")).checked();

            let (status_tx, status_rx) = crossbeam_channel::unbounded::<Status>();
            let engine = Engine::start(
                settings.volume,
                settings.dsp.clone(),
                settings.repeat,
                move |s| {
                    let _ = status_tx.send(s.clone());
                },
            );

            let read = setup_started.elapsed();
            // Show the cached library immediately; a background rescan picks up changes.
            let mut tracks = cache.tracks;
            let albums = library::build_albums(&mut tracks, &data_dir.join("art"));
            let built = setup_started.elapsed();
            let mut library = Library::default();
            library.set_tracks(tracks, albums);
            let has_folders = !settings.folders.is_empty();
            let state = Arc::new(AppState {
                engine,
                history: Mutex::new(history),
                playlists: Mutex::new(playlists),
                library: Mutex::new(library),
                sonic: Mutex::new(sonic),
                settings: Mutex::new(settings),
                data_dir,
                watcher: Mutex::new(None),
            });
            app.manage(state.clone());
            // Carry on where the last session left off (paused).
            state.restore_session();

            let main = app.get_webview_window("main").expect("main window");
            // Windows' media overlay is tied to the window; macOS and Linux don't need it.
            #[cfg(windows)]
            let hwnd = main.hwnd().map(|h| h.0 as isize).unwrap_or(0);
            #[cfg(not(windows))]
            let hwnd = {
                let _ = &main;
                0isize
            };
            let media_state = state.clone();
            let media = media::start(hwnd, move |cmd| media_state.engine.send(cmd));
            let scrobbler = scrobble::start(state.data_dir.join("scrobble-queue.json"));
            let _ = scrobbler.send(scrobble::Job::Flush);
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("cadence-status".into())
                .spawn(move || status_hub(handle, state, status_rx, media, scrobbler))?;

            #[cfg(desktop)]
            tray::create(app.handle())?;
            // On a phone the music lives in the shared Music folder; start with it.
            #[cfg(target_os = "android")]
            let has_folders = has_folders || {
                let music = "/storage/emulated/0/Music".to_string();
                let st = app.state::<Arc<AppState>>();
                st.settings.lock().folders.push(music);
                st.save_settings();
                true
            };
            if has_folders {
                // Watching lists the same folders the scan does: once it's done, not alongside.
                jobs::start_scan_then_watch(app.handle().clone());
            }
            log::info!(
                "started in {:?} (saved data read by {read:?}, albums built by {built:?})",
                setup_started.elapsed()
            );
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps the music playing in the tray (unless turned off).
            #[cfg(desktop)]
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let state = window.app_handle().state::<Arc<AppState>>();
                    if state.settings.lock().close_to_tray {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_library,
            commands::get_folders,
            commands::open_file,
            commands::add_folders,
            commands::remove_folder,
            commands::list_dirs,
            commands::download_update_apk,
            commands::rescan,
            commands::reanalyze,
            commands::play_tracks,
            commands::enqueue,
            commands::player,
            commands::seek,
            commands::jump,
            commands::remove_from_queue,
            commands::set_volume,
            commands::set_repeat,
            commands::get_status,
            commands::get_dsp,
            commands::set_dsp,
            commands::eq_presets,
            commands::parse_eq_file,
            commands::get_history,
            commands::get_app_settings,
            commands::set_close_to_tray,
            commands::set_lyrics_online,
            commands::set_fix_tags_online,
            commands::get_skips,
            commands::artist_images,
            commands::romanize_lyrics,
            commands::find_covers,
            commands::set_album_cover,
            commands::find_artist_photos,
            commands::set_artist_image,
            commands::reset_artist_image,
            commands::find_song_info,
            commands::save_song_info,
            commands::set_skipped,
            commands::set_skip_instrumentals,
            commands::get_lyrics,
            commands::get_playlists,
            commands::connect_listenbrainz,
            commands::write_tags,
            commands::disconnect_listenbrainz,
            commands::save_playlist,
            commands::delete_playlist,
            commands::sonic_status,
            commands::debug_tap,
            commands::get_ai_settings,
            commands::set_ai_settings,
            commands::set_ai_key,
            commands::search_lyrics,
            commands::use_lyrics,
            commands::paste_lyrics,
            commands::hide_lyrics,
            commands::ai_discover,
            commands::ai_find_lyrics,
            commands::ai_test,
            commands::ai_preview,
            commands::ai_ask,
            commands::ai_mixes,
            commands::similar_tracks,
            commands::sound_radio,
            commands::sound_mixes,
            commands::toggle_mini,
            commands::show_main,
            commands::quit,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Cadence")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Make sure the listening history is on disk.
                app.state::<Arc<AppState>>().save_history();
            }
        });
}
