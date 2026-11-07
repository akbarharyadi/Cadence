mod ai;
mod api;
mod artists;
mod autotag;
mod catalog;
mod changes;
mod covers;
pub mod cue;
mod ctx;
pub mod audio;
mod commands;
pub mod history;
mod health;
#[cfg(desktop)]
mod hotkeys;
mod hub;
mod jobs;
mod libfix;
pub mod library;
pub mod lyrics;
mod marks;
mod media;
mod net;
mod romanize;
pub mod playlists;
mod scrobble;
mod secrets;
mod sleep;
pub mod sonic;
mod state;
pub mod tagedit;
#[cfg(desktop)]
mod tray;
mod voice;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use ctx::{Ctx, Host};

/// The web UI: events go to its windows.
struct TauriHost(AppHandle);

impl Host for TauriHost {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.0.emit(event, payload);
    }

    fn set_hotkeys(&self, on: bool) {
        #[cfg(desktop)]
        hotkeys::set(&self.0, on);
        #[cfg(mobile)]
        let _ = on;
    }

    fn cache_dir(&self) -> PathBuf {
        self.0.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir())
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
        .plugin(hotkeys::plugin())
        .plugin(tauri_plugin_process::init());
    builder
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                // The decoders note every oddity of every file they open (thousands of lines while
                // songs are analysed), which pushed everything else out of the log.
                .filter(|m| !m.target().starts_with("symphonia") || m.level() <= log::Level::Error)
                .max_file_size(1_000_000)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            #[cfg(target_os = "android")]
            secrets::init(&data_dir);
            let local_dir = app.path().app_local_data_dir().unwrap_or_else(|_| data_dir.clone());
            let (ctx, status) = hub::open(data_dir, &local_dir, Arc::new(TauriHost(app.handle().clone())));
            app.manage(ctx.clone());

            let main = app.get_webview_window("main").expect("main window");
            // Windows' media overlay is tied to the window; macOS and Linux don't need it.
            #[cfg(windows)]
            let hwnd = main.hwnd().map(|h| h.0 as isize).unwrap_or(0);
            #[cfg(not(windows))]
            let hwnd = {
                let _ = &main;
                0isize
            };
            let handle = app.handle().clone();
            hub::start_status_hub(ctx.clone(), status, hwnd, move |song| {
                #[cfg(desktop)]
                tray::set_tooltip(&handle, song);
                #[cfg(mobile)]
                let _ = (&handle, song);
            });

            #[cfg(desktop)]
            tray::create(app.handle())?;
            #[cfg(desktop)]
            hotkeys::set(app.handle(), ctx.state.settings.lock().global_hotkeys);
            // On a phone the music lives in the shared Music folder; start with it.
            #[cfg(target_os = "android")]
            if ctx.state.settings.lock().folders.is_empty() {
                ctx.state.settings.lock().folders.push("/storage/emulated/0/Music".to_string());
                ctx.state.save_settings();
            }
            hub::start_background(&ctx);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps the music playing in the tray (unless turned off).
            #[cfg(desktop)]
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main" {
                    let ctx = window.app_handle().state::<Ctx>();
                    if ctx.state.settings.lock().close_to_tray {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_library,
            commands::voice_status,
            commands::voice_install,
            commands::voice_remove,
            commands::voice_set_auto,
            commands::voice_follow,
            commands::voice_use,
            commands::voice_batch,
            commands::voice_batch_stop,
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
            commands::set_shuffle,
            commands::get_status,
            commands::get_dsp,
            commands::set_dsp,
            commands::eq_presets,
            commands::parse_eq_file,
            commands::get_history,
            commands::get_app_settings,
            commands::set_close_to_tray,
            commands::set_global_hotkeys,
            commands::health_report,
            commands::health_duplicates,
            commands::health_start,
            commands::health_stop,
            commands::health_remove,
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
            commands::find_album_info,
            commands::album_plan,
            commands::save_album_info,
            commands::fix_all_start,
            commands::fix_all_stop,
            commands::artist_check_start,
            commands::artist_check_stop,
            commands::artist_pairs,
            commands::artist_pair_dismiss,
            commands::merge_artist,
            commands::fix_all_found,
            commands::fix_all_save,
            commands::fix_all_discard,
            commands::recent_changes,
            commands::undo_change,
            commands::set_skipped,
            commands::set_skip_instrumentals,
            commands::get_marks,
            commands::set_sleep_timer,
            commands::get_sleep_timer,
            commands::clear_up_next,
            commands::move_in_queue,
            commands::insert_in_queue,
            commands::set_rating,
            commands::set_loved,
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
            commands::listening_profile,
            commands::ui_busy,
            commands::listening_stats,
            commands::ai_recommended,
            commands::ai_still_new,
            commands::ai_daily_picks,
            commands::ai_recommend,
            commands::ai_find_lyrics,
            commands::find_singers,
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
                app.state::<Ctx>().state.save_history();
            }
        });
}
