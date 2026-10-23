//! Tauri commands called from the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::eq::{Filter, parse_parametric, presets};
use crate::audio::{Cmd, DspSettings, Repeat, Status};
use crate::jobs::{start_analysis, start_scan};
use crate::library::Snapshot;
use crate::state::AppState;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

// ---------- library ----------

#[tauri::command]
pub fn get_library(state: AppStateRef) -> Snapshot {
    state.library.lock().snapshot.clone()
}

#[tauri::command]
pub fn get_folders(state: AppStateRef) -> Vec<String> {
    state.settings.lock().folders.clone()
}

/// Adds folders (or the folders of dropped files) to the library and rescans.
#[tauri::command]
pub fn add_folders(app: AppHandle, state: AppStateRef, paths: Vec<String>) -> Vec<String> {
    {
        let mut s = state.settings.lock();
        for p in paths {
            // Rebuilding from components normalizes separators ("C:/x" → "C:\x" on Windows).
            let path: PathBuf = Path::new(&p).components().collect();
            if !path.exists() {
                log::warn!("ignoring missing path {p}");
                continue;
            }
            let folder = if path.is_dir() {
                path
            } else {
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            };
            let folder = folder.to_string_lossy().into_owned();
            let inside = |child: &str, parent: &str| {
                child.starts_with(&format!("{parent}\\"))
                    || child.starts_with(&format!("{parent}/"))
            };
            // Skip folders already covered by a parent folder; replace folders this one covers.
            if !s.folders.iter().any(|f| folder == *f || inside(&folder, f)) {
                s.folders.retain(|f| !inside(f, &folder));
                s.folders.push(folder);
            }
        }
    }
    state.save_settings();
    start_scan(app.clone());
    crate::jobs::watch_folders(app);
    state.settings.lock().folders.clone()
}

/// Android updates: downloads the new APK into the app's cache (the app then hands it to Android's
/// installer), reporting progress as `update-progress` events (0..1).
#[tauri::command]
pub async fn download_update_apk(app: AppHandle, url: String) -> Result<String, String> {
    use std::io::{Read, Write};
    if !url.starts_with("https://github.com/akbarharyadi/cadence/releases/") {
        return Err("Updates only come from Cadence's GitHub releases.".into());
    }
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("cadence-update.apk");
        let mut r = crate::net::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?
            .get(&url)
            .send()
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("Download failed ({})", r.status()));
        }
        let total = r.content_length().unwrap_or(0) as f64;
        let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0f64;
        let mut last = 0f64;
        loop {
            let n = r.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as f64;
            if total > 0.0 && done / total - last >= 0.02 {
                last = done / total;
                let _ = app.emit("update-progress", last);
            }
        }
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A folder in the in-app folder browser (phones have no usable system folder picker).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    /// Music files directly inside (not in subfolders).
    pub songs: usize,
    pub has_folders: bool,
}

/// The subfolders of `path`, or with no path the places to start from: internal storage and any
/// SD card or USB drive (on Android), or the user's Music folder and drives elsewhere.
#[tauri::command]
pub fn list_dirs(path: Option<String>) -> Result<Vec<DirEntry>, String> {
    const AUDIO: [&str; 10] = ["flac", "alac", "wav", "aif", "aiff", "mp3", "m4a", "aac", "ogg", "opus"];
    let is_audio = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
    };
    let entry = |dir: PathBuf, name: String| {
        let mut songs = 0;
        let mut has_folders = false;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    has_folders = true;
                } else if is_audio(&p) {
                    songs += 1;
                }
            }
        }
        DirEntry { name, path: dir.to_string_lossy().into_owned(), songs, has_folders }
    };
    let Some(path) = path else {
        let mut roots = Vec::new();
        #[cfg(target_os = "android")]
        {
            roots.push(entry(PathBuf::from("/storage/emulated/0"), "Phone storage".into()));
            // SD cards and USB drives show up as /storage/XXXX-XXXX.
            if let Ok(rd) = std::fs::read_dir("/storage") {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name != "emulated" && name != "self" && e.path().is_dir() {
                        roots.push(entry(e.path(), format!("SD card ({name})")));
                    }
                }
            }
        }
        #[cfg(not(target_os = "android"))]
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            roots.push(entry(PathBuf::from(home), "Home".into()));
        }
        return Ok(roots);
    };
    let rd = std::fs::read_dir(&path).map_err(|e| format!("Can't open {path}: {e}"))?;
    let mut dirs: Vec<DirEntry> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Hidden and app-private folders are never music folders.
            (!name.starts_with('.') && name != "Android").then(|| entry(e.path(), name))
        })
        .collect();
    dirs.sort_by_key(|d| d.name.to_lowercase());
    Ok(dirs)
}

#[tauri::command]
pub fn remove_folder(app: AppHandle, state: AppStateRef, path: String) -> Vec<String> {
    state.settings.lock().folders.retain(|f| *f != path);
    state.save_settings();
    start_scan(app.clone());
    crate::jobs::watch_folders(app);
    state.settings.lock().folders.clone()
}

#[tauri::command]
pub fn rescan(app: AppHandle) {
    // Folders that couldn't be watched before (e.g. no permission yet on a phone) are retried too.
    crate::jobs::watch_folders(app.clone());
    start_scan(app);
}

/// Forgets Cadence's loudness measurements and measures everything again.
#[tauri::command]
pub fn reanalyze(app: AppHandle, state: AppStateRef) {
    {
        let mut lib = state.library.lock();
        for t in lib.snapshot.tracks.iter_mut().filter(|t| t.gain_measured) {
            t.gain = Default::default();
            t.gain_measured = false;
        }
    }
    start_analysis(app);
}

// ---------- sounds like ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SonicStatus {
    pub analyzed: usize,
    pub total: usize,
    pub running: bool,
}

#[tauri::command]
pub fn sonic_status(state: AppStateRef) -> SonicStatus {
    let lib = state.library.lock();
    let sonic = state.sonic.lock();
    SonicStatus {
        analyzed: lib
            .snapshot
            .tracks
            .iter()
            .filter(|t| !sonic.needs(t))
            .count(),
        total: lib.snapshot.tracks.len(),
        running: lib.listening,
    }
}

/// Runs `f` over every analysed song.
fn with_sonic<T>(state: &AppState, f: impl FnOnce(&[crate::sonic::Item], &crate::state::Library) -> T) -> T {
    let lib = state.library.lock();
    let sonic = state.sonic.lock();
    let items: Vec<_> = lib
        .snapshot
        .tracks
        .iter()
        .filter_map(|t| sonic.features(t).map(|f| crate::sonic::Item { track: t, f }))
        .collect();
    f(&items, &lib)
}

/// Songs that sound like `id`, closest first (empty until the song has been analysed).
#[tauri::command]
pub fn similar_tracks(state: AppStateRef, id: String, limit: Option<usize>) -> Vec<String> {
    with_sonic(&state, |items, lib| {
        let Some(&i) = lib.by_id.get(&id) else {
            return Vec::new();
        };
        crate::sonic::similar(items, &lib.snapshot.tracks[i], limit.unwrap_or(25))
            .into_iter()
            .map(|t| t.id.clone())
            .collect()
    })
}

/// A radio of songs that sound like `id`, starting with it.
#[tauri::command]
pub fn sound_radio(state: AppStateRef, id: String, count: Option<usize>) -> Vec<String> {
    with_sonic(&state, |items, lib| {
        let Some(&i) = lib.by_id.get(&id) else {
            return Vec::new();
        };
        crate::sonic::radio(items, &lib.snapshot.tracks[i], count.unwrap_or(50))
            .into_iter()
            .map(|t| t.id.clone())
            .collect()
    })
}

#[tauri::command]
pub fn sound_mixes(state: AppStateRef) -> Vec<crate::sonic::Mix> {
    with_sonic(&state, |items, _| crate::sonic::mixes(items))
}

// ---------- AI DJ ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInfo {
    settings: crate::ai::AiSettings,
    has_key: bool,
}

#[tauri::command]
pub fn get_ai_settings(state: AppStateRef) -> AiInfo {
    AiInfo {
        settings: state.settings.lock().ai.clone(),
        has_key: crate::secrets::get(crate::ai::KEY_SECRET).is_some(),
    }
}

#[tauri::command]
pub fn set_ai_settings(state: AppStateRef, settings: crate::ai::AiSettings) {
    state.settings.lock().ai = settings;
    state.save_settings();
}

/// Saves the API key. Accepts the bare key or a pasted .env file (ZAI_API_KEY / AI_API_KEY /
/// OPENAI_API_KEY, plus AI_MODEL and AI_BASE_URL if present). Returns the new settings.
#[tauri::command]
pub fn set_ai_key(state: AppStateRef, text: String) -> Result<AiInfo, String> {
    let vars: std::collections::HashMap<String, String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| {
            let k = k.trim().trim_start_matches("export ").trim();
            (k.to_string(), v.trim().trim_matches(['"', '\'']).to_string())
        })
        .collect();
    let key = if vars.is_empty() {
        text.trim().to_string()
    } else {
        ["ZAI_API_KEY", "AI_API_KEY", "OPENAI_API_KEY", "API_KEY"]
            .iter()
            .find_map(|k| vars.get(*k).filter(|v| !v.is_empty()).cloned())
            .ok_or("No API key found. Paste the key itself, or a .env with ZAI_API_KEY=…")?
    };
    crate::secrets::set(crate::ai::KEY_SECRET, &key)?;
    {
        let mut s = state.settings.lock();
        if let Some(m) = vars.get("AI_MODEL").filter(|v| !v.is_empty()) {
            s.ai.cloud_model = m.clone();
        }
        if let Some(u) = vars.get("AI_BASE_URL").filter(|v| !v.is_empty()) {
            s.ai.cloud_url = u.clone();
        }
        if !key.is_empty() && s.ai.provider == crate::ai::Provider::Off {
            s.ai.provider = crate::ai::Provider::Cloud;
        }
    }
    state.save_settings();
    Ok(get_ai_settings(state))
}

/// Everything the DJ needs, copied out of the app state so the slow request runs without locks.
struct DjInput {
    settings: crate::ai::AiSettings,
    taste: String,
    candidates: Vec<crate::ai::Candidate>,
    history: crate::history::HistoryStats,
}

fn dj_input(state: &AppState, request: &str) -> DjInput {
    let rule = state.skip_rule();
    let history = state.history.lock().stats();
    let settings = state.settings.lock().ai.clone();
    let lib = state.library.lock();
    let sonic = state.sonic.lock();
    let items: Vec<_> = lib
        .snapshot
        .tracks
        .iter()
        .filter_map(|t| sonic.features(t).map(|f| crate::sonic::Item { track: t, f }))
        .collect();
    let mut sound = crate::sonic::describe(&items);
    let taste = crate::ai::taste(
        |id| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]),
        &history,
    );
    let candidates = lib
        .snapshot
        .tracks
        .iter()
        .filter(|t| !rule.skips(t))
        .map(|t| crate::ai::Candidate {
            sound: sound.remove(&t.id),
            track: t.clone(),
        })
        .collect();
    let candidates = crate::ai::shortlist(candidates, request, &history);
    DjInput {
        settings,
        taste,
        candidates,
        history,
    }
}

#[tauri::command]
pub async fn ai_test(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let settings = state.settings.lock().ai.clone();
    tauri::async_runtime::spawn_blocking(move || crate::ai::test(&settings).map(|_| "Connected".to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Exactly what would be sent for this request (shown in Settings).
#[tauri::command]
pub fn ai_preview(state: AppStateRef, request: String) -> String {
    let input = dj_input(&state, &request);
    format!(
        "{}\n\n{}",
        crate::ai::SYSTEM,
        crate::ai::ask_prompt(&request, &input.taste, &input.candidates, &input.history)
    )
}

/// Tops a short AI pick up with songs that sound like it, when the library has them.
fn fill_by_sound(state: &AppState, mix: &mut crate::ai::AiMix, want: usize) {
    if mix.track_ids.len() >= want {
        return;
    }
    with_sonic(state, |items, _| {
        let picked: Vec<&[f32]> = items
            .iter()
            .filter(|i| mix.track_ids.contains(&i.track.id))
            .map(|i| i.f)
            .collect();
        if picked.is_empty() {
            return;
        }
        let mut rest: Vec<(f32, &str)> = items
            .iter()
            .filter(|i| !mix.track_ids.contains(&i.track.id))
            .map(|i| {
                let d = picked
                    .iter()
                    .map(|p| crate::sonic::distance(p, i.f))
                    .fold(f32::MAX, f32::min);
                (d, i.track.id.as_str())
            })
            .collect();
        rest.sort_by(|a, b| a.0.total_cmp(&b.0));
        let need = want - mix.track_ids.len();
        mix.track_ids
            .extend(rest.into_iter().take(need).map(|(_, id)| id.to_string()));
    });
}

#[tauri::command]
pub async fn ai_ask(state: State<'_, Arc<AppState>>, request: String) -> Result<crate::ai::AiMix, String> {
    let input = dj_input(&state, &request);
    let library = state.library.lock().snapshot.tracks.clone();
    let mut mix = tauri::async_runtime::spawn_blocking(move || {
        crate::ai::ask(&input.settings, &request, &input.taste, &input.candidates, &input.history).map(|mut m| {
            m.discover = crate::catalog::check(std::mem::take(&mut m.discover), &library);
            m
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    fill_by_sound(&state, &mut mix, 12);
    Ok(mix)
}

/// Today's AI mixes: made once a day (or when `refresh` is set) and kept in ai-mixes.json.
#[tauri::command]
pub async fn ai_mixes(state: State<'_, Arc<AppState>>, refresh: bool) -> Result<Vec<crate::ai::AiMix>, String> {
    let file = state.data_dir.join("ai-mixes.json");
    let cache: crate::ai::DailyCache = crate::state::read_json(&file);
    // Made once a day: today's mixes (or today's failure) are reused unless asked for new ones.
    if !refresh && cache.day == crate::ai::today() {
        if !cache.mixes.is_empty() {
            return Ok(cache.mixes);
        }
        if !cache.error.is_empty() {
            return Err(cache.error);
        }
    }
    let input = dj_input(&state, "");
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::ai::daily_mixes(&input.settings, &input.taste, &input.candidates, &input.history)
    })
    .await
    .map_err(|e| e.to_string())?;
    crate::state::write_json(
        &file,
        &crate::ai::DailyCache {
            day: crate::ai::today(),
            mixes: result.clone().unwrap_or_default(),
            error: result.as_ref().err().cloned().unwrap_or_default(),
        },
    );
    result
}

// ---------- player ----------

/// Plays a list from `start`. Skipped songs are left out, except the one the user picked (`picked`).
#[tauri::command]
pub fn play_tracks(state: AppStateRef, ids: Vec<String>, start: usize, picked: Option<bool>) {
    let rule = state.skip_rule();
    let (ids, start_id) = {
        let lib = state.library.lock();
        without_skipped(ids, start, picked.unwrap_or(false), |id| {
            lib.by_id.get(id).is_some_and(|&k| rule.skips(&lib.snapshot.tracks[k]))
        })
    };
    if ids.is_empty() {
        return; // Everything in the list is skipped: keep playing what's on.
    }
    let items = state.queue_items(&ids);
    let start = start_id
        .and_then(|id| items.iter().position(|q| q.id == id))
        .unwrap_or(0);
    state.engine.send(Cmd::Load {
        items,
        index: start,
    });
}

/// The list without skipped songs, and the song to start with: the one at `start` if the user
/// picked it, else the first one from there that isn't skipped.
fn without_skipped(
    ids: Vec<String>,
    start: usize,
    picked: bool,
    skips: impl Fn(&str) -> bool,
) -> (Vec<String>, Option<String>) {
    let kept: Vec<(usize, String)> = ids
        .into_iter()
        .enumerate()
        .filter(|(i, id)| (picked && *i == start) || !skips(id))
        .collect();
    let start_id = kept
        .iter()
        .find(|(i, _)| *i >= start)
        .or(kept.first())
        .map(|(_, id)| id.clone());
    (kept.into_iter().map(|(_, id)| id).collect(), start_id)
}

#[tauri::command]
pub fn enqueue(state: AppStateRef, ids: Vec<String>, next: bool) {
    let items = state.queue_items(&ids);
    state.engine.send(Cmd::Enqueue { items, next });
}

#[tauri::command]
pub fn player(state: AppStateRef, action: String) {
    let cmd = match action.as_str() {
        "play" => Cmd::Play,
        "pause" => Cmd::Pause,
        "toggle" => Cmd::Toggle,
        "next" => Cmd::Next,
        "prev" => Cmd::Prev,
        _ => return,
    };
    state.engine.send(cmd);
}

#[tauri::command]
pub fn seek(state: AppStateRef, position: f64) {
    state.engine.send(Cmd::Seek(position));
}

#[tauri::command]
pub fn jump(state: AppStateRef, index: usize) {
    state.engine.send(Cmd::Jump(index));
}

#[tauri::command]
pub fn remove_from_queue(state: AppStateRef, index: usize) {
    state.engine.send(Cmd::Remove(index));
}

#[tauri::command]
pub fn set_volume(state: AppStateRef, volume: f32) {
    state.engine.send(Cmd::SetVolume(volume));
    state.settings.lock().volume = volume;
    state.save_settings();
}

#[tauri::command]
pub fn set_repeat(state: AppStateRef, repeat: Repeat) {
    state.engine.send(Cmd::SetRepeat(repeat));
    state.settings.lock().repeat = repeat;
    state.save_settings();
}

#[tauri::command]
pub fn get_status(state: AppStateRef) -> Status {
    state.engine.status()
}

// ---------- sound (EQ, leveling, crossfeed) ----------

#[derive(Serialize)]
pub struct Preset {
    name: &'static str,
    gains: [f64; 10],
}

#[tauri::command]
pub fn get_dsp(state: AppStateRef) -> DspSettings {
    state.settings.lock().dsp.clone()
}

#[tauri::command]
pub fn set_dsp(state: AppStateRef, dsp: DspSettings) {
    state.engine.send(Cmd::SetDsp(dsp.clone()));
    state.settings.lock().dsp = dsp;
    state.save_settings();
}

#[tauri::command]
pub fn eq_presets() -> Vec<Preset> {
    presets()
        .into_iter()
        .map(|(name, gains)| Preset { name, gains })
        .collect()
}

#[derive(Serialize)]
pub struct ParametricImport {
    preamp: f64,
    filters: Vec<Filter>,
}

/// Parses an AutoEq / Equalizer APO `ParametricEQ.txt` (text pasted or read from a file).
#[tauri::command]
pub fn parse_eq_file(
    text: Option<String>,
    path: Option<String>,
) -> Result<ParametricImport, String> {
    let text = match (text, path) {
        (Some(t), _) => t,
        (None, Some(p)) => {
            std::fs::read_to_string(&p).map_err(|e| format!("Couldn't read the file: {e}"))?
        }
        _ => return Err("Nothing to import".into()),
    };
    let (preamp, filters) = parse_parametric(&text)?;
    Ok(ParametricImport { preamp, filters })
}

// ---------- history, windows ----------

#[tauri::command]
pub fn get_history(state: AppStateRef) -> crate::history::HistoryStats {
    state.history.lock().stats()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    close_to_tray: bool,
    lyrics_online: bool,
    fix_tags_online: bool,
    listenbrainz_user: String,
}

#[tauri::command]
pub fn get_app_settings(state: AppStateRef) -> AppSettings {
    let s = state.settings.lock();
    AppSettings {
        close_to_tray: s.close_to_tray,
        lyrics_online: s.lyrics_online,
        fix_tags_online: s.fix_tags_online,
        listenbrainz_user: s.listenbrainz_user.clone(),
    }
}

#[tauri::command]
pub fn set_lyrics_online(state: AppStateRef, enabled: bool) {
    state.settings.lock().lyrics_online = enabled;
    state.save_settings();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skips {
    ids: Vec<String>,
    instrumentals: bool,
}

#[tauri::command]
pub fn get_skips(state: AppStateRef) -> Skips {
    let s = state.settings.lock();
    Skips {
        ids: s.skipped.iter().cloned().collect(),
        instrumentals: s.skip_instrumentals,
    }
}

/// Marks songs to be skipped (or not). Newly skipped songs also leave the queue; if one is
/// playing, Cadence moves on to the next song.
#[tauri::command]
pub fn set_skipped(state: AppStateRef, ids: Vec<String>, skip: bool) {
    {
        let mut s = state.settings.lock();
        for id in &ids {
            if skip {
                s.skipped.insert(id.clone());
            } else {
                s.skipped.remove(id);
            }
        }
    }
    state.save_settings();
    if !skip {
        return;
    }
    let status = state.engine.status();
    let Some(current) = status.index else {
        return;
    };
    for (i, id) in status.queue.iter().enumerate().rev() {
        if i > current && ids.contains(id) {
            state.engine.send(Cmd::Remove(i));
        }
    }
    if status.queue.get(current).is_some_and(|id| ids.contains(id)) {
        state.engine.send(Cmd::Next);
    }
}

#[tauri::command]
pub fn set_skip_instrumentals(state: AppStateRef, enabled: bool) {
    state.settings.lock().skip_instrumentals = enabled;
    state.save_settings();
}

#[tauri::command]
pub fn set_fix_tags_online(state: AppStateRef, enabled: bool) {
    state.settings.lock().fix_tags_online = enabled;
    state.save_settings();
}

/// Lyrics for a song: from the file or a .lrc next to it, else LRCLIB (if allowed).
#[tauri::command]
pub async fn get_lyrics(
    state: State<'_, Arc<AppState>>,
    track_id: String,
) -> Result<crate::lyrics::Lyrics, String> {
    let (track, online) = {
        let lib = state.library.lock();
        let t = lib
            .by_id
            .get(&track_id)
            .map(|&i| lib.snapshot.tracks[i].clone())
            .ok_or("Unknown song")?;
        (t, state.settings.lock().lyrics_online)
    };
    let cache = state.data_dir.join("lyrics");
    tauri::async_runtime::spawn_blocking(move || {
        if crate::lyrics::is_hidden(&cache, &track.id) {
            return crate::lyrics::Lyrics {
                lookup: "hidden".into(),
                ..Default::default()
            };
        }
        if let Some(l) = crate::lyrics::picked(&cache, &track.id) {
            return l;
        }
        // Lyrics in the file win when they're timed. Untimed ones (common in tags) can't follow
        // the song, so timed lyrics from the saved store or LRCLIB are preferred when there are any.
        let local = crate::lyrics::local(Path::new(&track.path));
        if let Some(l) = local.as_ref().filter(|l| !l.synced.is_empty()) {
            return l.clone();
        }
        let found = crate::lyrics::saved(&cache, &track.id, &track.title).map(Ok).unwrap_or_else(|| {
            if online {
                crate::lyrics::online(
                    &cache,
                    &track.id,
                    &track.artist,
                    &track.title,
                    &track.album,
                    track.duration,
                )
            } else {
                Err("off")
            }
        });
        match (found, local) {
            (Ok(l), _) if !l.synced.is_empty() => l,
            (_, Some(l)) => l,
            (Ok(l), None) => l,
            (Err(why), None) => crate::lyrics::Lyrics {
                lookup: why.into(),
                ..Default::default()
            },
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_close_to_tray(state: AppStateRef, enabled: bool) {
    state.settings.lock().close_to_tray = enabled;
    state.save_settings();
}

// Async on purpose: creating a window from a synchronous command deadlocks on Windows.
#[tauri::command]
pub async fn toggle_mini(app: AppHandle) {
    #[cfg(desktop)]
    crate::tray::toggle_mini(&app);
    #[cfg(mobile)]
    let _ = app;
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    #[cfg(desktop)]
    crate::tray::show_main(&app);
    #[cfg(mobile)]
    let _ = app;
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

// ---------- playlists ----------

#[tauri::command]
pub fn get_playlists(state: AppStateRef) -> Vec<crate::playlists::Playlist> {
    state.playlists.lock().items.clone()
}

/// Creates (empty id) or updates a playlist; returns it with its id.
#[tauri::command]
pub fn save_playlist(
    state: AppStateRef,
    playlist: crate::playlists::Playlist,
) -> crate::playlists::Playlist {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let saved = state.playlists.lock().upsert(playlist, now);
    state.save_playlists();
    saved
}

#[tauri::command]
pub fn delete_playlist(state: AppStateRef, id: String) {
    state.playlists.lock().remove(&id);
    state.save_playlists();
}

// ---------- scrobbling ----------

/// Checks the token with ListenBrainz, then stores it in Credential Manager. Returns the user name.
#[tauri::command]
pub async fn connect_listenbrainz(
    state: State<'_, Arc<AppState>>,
    token: String,
) -> Result<String, String> {
    let t = token.clone();
    let user = tauri::async_runtime::spawn_blocking(move || crate::scrobble::validate(&t))
        .await
        .map_err(|e| e.to_string())??;
    crate::secrets::set(crate::scrobble::TOKEN_SECRET, &token)?;
    state.settings.lock().listenbrainz_user = user.clone();
    state.save_settings();
    Ok(user)
}

#[tauri::command]
pub fn disconnect_listenbrainz(state: AppStateRef) -> Result<(), String> {
    crate::secrets::set(crate::scrobble::TOKEN_SECRET, "")?;
    state.settings.lock().listenbrainz_user.clear();
    state.save_settings();
    Ok(())
}

// ---------- tag editing ----------

/// Writes the changed fields into each song's file, then rescans so the library shows them.
#[tauri::command]
pub async fn write_tags(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    ids: Vec<String>,
    changes: crate::tagedit::TagChanges,
) -> Result<usize, String> {
    let playing = {
        let s = state.engine.status();
        s.index.and_then(|i| s.queue.get(i).cloned())
    };
    if ids.iter().any(|id| Some(id) == playing.as_ref()) {
        return Err(
            "This song is playing. Play something else (or pause and skip ahead), then edit it."
                .into(),
        );
    }
    let paths: Vec<String> = {
        let lib = state.library.lock();
        ids.iter()
            .filter_map(|id| {
                lib.by_id
                    .get(id)
                    .map(|&i| lib.snapshot.tracks[i].path.clone())
            })
            .collect()
    };
    let written = tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        for p in &paths {
            crate::tagedit::write(Path::new(p), &changes)?;
        }
        Ok(paths.len())
    })
    .await
    .map_err(|e| e.to_string())??;
    start_scan(app);
    Ok(written)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongInfoResults {
    term: String,
    results: Vec<crate::autotag::Candidate>,
}

/// Fix Song Info: catalogue matches for a song (searching `query`, or its name).
#[tauri::command]
pub async fn find_song_info(
    state: State<'_, Arc<AppState>>,
    id: String,
    query: Option<String>,
) -> Result<SongInfoResults, String> {
    let track = track_of(&state, &id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let term = query
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .unwrap_or_else(|| crate::autotag::search_term(&track));
        let results = crate::autotag::candidates(&track, &term)?;
        Ok(SongInfoResults { term, results })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves the match picked in Fix Song Info; true when the file is written after the song ends.
#[tauri::command]
pub async fn save_song_info(app: AppHandle, id: String, info: crate::autotag::Candidate) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || crate::autotag::save_choice(&app, &id, info))
        .await
        .map_err(|e| e.to_string())?
}

// ---------- album covers ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverResults {
    term: String,
    results: Vec<crate::covers::Cover>,
}

/// Change Cover: covers from the online catalogues for an album (searching `query`, or its
/// artist and title).
#[tauri::command]
pub async fn find_covers(
    state: State<'_, Arc<AppState>>,
    album_id: String,
    query: Option<String>,
) -> Result<CoverResults, String> {
    let (title, artist) = {
        let lib = state.library.lock();
        let a = lib.snapshot.albums.iter().find(|a| a.id == album_id).ok_or("Unknown album")?;
        (a.title.clone(), a.artist.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        // The user's words, or (when the full name finds nothing) shorter ones.
        let terms = match query.map(|q| q.trim().to_string()).filter(|q| !q.is_empty()) {
            Some(q) => vec![q],
            None => crate::covers::search_terms(&artist, &title),
        };
        let mut last = (terms[0].clone(), Vec::new());
        for term in terms {
            let results = crate::covers::search(&term, &artist, &title)?;
            let found = !results.is_empty();
            last = (term, results);
            if found {
                break;
            }
        }
        Ok(CoverResults { term: last.0, results: last.1 })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Sets an album's cover (shown at once, and written into its songs' files).
#[tauri::command]
pub async fn set_album_cover(
    app: AppHandle,
    album_id: String,
    source: crate::artists::Source,
) -> Result<crate::covers::Changed, String> {
    tauri::async_runtime::spawn_blocking(move || crate::covers::set(&app, &album_id, source))
        .await
        .map_err(|e| e.to_string())?
}

// ---------- artist pictures ----------

#[tauri::command]
pub fn artist_images(state: AppStateRef) -> std::collections::HashMap<String, crate::artists::ArtistImage> {
    crate::artists::all(&state.data_dir)
}

/// Photos of this artist from the online catalogue.
#[tauri::command]
pub async fn find_artist_photos(name: String) -> Result<Vec<crate::artists::Photo>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::artists::search(&name))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_artist_image(
    state: State<'_, Arc<AppState>>,
    name: String,
    source: crate::artists::Source,
) -> Result<crate::artists::ArtistImage, String> {
    let (data, art) = (state.data_dir.clone(), state.art_dir());
    tauri::async_runtime::spawn_blocking(move || crate::artists::set(&data, &art, &name, source))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn reset_artist_image(state: AppStateRef, name: String) {
    crate::artists::reset(&state.data_dir, &name);
}

fn track_of(state: &AppState, id: &str) -> Result<crate::library::Track, String> {
    let lib = state.library.lock();
    lib.by_id
        .get(id)
        .map(|&i| lib.snapshot.tracks[i].clone())
        .ok_or_else(|| "Unknown song".to_string())
}

/// Searches LRCLIB for lyrics (any text: title, artist, a line).
#[tauri::command]
pub async fn search_lyrics(query: String) -> Result<Vec<crate::lyrics::Hit>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::lyrics::search(&query))
        .await
        .map_err(|e| e.to_string())?
}

/// Uses an LRCLIB search result as this song's lyrics (kept for next time).
#[tauri::command]
pub async fn use_lyrics(
    state: State<'_, Arc<AppState>>,
    track_id: String,
    hit_id: u64,
) -> Result<crate::lyrics::Lyrics, String> {
    let cache = state.data_dir.join("lyrics");
    tauri::async_runtime::spawn_blocking(move || {
        let l = crate::lyrics::fetch(hit_id)?;
        crate::lyrics::pick(&cache, &track_id, &l);
        Ok(l)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Marks this song's lyrics as wrong, so they aren't shown (or looked up) again.
#[tauri::command]
pub fn hide_lyrics(state: AppStateRef, track_id: String) {
    crate::lyrics::hide(&state.data_dir.join("lyrics"), &track_id);
}

/// Uses pasted text (LRC or plain) as this song's lyrics.
#[tauri::command]
pub fn paste_lyrics(state: AppStateRef, track_id: String, text: String) -> Result<crate::lyrics::Lyrics, String> {
    let l = crate::lyrics::from_pasted(&text).ok_or("There’s no text to use.")?;
    crate::lyrics::pick(&state.data_dir.join("lyrics"), &track_id, &l);
    Ok(l)
}

/// Lets the AI find lyrics: it suggests better searches for LRCLIB (for time-synced lyrics) and
/// looks the lyrics up on the web as a fallback.
#[tauri::command]
pub async fn ai_find_lyrics(
    state: State<'_, Arc<AppState>>,
    track_id: String,
) -> Result<crate::lyrics::Lyrics, String> {
    let track = track_of(&state, &track_id)?;
    let settings = state.settings.lock().ai.clone();
    let cache = state.data_dir.join("lyrics");
    tauri::async_runtime::spawn_blocking(move || {
        let found = crate::ai::find_lyrics(&settings, &track)?;
        // Synced lyrics from LRCLIB beat anything else, so try the AI's searches there first.
        for q in &found.queries {
            if let Ok(hits) = crate::lyrics::search(q) {
                let wanted = crate::lyrics::Wanted {
                    title: &track.title,
                    artist: &track.artist,
                    album: &track.album,
                    duration: track.duration,
                };
                if let Some(hit) = crate::lyrics::best_hit(&hits, &wanted) {
                    if let Ok(l) = crate::lyrics::fetch(hit.id) {
                        crate::lyrics::pick(&cache, &track.id, &l);
                        return Ok(l);
                    }
                }
            }
        }
        let l = if found.instrumental {
            crate::lyrics::Lyrics {
                source: "ai".into(),
                instrumental: true,
                ..Default::default()
            }
        } else {
            crate::lyrics::from_ai(&found.lrc)
                .filter(|l| !l.synced.is_empty())
                .or_else(|| crate::lyrics::from_ai(&found.plain))
                .ok_or("The AI couldn’t find lyrics for this song.")?
        };
        crate::lyrics::pick(&cache, &track.id, &l);
        Ok(l)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Diagnostics: records what goes to the device into `%TEMP%\cadence-tap.f32` while on.
#[tauri::command]
pub fn debug_tap(state: AppStateRef, on: bool) -> String {
    state.engine.tap(on);
    std::env::temp_dir().join("cadence-tap.f32").display().to_string()
}

/// New music (not in the library) for the listener, found by the AI with web search. Made once a
/// day (or when `refresh` is set) and kept in ai-discover.json.
#[tauri::command]
pub async fn ai_discover(state: State<'_, Arc<AppState>>, refresh: bool) -> Result<Vec<crate::ai::Discovery>, String> {
    let file = state.data_dir.join("ai-discover.json");
    let cache: crate::ai::DiscoverCache = crate::state::read_json(&file);
    // Made once a day: today's picks (or today's failure) are reused unless asked for more.
    let library = state.library.lock().snapshot.tracks.clone();
    if !refresh && cache.day == crate::ai::today() {
        if !cache.picks.is_empty() {
            // Picks saved before they were checked against the library and catalogue get checked now.
            if cache.picks.iter().any(|p| !p.checked) {
                let picks = cache.picks;
                let lib = library.clone();
                let checked = tauri::async_runtime::spawn_blocking(move || crate::catalog::check(picks, &lib))
                    .await
                    .map_err(|e| e.to_string())?;
                crate::state::write_json(
                    &file,
                    &crate::ai::DiscoverCache {
                        day: cache.day,
                        picks: checked.clone(),
                        error: String::new(),
                    },
                );
                return Ok(checked);
            }
            return Ok(cache.picks);
        }
        if !cache.error.is_empty() {
            return Err(cache.error);
        }
    }
    let input = dj_input(&state, "");
    let owned: Vec<String> = {
        let lib = state.library.lock();
        let mut a: Vec<String> = lib.snapshot.tracks.iter().map(|t| t.artist.clone()).collect();
        a.sort();
        a.dedup();
        a
    };
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::ai::discover(&input.settings, &input.taste, &input.candidates, &owned)
            .map(|picks| crate::catalog::check(picks, &library))
            .and_then(|picks| {
                if picks.is_empty() {
                    Err("Everything the DJ found is already in your library. Try “Find more”.".to_string())
                } else {
                    Ok(picks)
                }
            })
    })
    .await
    .map_err(|e| e.to_string())?;
    crate::state::write_json(
        &file,
        &crate::ai::DiscoverCache {
            day: crate::ai::today(),
            picks: result.clone().unwrap_or_default(),
            error: result.as_ref().err().cloned().unwrap_or_default(),
        },
    );
    result
}

#[cfg(test)]
mod tests {
    use super::without_skipped;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn skipped_songs_leave_lists() {
        let skips = |id: &str| id.starts_with('x');
        // Play / Shuffle: skipped songs are left out, even the first one.
        let (list, start) = without_skipped(ids(&["x1", "a", "x2", "b"]), 0, false, skips);
        assert_eq!(list, ids(&["a", "b"]));
        assert_eq!(start.as_deref(), Some("a"));
        // Picking a skipped song plays it, the rest still skip.
        let (list, start) = without_skipped(ids(&["a", "x1", "b", "x2"]), 1, true, skips);
        assert_eq!(list, ids(&["a", "x1", "b"]));
        assert_eq!(start.as_deref(), Some("x1"));
        // Starting on a skipped song that wasn't picked moves on to the next one.
        let (_, start) = without_skipped(ids(&["a", "x1", "b"]), 1, false, skips);
        assert_eq!(start.as_deref(), Some("b"));
        // Everything skipped: nothing plays.
        let (list, start) = without_skipped(ids(&["x1", "x2"]), 0, false, skips);
        assert!(list.is_empty() && start.is_none());
    }
}
