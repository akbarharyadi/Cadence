//! Everything the UI can ask of Cadence, as plain functions over `Ctx`. The Tauri app wraps them
//! as commands (commands.rs) and the native Windows app calls them through ffi.rs; the list at the
//! end (`cadence_commands!`) drives both. Slow ones block: Tauri runs them on a worker thread,
//! the native app calls them off its UI thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::audio::eq::{Filter, parse_parametric, presets};
use crate::audio::{Cmd, DspSettings, Repeat, Status};
use crate::ctx::Ctx;
use crate::jobs::{start_analysis, start_scan};
use crate::library::Snapshot;
use crate::state::AppState;

// ---------- library ----------

pub fn get_library(ctx: &Ctx) -> Snapshot {
    let state = &ctx.state;
    state.library.lock().snapshot.clone()
}

pub fn get_folders(ctx: &Ctx) -> Vec<String> {
    let state = &ctx.state;
    state.settings.lock().folders.clone()
}

/// `child` is a file or folder somewhere inside the folder `parent`.
fn inside(child: &str, parent: &str) -> bool {
    child.starts_with(&format!("{parent}\\")) || child.starts_with(&format!("{parent}/"))
}

/// Adds folders (or the folders of dropped files) to the library and rescans.
pub fn add_folders(ctx: &Ctx, paths: Vec<String>) -> Vec<String> {
    let state = &ctx.state;
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
            // Skip folders already covered by a parent folder; replace folders this one covers.
            if !s.folders.iter().any(|f| folder == *f || inside(&folder, f)) {
                s.folders.retain(|f| !inside(f, &folder));
                s.folders.push(folder);
            }
        }
    }
    state.save_settings();
    start_scan(ctx.clone());
    crate::jobs::watch_folders(ctx.clone());
    state.settings.lock().folders.clone()
}

/// What opening a song from another app did.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub id: String,
    /// The song wasn't in the library: it was added, on its own.
    pub added: bool,
}

/// Plays a song opened from another app ("Open with Cadence" in a file manager). A song outside
/// the music folders is added to the library on its own (not its whole folder, which may be
/// Downloads or voice messages), and plays as soon as it has been read.
pub fn open_file(ctx: &Ctx, path: String) -> Result<Opened, String> {
    let state = &ctx.state;
    let path: PathBuf = Path::new(&path).components().collect();
    // "/sdcard/..." is "/storage/emulated/0/...", as the library knows it.
    #[cfg(unix)]
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    if !path.is_file() || !crate::library::is_audio(&path) {
        return Err(format!("Cadence can't play {}", path.display()));
    }
    let key = path.to_string_lossy().into_owned();
    // Songs are known by their path (see library::read_track).
    let id = crate::library::short_hash(&key);
    let mut added = false;
    if !state.library.lock().by_id.contains_key(&id) {
        {
            let mut s = state.settings.lock();
            if !s.folders.iter().any(|f| *f == key || inside(&key, f)) {
                s.folders.push(key.clone());
                added = true;
            }
        }
        if added {
            state.save_settings();
            crate::jobs::watch_folders(ctx.clone());
        }
        // Also when it's in a music folder already: a song copied there just now isn't read yet.
        start_scan(ctx.clone());
    }
    let state = state.clone();
    let wanted = id.clone();
    std::thread::spawn(move || {
        for _ in 0..600 {
            if state.library.lock().by_id.contains_key(&wanted) {
                let items = state.queue_items(&[wanted]);
                state.engine.send(Cmd::Load { items, index: 0 });
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        log::warn!("opened song {wanted} never showed up in the library");
    });
    Ok(Opened { id, added })
}

/// Android updates: downloads the new APK into the app's cache (the app then hands it to Android's
/// installer), reporting progress as `update-progress` events (0..1).
pub fn download_update_apk(ctx: &Ctx, url: String) -> Result<String, String> {
    use std::io::{Read, Write};
    // GitHub names are case-insensitive (the repository is now "Cadence"; it was "cadence").
    if !url.to_ascii_lowercase().starts_with("https://github.com/akbarharyadi/cadence/releases/") {
        return Err("Updates only come from Cadence's GitHub releases.".into());
    }
    let dir = ctx.host().cache_dir();
    (move || {
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
                ctx.emit("update-progress", last);
            }
        }
        Ok(path.to_string_lossy().into_owned())
    })()
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

/// An SD card or USB drive, as Android lists it.
#[derive(serde::Deserialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct Volume {
    pub path: String,
    pub name: String,
}

/// The subfolders of `path`, or with no path the places to start from: internal storage and any
/// SD card or USB drive (on Android, which lists them in `volumes`), or the user's home elsewhere.
pub fn list_dirs(_ctx: &Ctx, path: Option<String>, volumes: Option<Vec<Volume>>) -> Result<Vec<DirEntry>, String> {
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
            for v in volumes.unwrap_or_default() {
                if !roots.iter().any(|r| r.path == v.path) {
                    roots.push(entry(PathBuf::from(&v.path), v.name));
                }
            }
            // SD cards and USB drives are /storage/XXXX-XXXX, though newer Android versions don't
            // let apps list /storage (hence the volumes from Android itself).
            if let Ok(rd) = std::fs::read_dir("/storage") {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let known = roots.iter().any(|r| Path::new(&r.path) == e.path());
                    if name != "emulated" && name != "self" && !known && e.path().is_dir() {
                        roots.push(entry(e.path(), format!("SD card ({name})")));
                    }
                }
            }
        }
        #[cfg(not(target_os = "android"))]
        let _ = volumes;
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

pub fn remove_folder(ctx: &Ctx, path: String) -> Vec<String> {
    let state = &ctx.state;
    state.settings.lock().folders.retain(|f| *f != path);
    state.save_settings();
    start_scan(ctx.clone());
    crate::jobs::watch_folders(ctx.clone());
    state.settings.lock().folders.clone()
}

pub fn rescan(ctx: &Ctx) {
    // Folders that couldn't be watched before (e.g. no permission yet on a phone) are retried too.
    crate::jobs::watch_folders(ctx.clone());
    start_scan(ctx.clone());
}

/// Forgets Cadence's loudness measurements and measures everything again.
pub fn reanalyze(ctx: &Ctx) {
    let state = &ctx.state;
    {
        let mut lib = state.library.lock();
        for t in lib.snapshot.tracks.iter_mut().filter(|t| t.gain_measured) {
            t.gain = Default::default();
            t.gain_measured = false;
        }
    }
    start_analysis(ctx.clone());
}

// ---------- sounds like ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SonicStatus {
    pub analyzed: usize,
    pub total: usize,
    pub running: bool,
}

pub fn sonic_status(ctx: &Ctx) -> SonicStatus {
    let state = &ctx.state;
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
pub fn similar_tracks(ctx: &Ctx, id: String, limit: Option<usize>) -> Vec<String> {
    let state = &ctx.state;
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
pub fn sound_radio(ctx: &Ctx, id: String, count: Option<usize>) -> Vec<String> {
    let state = &ctx.state;
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

pub fn sound_mixes(ctx: &Ctx) -> Vec<crate::sonic::Mix> {
    let state = &ctx.state;
    with_sonic(&state, |items, _| crate::sonic::mixes(items))
}

// ---------- AI DJ ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInfo {
    settings: crate::ai::AiSettings,
    has_key: bool,
}

pub fn get_ai_settings(ctx: &Ctx) -> AiInfo {
    let state = &ctx.state;
    // One lock, released before the key lookup (a second lock in the same expression deadlocks).
    let settings = state.settings.lock().ai.clone();
    let has_key = crate::secrets::get(settings.service.key_name()).is_some();
    AiInfo { settings, has_key }
}

pub fn set_ai_settings(ctx: &Ctx, settings: crate::ai::AiSettings) {
    let state = &ctx.state;
    state.settings.lock().ai = settings;
    state.save_settings();
}

/// Saves the API key for the chosen service. Accepts the bare key or a pasted .env file
/// (ANTHROPIC_API_KEY for Claude; ZAI_API_KEY / AI_API_KEY / OPENAI_API_KEY, plus AI_MODEL and
/// AI_BASE_URL, for an OpenAI-compatible service). An Anthropic key (sk-ant-…) switches to Claude.
/// Returns the new settings.
pub fn set_ai_key(ctx: &Ctx, text: String) -> Result<AiInfo, String> {
    let state = &ctx.state;
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
    let claude_env = vars.get("ANTHROPIC_API_KEY").filter(|v| !v.is_empty()).cloned();
    let key = if let Some(k) = &claude_env {
        k.clone()
    } else if vars.is_empty() {
        text.trim().to_string()
    } else {
        ["ZAI_API_KEY", "AI_API_KEY", "OPENAI_API_KEY", "API_KEY"]
            .iter()
            .find_map(|k| vars.get(*k).filter(|v| !v.is_empty()).cloned())
            .ok_or("No API key found. Paste the key itself, or a .env with ANTHROPIC_API_KEY=… or ZAI_API_KEY=…")?
    };
    let service = if claude_env.is_some() || key.starts_with("sk-ant-") {
        crate::ai::Service::Claude
    } else if !vars.is_empty() {
        crate::ai::Service::Openai
    } else {
        state.settings.lock().ai.service
    };
    crate::secrets::set(service.key_name(), &key)?;
    {
        let mut s = state.settings.lock();
        if !key.is_empty() {
            s.ai.service = service;
        }
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
    Ok(get_ai_settings(ctx))
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

pub fn ai_test(ctx: &Ctx) -> Result<String, String> {
    let state = &ctx.state;
    let settings = state.settings.lock().ai.clone();
    (move || crate::ai::test(&settings).map(|_| "Connected".to_string()))()
}

/// Exactly what would be sent for this request (shown in Settings).
pub fn ai_preview(ctx: &Ctx, request: String) -> String {
    let state = &ctx.state;
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

pub fn ai_ask(ctx: &Ctx, request: String) -> Result<crate::ai::AiMix, String> {
    let state = &ctx.state;
    let input = dj_input(&state, &request);
    let library = state.library.lock().snapshot.tracks.clone();
    let mut mix = (move || {
        crate::ai::ask(&input.settings, &request, &input.taste, &input.candidates, &input.history).map(|mut m| {
            m.discover = crate::catalog::check(std::mem::take(&mut m.discover), &library);
            m
        })
    })()?;
    fill_by_sound(&state, &mut mix, 12);
    Ok(mix)
}

/// Today's AI mixes: made once a day (or when `refresh` is set) and kept in ai-mixes.json.
pub fn ai_mixes(ctx: &Ctx, refresh: bool) -> Result<Vec<crate::ai::AiMix>, String> {
    let state = &ctx.state;
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
    let result = (move || {
        crate::ai::daily_mixes(&input.settings, &input.taste, &input.candidates, &input.history)
    })();
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
pub fn play_tracks(ctx: &Ctx, ids: Vec<String>, start: usize, picked: Option<bool>) {
    let state = &ctx.state;
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
pub(crate) fn without_skipped(
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

pub fn enqueue(ctx: &Ctx, ids: Vec<String>, next: bool) {
    let state = &ctx.state;
    let items = state.queue_items(&ids);
    state.engine.send(Cmd::Enqueue { items, next });
}

pub fn player(ctx: &Ctx, action: String) {
    let state = &ctx.state;
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

pub fn seek(ctx: &Ctx, position: f64) {
    let state = &ctx.state;
    state.engine.send(Cmd::Seek(position));
}

pub fn jump(ctx: &Ctx, index: usize) {
    let state = &ctx.state;
    state.engine.send(Cmd::Jump(index));
}

pub fn remove_from_queue(ctx: &Ctx, index: usize) {
    let state = &ctx.state;
    state.engine.send(Cmd::Remove(index));
}

pub fn set_volume(ctx: &Ctx, volume: f32) {
    let state = &ctx.state;
    state.engine.send(Cmd::SetVolume(volume));
    state.settings.lock().volume = volume;
    state.save_settings();
}

pub fn set_repeat(ctx: &Ctx, repeat: Repeat) {
    let state = &ctx.state;
    state.engine.send(Cmd::SetRepeat(repeat));
    state.settings.lock().repeat = repeat;
    state.save_settings();
}

pub fn get_status(ctx: &Ctx) -> Status {
    let state = &ctx.state;
    state.engine.status()
}

// ---------- sound (EQ, leveling, crossfeed) ----------

#[derive(Serialize)]
pub struct Preset {
    name: &'static str,
    gains: [f64; 10],
}

pub fn get_dsp(ctx: &Ctx) -> DspSettings {
    let state = &ctx.state;
    state.settings.lock().dsp.clone()
}

pub fn set_dsp(ctx: &Ctx, dsp: DspSettings) {
    let state = &ctx.state;
    state.engine.send(Cmd::SetDsp(dsp.clone()));
    state.settings.lock().dsp = dsp;
    state.save_settings();
}

pub fn eq_presets(_ctx: &Ctx) -> Vec<Preset> {
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
pub fn parse_eq_file(_ctx: &Ctx, text: Option<String>, path: Option<String>) -> Result<ParametricImport, String> {
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

pub fn get_history(ctx: &Ctx) -> crate::history::HistoryStats {
    let state = &ctx.state;
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

pub fn get_app_settings(ctx: &Ctx) -> AppSettings {
    let state = &ctx.state;
    let s = state.settings.lock();
    AppSettings {
        close_to_tray: s.close_to_tray,
        lyrics_online: s.lyrics_online,
        fix_tags_online: s.fix_tags_online,
        listenbrainz_user: s.listenbrainz_user.clone(),
    }
}

pub fn set_lyrics_online(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().lyrics_online = enabled;
    state.save_settings();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skips {
    ids: Vec<String>,
    instrumentals: bool,
}

pub fn get_skips(ctx: &Ctx) -> Skips {
    let state = &ctx.state;
    let s = state.settings.lock();
    Skips {
        ids: s.skipped.iter().cloned().collect(),
        instrumentals: s.skip_instrumentals,
    }
}

/// Marks songs to be skipped (or not). Newly skipped songs also leave the queue; if one is
/// playing, Cadence moves on to the next song.
pub fn set_skipped(ctx: &Ctx, ids: Vec<String>, skip: bool) {
    let state = &ctx.state;
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

pub fn set_skip_instrumentals(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().skip_instrumentals = enabled;
    state.save_settings();
}

pub fn set_fix_tags_online(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().fix_tags_online = enabled;
    state.save_settings();
}

/// Lyrics for a song: from the file or a .lrc next to it, else LRCLIB (if allowed). Placed on the
/// singing when Cadence has listened to the song (see `voice`).
pub fn get_lyrics(ctx: &Ctx, track_id: String) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let track = track_of(&state, &track_id)?;
    let (state, voice) = (state.clone(), voice.clone());
    Ok(voice.apply(&track, resolve_lyrics(&state, &track)))
}

/// The lyrics the lyrics view shows for a song (before placing them on the singing). Blocking:
/// may ask LRCLIB.
pub(crate) fn resolve_lyrics(state: &AppState, track: &crate::library::Track) -> crate::lyrics::Lyrics {
    let online = state.settings.lock().lyrics_online;
    let cache = state.data_dir.join("lyrics");
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
            crate::lyrics::online(&cache, &track.id, &track.artist, &track.title, &track.album, track.duration)
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
}

/// Whether Cadence can follow the singer here, the download, and what it's listening to.
pub fn voice_status(ctx: &Ctx) -> crate::voice::Status {
    let state = &ctx.state;
    let voice = &ctx.voice;
    voice.status(state.settings.lock().voice_auto)
}

/// Downloads the voice models (progress as "voice-install" events).
pub fn voice_install(ctx: &Ctx) {
    crate::voice::install(ctx);
}

pub fn voice_remove(ctx: &Ctx) {
    let voice = &ctx.voice;
    crate::voice::remove(&voice);
}

pub fn voice_set_auto(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().voice_auto = enabled;
    state.save_settings();
}

/// Listens to a song so its lyrics follow the singer ("voice-progress", then "voice-done").
pub fn voice_follow(ctx: &Ctx, track_id: String) {
    crate::voice::request(ctx, track_id, resolve_lyrics);
}

/// Follows the singer in every song with lyrics ("voice-batch" events tell how it goes).
pub fn voice_batch(ctx: &Ctx) {
    crate::voice::start_batch(ctx, resolve_lyrics);
}

/// The app is changing pages (or scrolling): background listening gives way for a moment.
pub fn ui_busy(ctx: &Ctx) {
    ctx.voice.ui_busy(1_200);
}

pub fn voice_batch_stop(ctx: &Ctx) {
    crate::voice::stop_batch(ctx);
}

/// Turns following the singer on or off for one song; returns its lyrics as now shown.
pub fn voice_use(ctx: &Ctx, track_id: String, on: bool) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let track = track_of(&state, &track_id)?;
    let (state, voice) = (state.clone(), voice.clone());
    voice.set_on(&track.id, on);
    Ok(voice.apply(&track, resolve_lyrics(&state, &track)))
}

pub fn set_close_to_tray(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().close_to_tray = enabled;
    state.save_settings();
}

// ---------- playlists ----------

pub fn get_playlists(ctx: &Ctx) -> Vec<crate::playlists::Playlist> {
    let state = &ctx.state;
    state.playlists.lock().items.clone()
}

/// Creates (empty id) or updates a playlist; returns it with its id.
pub fn save_playlist(ctx: &Ctx, playlist: crate::playlists::Playlist) -> crate::playlists::Playlist {
    let state = &ctx.state;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let saved = state.playlists.lock().upsert(playlist, now);
    state.save_playlists();
    saved
}

pub fn delete_playlist(ctx: &Ctx, id: String) {
    let state = &ctx.state;
    state.playlists.lock().remove(&id);
    state.save_playlists();
}

// ---------- scrobbling ----------

/// Checks the token with ListenBrainz, then stores it in Credential Manager. Returns the user name.
pub fn connect_listenbrainz(ctx: &Ctx, token: String) -> Result<String, String> {
    let state = &ctx.state;
    let t = token.clone();
    let user = (move || crate::scrobble::validate(&t))()?;
    crate::secrets::set(crate::scrobble::TOKEN_SECRET, &token)?;
    state.settings.lock().listenbrainz_user = user.clone();
    state.save_settings();
    Ok(user)
}

pub fn disconnect_listenbrainz(ctx: &Ctx) -> Result<(), String> {
    let state = &ctx.state;
    crate::secrets::set(crate::scrobble::TOKEN_SECRET, "")?;
    state.settings.lock().listenbrainz_user.clear();
    state.save_settings();
    Ok(())
}

// ---------- tag editing ----------

#[derive(Serialize)]
pub struct TagsSaved {
    saved: usize,
    /// Songs playing now: shown changed already, their files are written when they stop.
    later: usize,
}

/// Writes the changed fields into each song's file, then rescans so the library shows them.
pub fn write_tags(ctx: &Ctx, ids: Vec<String>, changes: crate::tagedit::TagChanges) -> Result<TagsSaved, String> {
    let state = &ctx.state;
    crate::tagedit::check(&changes)?;
    let playing = {
        let s = state.engine.status();
        s.index.and_then(|i| s.queue.get(i).cloned())
    };
    let songs: Vec<(String, String)> = {
        let lib = state.library.lock();
        ids.iter()
            .filter_map(|id| {
                lib.by_id
                    .get(id)
                    .map(|&i| (id.clone(), lib.snapshot.tracks[i].path.clone()))
            })
            .collect()
    };
    let (later, now): (Vec<_>, Vec<_>) = songs
        .into_iter()
        .partition(|(id, _)| Some(id) == playing.as_ref());

    // The playing song's file can't be rewritten under the decoder: show the changes now and
    // write them when the song stops (held on disk, in case Cadence closes first).
    for (id, path) in &later {
        crate::tagedit::hold(&state.data_dir, path, &changes);
        {
            let mut lib = state.library.lock();
            if let Some(&i) = lib.by_id.get(id) {
                crate::tagedit::show(&mut lib.snapshot.tracks[i], &changes);
                let mut tracks = lib.snapshot.tracks.clone();
                let albums = crate::library::build_albums(&mut tracks, &state.art_dir());
                lib.set_tracks(tracks, albums);
            }
        }
        let (ctx2, dir, path) = (ctx.clone(), state.data_dir.clone(), path.clone());
        crate::autotag::after_song(
            id,
            Box::new(move || match crate::tagedit::write_held(&dir, &path) {
                Ok(true) => start_scan(ctx2),
                Ok(false) => {}
                Err(e) => log::warn!("{e}"),
            }),
        );
    }
    if !later.is_empty() {
        state.save_library();
        ctx.emit("library-changed", ());
    }

    let paths: Vec<String> = now.into_iter().map(|(_, p)| p).collect();
    let written = (move || -> Result<usize, String> {
        for p in &paths {
            crate::tagedit::write(Path::new(p), &changes)?;
        }
        Ok(paths.len())
    })()?;
    if written > 0 {
        start_scan(ctx.clone());
    }
    Ok(TagsSaved {
        saved: written + later.len(),
        later: later.len(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongInfoResults {
    term: String,
    results: Vec<crate::autotag::Candidate>,
}

/// Fix Song Info: catalogue matches for a song (searching `query`, or its name).
pub fn find_song_info(ctx: &Ctx, id: String, query: Option<String>) -> Result<SongInfoResults, String> {
    let state = &ctx.state;
    let track = track_of(&state, &id)?;
    (move || {
        let term = query
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .unwrap_or_else(|| crate::autotag::search_term(&track));
        let results = crate::autotag::candidates(&track, &term)?;
        Ok(SongInfoResults { term, results })
    })()
}

/// Saves the match picked in Fix Song Info; true when the file is written after the song ends.
pub fn save_song_info(ctx: &Ctx, id: String, info: crate::autotag::Candidate) -> Result<bool, String> {
    (move || crate::autotag::save_choice(ctx, &id, info))()
}

/// Lyrics lines in Latin letters (Japanese romaji, Chinese pinyin, Korean); empty for lines
/// that need none.
pub fn romanize_lyrics(_ctx: &Ctx, lines: Vec<String>) -> Result<Vec<String>, String> {
    Ok(crate::romanize::lines(&lines))
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
pub fn find_covers(ctx: &Ctx, album_id: String, query: Option<String>) -> Result<CoverResults, String> {
    let state = &ctx.state;
    let (title, artist) = {
        let lib = state.library.lock();
        let a = lib.snapshot.albums.iter().find(|a| a.id == album_id).ok_or("Unknown album")?;
        (a.title.clone(), a.artist.clone())
    };
    (move || {
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
    })()
}

/// Sets an album's cover (shown at once, and written into its songs' files).
pub fn set_album_cover(ctx: &Ctx, album_id: String, source: crate::artists::Source) -> Result<crate::covers::Changed, String> {
    (move || crate::covers::set(ctx, &album_id, source))()
}

// ---------- artist pictures ----------

pub fn artist_images(ctx: &Ctx) -> std::collections::HashMap<String, crate::artists::ArtistImage> {
    let state = &ctx.state;
    crate::artists::all(&state.data_dir)
}

/// Photos of this artist from the online catalogue.
pub fn find_artist_photos(_ctx: &Ctx, name: String) -> Result<Vec<crate::artists::Photo>, String> {
    (move || crate::artists::search(&name))()
}

pub fn set_artist_image(ctx: &Ctx, name: String, source: crate::artists::Source) -> Result<crate::artists::ArtistImage, String> {
    let state = &ctx.state;
    let (data, art) = (state.data_dir.clone(), state.art_dir());
    (move || crate::artists::set(&data, &art, &name, source))()
}

pub fn reset_artist_image(ctx: &Ctx, name: String) {
    let state = &ctx.state;
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
pub fn search_lyrics(_ctx: &Ctx, query: String) -> Result<Vec<crate::lyrics::Hit>, String> {
    (move || crate::lyrics::search(&query))()
}

/// Uses an LRCLIB search result as this song's lyrics (kept for next time).
pub fn use_lyrics(ctx: &Ctx, track_id: String, hit_id: u64) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let cache = state.data_dir.join("lyrics");
    let track = track_of(&state, &track_id)?;
    let voice = voice.clone();
    (move || {
        let l = crate::lyrics::fetch(hit_id)?;
        crate::lyrics::pick(&cache, &track_id, &l);
        Ok(voice.apply(&track, l))
    })()
}

/// Marks this song's lyrics as wrong, so they aren't shown (or looked up) again.
pub fn hide_lyrics(ctx: &Ctx, track_id: String) {
    let state = &ctx.state;
    crate::lyrics::hide(&state.data_dir.join("lyrics"), &track_id);
}

/// Uses pasted text (LRC or plain) as this song's lyrics.
pub fn paste_lyrics(ctx: &Ctx, track_id: String, text: String) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let l = crate::lyrics::from_pasted(&text).ok_or("There’s no text to use.")?;
    crate::lyrics::pick(&state.data_dir.join("lyrics"), &track_id, &l);
    let track = track_of(&state, &track_id)?;
    Ok(voice.apply(&track, l))
}

/// Lets the AI find lyrics: it suggests better searches for LRCLIB (for time-synced lyrics) and
/// looks the lyrics up on the web as a fallback.
pub fn ai_find_lyrics(ctx: &Ctx, track_id: String) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let track = track_of(&state, &track_id)?;
    let settings = state.settings.lock().ai.clone();
    let cache = state.data_dir.join("lyrics");
    let voice = voice.clone();
    (move || {
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
                        return Ok(voice.apply(&track, l));
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
        Ok(voice.apply(&track, l))
    })()
}

/// Diagnostics: records what goes to the device into `%TEMP%\cadence-tap.f32` while on.
pub fn debug_tap(ctx: &Ctx, on: bool) -> String {
    let state = &ctx.state;
    state.engine.tap(on);
    std::env::temp_dir().join("cadence-tap.f32").display().to_string()
}

/// Splits a genre tag into genres ("J-Pop; Anime" → two), trimmed.
fn genres_of(tag: &str) -> impl Iterator<Item = &str> {
    tag.split([';', '/', ',', '|']).map(str::trim).filter(|g| g.len() >= 2)
}

/// Adds up plays per name, case-insensitively (the first spelling seen is kept), most first.
fn tally<'a>(items: impl Iterator<Item = (&'a str, u32)>) -> Vec<crate::ai::Count> {
    let mut by: HashMap<String, crate::ai::Count> = HashMap::new();
    for (name, n) in items {
        by.entry(name.to_lowercase())
            .or_insert_with(|| crate::ai::Count { name: name.to_string(), plays: 0 })
            .plays += n;
    }
    let mut v: Vec<_> = by.into_values().collect();
    v.sort_by(|a, b| b.plays.cmp(&a.plays).then_with(|| a.name.cmp(&b.name)));
    v.truncate(30);
    v
}

/// What the listener plays most: in the last 30 days ("this month") and all time, by artist,
/// genre and song. A play counts when half the song (or 30 s) was heard and it wasn't skipped.
pub fn listening_profile(ctx: &Ctx) -> crate::ai::Profile {
    let state = &ctx.state;
    let since = crate::history::now().saturating_sub(30 * 86_400);
    let (month, all) = {
        let h = state.history.lock();
        let mut m: HashMap<String, u32> = HashMap::new();
        for e in h.events.iter().filter(|e| e.at >= since && !e.skipped && (e.listened >= 30.0 || e.listened >= e.duration * 0.5)) {
            *m.entry(e.track_id.clone()).or_insert(0) += 1;
        }
        (m, h.stats().plays)
    };
    let lib = state.library.lock();
    let track = |id: &str| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]);
    let m: Vec<_> = month.iter().filter_map(|(id, &n)| track(id).map(|t| (t, n))).collect();
    let a: Vec<_> = all.iter().filter_map(|(id, &n)| track(id).map(|t| (t, n))).collect();
    let mut songs: Vec<crate::ai::Count> = m
        .iter()
        .map(|(t, n)| crate::ai::Count { name: format!("{} - {}", t.artist, t.title), plays: *n })
        .collect();
    songs.sort_by(|x, y| y.plays.cmp(&x.plays).then_with(|| x.name.cmp(&y.name)));
    songs.truncate(30);
    crate::ai::Profile {
        month_artists: tally(m.iter().map(|(t, n)| (t.artist.as_str(), *n))),
        month_genres: tally(m.iter().flat_map(|(t, n)| genres_of(&t.genre).map(move |g| (g, *n)))),
        month_songs: songs,
        artists: tally(a.iter().map(|(t, n)| (t.artist.as_str(), *n))),
        // Every genre in the library, weighted by plays (+1, so unplayed genres are listed too).
        genres: tally(lib.snapshot.tracks.iter().flat_map(|t| {
            let n = all.get(&t.id).copied().unwrap_or(0) + 1;
            genres_of(&t.genre).map(move |g| (g, n))
        })),
    }
}

/// A song, artist or album in the listening stats.
#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatItem {
    /// Track or album id; the name for an artist.
    pub id: String,
    pub name: String,
    /// The artist (songs, albums).
    pub sub: String,
    /// Plays heard to the end.
    pub finished: u32,
    /// All plays, finished or not.
    pub plays: u32,
    pub minutes: u32,
}

/// What was played in a time range ("week", "month", "year", "all"), and what most often to the end.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListeningStats {
    pub range: String,
    pub plays: u32,
    /// Heard to the end (90% of the song, or within 10 s of the end, and not skipped).
    pub finished: u32,
    pub skipped: u32,
    pub minutes: u32,
    pub songs: u32,
    pub artists: u32,
    pub albums: u32,
    pub top_songs: Vec<StatItem>,
    pub top_artists: Vec<StatItem>,
    pub top_albums: Vec<StatItem>,
    /// Listening minutes per day (week, month) or per 30 days (year), oldest first; empty for "all".
    pub timeline: Vec<u32>,
}

pub fn listening_stats(ctx: &Ctx, range: String) -> ListeningStats {
    let state = &ctx.state;
    let now = crate::history::now();
    let (days, bucket) = match range.as_str() {
        "week" => (7, 86_400),
        "month" => (30, 86_400),
        "year" => (365, 30 * 86_400),
        _ => (0, 0),
    };
    let since = if days == 0 { 0 } else { now.saturating_sub(days * 86_400) };
    let events: Vec<crate::history::PlayEvent> = state.history.lock().events.iter().filter(|e| e.at >= since).cloned().collect();
    let lib = state.library.lock();
    let mut out = ListeningStats { range: range.clone(), ..Default::default() };
    if bucket > 0 {
        out.timeline = vec![0; (days * 86_400 / bucket) as usize];
    }
    let mut songs: HashMap<String, StatItem> = HashMap::new();
    let mut artists: HashMap<String, StatItem> = HashMap::new();
    let mut albums: HashMap<String, StatItem> = HashMap::new();
    let album_of: HashMap<&str, &crate::library::Album> = lib.snapshot.albums.iter().map(|a| (a.id.as_str(), a)).collect();
    let mut seconds = 0.0;
    let mut timeline = vec![0.0f64; out.timeline.len()];
    for e in &events {
        let Some(t) = lib.by_id.get(&e.track_id).map(|&i| &lib.snapshot.tracks[i]) else { continue };
        let finished = !e.skipped && (e.duration <= 0.0 || e.listened >= e.duration * 0.9 || e.duration - e.listened <= 10.0);
        let heard = e.listened.max(0.0);
        out.plays += 1;
        out.finished += finished as u32;
        out.skipped += e.skipped as u32;
        seconds += heard;
        if let Some(last) = timeline.len().checked_sub(1) {
            timeline[(((e.at - since) / bucket) as usize).min(last)] += heard;
        }
        // `minutes` holds seconds until the end.
        let add = |map: &mut HashMap<String, StatItem>, id: &str, name: &str, sub: &str| {
            let s = map
                .entry(id.to_lowercase())
                .or_insert_with(|| StatItem { id: id.to_string(), name: name.to_string(), sub: sub.to_string(), ..Default::default() });
            s.plays += 1;
            s.finished += finished as u32;
            s.minutes += heard.round() as u32;
        };
        add(&mut songs, &t.id, &t.title, &t.artist);
        add(&mut artists, &t.artist, &t.artist, "");
        if let Some(a) = album_of.get(t.album_id.as_str()) {
            add(&mut albums, &a.id, &a.title, &a.artist);
        }
    }
    out.minutes = (seconds / 60.0).round() as u32;
    out.timeline = timeline.iter().map(|s| (s / 60.0).round() as u32).collect();
    out.songs = songs.len() as u32;
    out.artists = artists.len() as u32;
    out.albums = albums.len() as u32;
    let top = |map: HashMap<String, StatItem>| {
        let mut v: Vec<StatItem> = map
            .into_values()
            .map(|mut s| {
                s.minutes = (s.minutes as f64 / 60.0).round() as u32;
                s
            })
            .collect();
        v.sort_by(|a, b| b.finished.cmp(&a.finished).then(b.minutes.cmp(&a.minutes)).then_with(|| a.name.cmp(&b.name)));
        v.truncate(50);
        v
    };
    out.top_songs = top(songs);
    out.top_artists = top(artists);
    out.top_albums = top(albums);
    out
}

/// The last Discover search, shown again when the page opens (without what the library has now).
pub fn ai_recommended(ctx: &Ctx) -> crate::ai::RecommendCache {
    let mut last: crate::ai::RecommendCache = crate::state::read_json(&ctx.state.data_dir.join("ai-recommend.json"));
    last.picks = ai_still_new(ctx, last.picks);
    last
}

/// Suggestions the library still doesn't have: the UI asks again when the library changes, so
/// songs downloaded since drop out of Discover and Home.
pub fn ai_still_new(ctx: &Ctx, picks: Vec<crate::ai::Discovery>) -> Vec<crate::ai::Discovery> {
    crate::catalog::still_new(picks, &ctx.state.library.lock().snapshot.tracks)
}

/// A song as Discover remembers it ("artist|title", lower-case).
fn seen_key(d: &crate::ai::Discovery) -> String {
    format!("{}|{}", d.artist.trim().to_lowercase(), d.title.trim().to_lowercase())
}

/// Songs Discover suggested before, oldest first: its own list (ai-recommend-seen.json), plus what
/// earlier versions suggested before that list was kept (the last search, Home's old daily picks).
fn seen_songs(dir: &std::path::Path) -> Vec<String> {
    let mut seen: Vec<String> = crate::state::read_json(&dir.join("ai-recommend-seen.json"));
    for name in ["ai-recommend.json", "ai-discover.json"] {
        let old: crate::ai::RecommendCache = crate::state::read_json(&dir.join(name));
        for d in &old.picks {
            let k = seen_key(d);
            if !seen.contains(&k) {
                seen.insert(0, k);
            }
        }
    }
    seen
}

/// One Discover search: new songs (not in the library, not on an album in it) found by the AI on
/// the web, with how similar and how well rated each is. A song suggested before comes back only
/// when it's rated `REPEAT_AT` or better. What it found is remembered (ai-recommend-seen.json).
fn recommend_now(ctx: &Ctx, basis: &str, value: &str) -> Result<crate::ai::RecommendCache, String> {
    let state = &ctx.state;
    let profile = listening_profile(ctx);
    let input = dj_input(state, value);
    let library = state.library.lock().snapshot.tracks.clone();
    let mut owned: Vec<String> = library.iter().map(|t| t.artist.clone()).collect();
    owned.sort();
    owned.dedup();
    let seen_file = state.data_dir.join("ai-recommend-seen.json");
    let mut seen = seen_songs(&state.data_dir);
    let before: Vec<String> = seen.iter().rev().take(60).map(|k| k.replacen('|', " - ", 1)).collect();
    let known: HashSet<&str> = seen.iter().map(String::as_str).collect();
    let picks = crate::ai::recommend(&input.settings, basis, value, &input.taste, &profile, &owned, &before)?;
    let fresh: Vec<_> = picks
        .into_iter()
        .filter(|d| !known.contains(seen_key(d).as_str()) || d.rating >= crate::ai::REPEAT_AT)
        .collect();
    let picks = crate::catalog::check(fresh, &library);
    if picks.is_empty() {
        return Err("Everything the DJ found you already have, or it suggested before. Try again or ask another way.".into());
    }
    for d in &picks {
        let k = seen_key(d);
        if !seen.contains(&k) {
            seen.push(k);
        }
    }
    let excess = seen.len().saturating_sub(800);
    seen.drain(..excess);
    crate::state::write_json(&seen_file, &seen);
    Ok(crate::ai::RecommendCache { basis: basis.to_string(), value: value.to_string(), at: crate::history::now(), picks })
}

/// Discover: new songs for the listener's last 30 days ("month"), an artist, a genre, or a request
/// in words ("search"). The last one is kept in ai-recommend.json, to show again.
pub fn ai_recommend(ctx: &Ctx, basis: String, value: String) -> Result<crate::ai::RecommendCache, String> {
    let value = value.trim().to_string();
    if basis != "month" && value.is_empty() {
        return Err("Pick or type something to base it on.".into());
    }
    let result = recommend_now(ctx, &basis, &value)?;
    crate::state::write_json(&ctx.state.data_dir.join("ai-recommend.json"), &result);
    Ok(result)
}

/// Today's picks, kept in ai-recommend-daily.json.
#[derive(Default, Serialize, serde::Deserialize)]
struct DailyPicks {
    day: u64,
    picks: crate::ai::RecommendCache,
    /// Why today's attempt failed; not tried again automatically until tomorrow.
    error: String,
}

/// Made once at a time: Home and Discover may both ask first thing in the morning.
static DAILY: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Discover's picks for the day: new songs for what the listener plays this month, made once a day
/// (or when `refresh` is set) and kept, so Discover and Home's "New music for you" open at once.
pub fn ai_daily_picks(ctx: &Ctx, refresh: bool) -> Result<crate::ai::RecommendCache, String> {
    let _one = DAILY.lock();
    let file = ctx.state.data_dir.join("ai-recommend-daily.json");
    let mut cache: DailyPicks = crate::state::read_json(&file);
    if !refresh && cache.day == crate::ai::today() {
        if !cache.picks.picks.is_empty() {
            // Without what the library has now: downloaded since, or there under other names
            // (looked up once for picks saved without them). When it has all of them, new ones.
            let library = ctx.state.library.lock().snapshot.tracks.clone();
            if crate::catalog::add_other_names(&mut cache.picks.picks, &library) {
                crate::state::write_json(&file, &cache);
            }
            let mut today = cache.picks;
            today.picks = crate::catalog::still_new(today.picks, &library);
            if !today.picks.is_empty() {
                return Ok(today);
            }
        } else if !cache.error.is_empty() {
            return Err(cache.error);
        }
    }
    let result = recommend_now(ctx, "month", "");
    crate::state::write_json(
        &file,
        &DailyPicks {
            day: crate::ai::today(),
            picks: result.clone().unwrap_or_default(),
            error: result.as_ref().err().cloned().unwrap_or_default(),
        },
    );
    result
}

/// Home's "New music for you": today's Discover picks (one search a day for both).
pub fn ai_discover(ctx: &Ctx, refresh: bool) -> Result<Vec<crate::ai::Discovery>, String> {
    ai_daily_picks(ctx, refresh).map(|r| r.picks)
}

/// Every command, for both front ends (commands.rs for Tauri, ffi.rs for the native app).
/// `plain` returns a value, `fallible` a Result, `blocking` a Result after slow work (network,
/// files, AI), which Tauri runs on a worker thread. Argument names are the UI's, in camelCase.
#[macro_export]
macro_rules! cadence_commands {
    ($m:ident) => {
        $m! {
            plain get_library() -> $crate::library::Snapshot;
            plain get_folders() -> Vec<String>;
            plain add_folders(paths: Vec<String>) -> Vec<String>;
            fallible open_file(path: String) -> $crate::api::Opened;
            blocking download_update_apk(url: String) -> String;
            fallible list_dirs(path: Option<String>, volumes: Option<Vec<$crate::api::Volume>>) -> Vec<$crate::api::DirEntry>;
            plain remove_folder(path: String) -> Vec<String>;
            plain rescan() -> ();
            plain reanalyze() -> ();
            plain sonic_status() -> $crate::api::SonicStatus;
            plain similar_tracks(id: String, limit: Option<usize>) -> Vec<String>;
            plain sound_radio(id: String, count: Option<usize>) -> Vec<String>;
            plain sound_mixes() -> Vec<$crate::sonic::Mix>;
            plain get_ai_settings() -> $crate::api::AiInfo;
            plain set_ai_settings(settings: $crate::ai::AiSettings) -> ();
            fallible set_ai_key(text: String) -> $crate::api::AiInfo;
            blocking ai_test() -> String;
            plain ai_preview(request: String) -> String;
            blocking ai_ask(request: String) -> $crate::ai::AiMix;
            blocking ai_mixes(refresh: bool) -> Vec<$crate::ai::AiMix>;
            blocking ai_discover(refresh: bool) -> Vec<$crate::ai::Discovery>;
            plain listening_profile() -> $crate::ai::Profile;
            plain ui_busy() -> ();
            plain listening_stats(range: String) -> $crate::api::ListeningStats;
            plain ai_recommended() -> $crate::ai::RecommendCache;
            plain ai_still_new(picks: Vec<$crate::ai::Discovery>) -> Vec<$crate::ai::Discovery>;
            blocking ai_daily_picks(refresh: bool) -> $crate::ai::RecommendCache;
            blocking ai_recommend(basis: String, value: String) -> $crate::ai::RecommendCache;
            plain play_tracks(ids: Vec<String>, start: usize, picked: Option<bool>) -> ();
            plain enqueue(ids: Vec<String>, next: bool) -> ();
            plain player(action: String) -> ();
            plain seek(position: f64) -> ();
            plain jump(index: usize) -> ();
            plain remove_from_queue(index: usize) -> ();
            plain set_volume(volume: f32) -> ();
            plain set_repeat(repeat: $crate::audio::Repeat) -> ();
            plain get_status() -> $crate::audio::Status;
            plain get_dsp() -> $crate::audio::DspSettings;
            plain set_dsp(dsp: $crate::audio::DspSettings) -> ();
            plain eq_presets() -> Vec<$crate::api::Preset>;
            fallible parse_eq_file(text: Option<String>, path: Option<String>) -> $crate::api::ParametricImport;
            plain get_history() -> $crate::history::HistoryStats;
            plain get_app_settings() -> $crate::api::AppSettings;
            plain set_close_to_tray(enabled: bool) -> ();
            plain set_lyrics_online(enabled: bool) -> ();
            plain set_fix_tags_online(enabled: bool) -> ();
            plain get_skips() -> $crate::api::Skips;
            plain set_skipped(ids: Vec<String>, skip: bool) -> ();
            plain set_skip_instrumentals(enabled: bool) -> ();
            blocking get_lyrics(track_id: String) -> $crate::lyrics::Lyrics;
            plain voice_status() -> $crate::voice::Status;
            plain voice_install() -> ();
            plain voice_remove() -> ();
            plain voice_set_auto(enabled: bool) -> ();
            plain voice_follow(track_id: String) -> ();
            plain voice_batch() -> ();
            plain voice_batch_stop() -> ();
            blocking voice_use(track_id: String, on: bool) -> $crate::lyrics::Lyrics;
            plain get_playlists() -> Vec<$crate::playlists::Playlist>;
            plain save_playlist(playlist: $crate::playlists::Playlist) -> $crate::playlists::Playlist;
            plain delete_playlist(id: String) -> ();
            blocking connect_listenbrainz(token: String) -> String;
            fallible disconnect_listenbrainz() -> ();
            blocking write_tags(ids: Vec<String>, changes: $crate::tagedit::TagChanges) -> $crate::api::TagsSaved;
            blocking find_song_info(id: String, query: Option<String>) -> $crate::api::SongInfoResults;
            blocking save_song_info(id: String, info: $crate::autotag::Candidate) -> bool;
            blocking romanize_lyrics(lines: Vec<String>) -> Vec<String>;
            blocking find_covers(album_id: String, query: Option<String>) -> $crate::api::CoverResults;
            blocking set_album_cover(album_id: String, source: $crate::artists::Source) -> $crate::covers::Changed;
            plain artist_images() -> std::collections::HashMap<String, $crate::artists::ArtistImage>;
            blocking find_artist_photos(name: String) -> Vec<$crate::artists::Photo>;
            blocking set_artist_image(name: String, source: $crate::artists::Source) -> $crate::artists::ArtistImage;
            plain reset_artist_image(name: String) -> ();
            blocking search_lyrics(query: String) -> Vec<$crate::lyrics::Hit>;
            blocking use_lyrics(track_id: String, hit_id: u64) -> $crate::lyrics::Lyrics;
            plain hide_lyrics(track_id: String) -> ();
            fallible paste_lyrics(track_id: String, text: String) -> $crate::lyrics::Lyrics;
            blocking ai_find_lyrics(track_id: String) -> $crate::lyrics::Lyrics;
            plain debug_tap(on: bool) -> String;
        }
    };
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
