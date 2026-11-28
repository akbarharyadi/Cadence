//! Everything the UI can ask of Cadence, as plain functions over `Ctx`. The Tauri app wraps them
//! as commands (commands.rs) from the list at the end (`cadence_commands!`). Slow ones block, and
//! Tauri runs them on a worker thread.

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

/// Only some songs and their albums (the mini player asks for the song playing, not the library).
pub fn get_library_part(ctx: &Ctx, track_ids: Vec<String>) -> Snapshot {
    ctx.state.library.lock().part(&track_ids)
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
    // Songs are known by their path (see library::read_track), except those of a single-file
    // album (CUE sheet): the file's first song plays.
    let id = {
        let lib = state.library.lock();
        lib.snapshot
            .tracks
            .iter()
            .filter(|t| t.path == key && t.cue.is_some())
            .min_by(|a, b| a.cue.map_or(0.0, |c| c.start).total_cmp(&b.cue.map_or(0.0, |c| c.start)))
            .map(|t| t.id.clone())
            .unwrap_or_else(|| crate::library::short_hash(&key))
    };
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

// ---------- waveform seek bar ----------

/// The song's waveform (`waveform` module's file format), or empty: not drawn yet ("waveform-ready"
/// follows once it is) or it can't be.
pub fn waveform(ctx: &Ctx, track_id: String) -> Vec<u8> {
    crate::waveform::for_track(ctx, &track_id)
}

/// Settings > "Prepare waveforms for all songs". False: already running.
pub fn waveforms_prepare(ctx: &Ctx) -> bool {
    crate::waveform::prepare_all(ctx)
}

pub fn waveforms_pause(ctx: &Ctx, paused: bool) {
    crate::waveform::pause(ctx, paused);
}

pub fn waveforms_stop(ctx: &Ctx) {
    crate::waveform::stop(ctx);
}

pub fn waveforms_status(_ctx: &Ctx) -> crate::waveform::JobStatus {
    crate::waveform::status()
}

// ---------- sync with phone (phone::commands) ----------

pub fn phone_summary(ctx: &Ctx) -> Result<crate::phone::Summary, String> {
    Ok(crate::phone::commands::summary(ctx))
}

pub fn phone_kept(ctx: &Ctx) -> crate::phone::commands::Kept {
    crate::phone::commands::kept(ctx)
}

pub fn phone_keep(ctx: &Ctx, kind: crate::phone::Kind, id: String, keep: bool) {
    crate::phone::commands::keep(ctx, kind, &id, keep);
}

pub fn phone_set_top(ctx: &Ctx, top: u32) {
    crate::phone::commands::set_top(ctx, top);
}

pub fn phone_service(ctx: &Ctx) -> crate::phone::commands::Service {
    crate::phone::commands::service(ctx)
}

pub fn set_phone_sync(ctx: &Ctx, enabled: bool) -> crate::phone::commands::Service {
    crate::phone::commands::set_enabled(ctx, enabled)
}

pub fn phone_pair(ctx: &Ctx) -> Result<crate::phone::commands::Service, String> {
    crate::phone::commands::pair(ctx)
}

pub fn phone_unpair(ctx: &Ctx, phone_id: String) -> Result<crate::phone::commands::Service, String> {
    crate::phone::commands::unpair(ctx, &phone_id)
}

pub fn phone_pair_offer(_ctx: &Ctx, link: String) -> Result<crate::phone::pair_link::Offer, String> {
    crate::phone::commands::pair_offer(&link)
}

pub fn phone_link(ctx: &Ctx) -> crate::phone::commands::LinkView {
    crate::phone::commands::link_view(ctx)
}

pub fn phone_link_pair(ctx: &Ctx, address: String, code: String, name: String) -> Result<crate::phone::commands::LinkView, String> {
    crate::phone::commands::link_pair(ctx, &address, &code, &name)
}

pub fn phone_link_forget(ctx: &Ctx) -> Result<crate::phone::commands::LinkView, String> {
    crate::phone::commands::link_forget(ctx)
}

pub fn phone_sync_now(ctx: &Ctx) -> Result<bool, String> {
    crate::phone::commands::sync_now(ctx)
}

/// Pauses or resumes the job running on this device: preparing songs (computer) or a sync (phone).
pub fn phone_pause(ctx: &Ctx, paused: bool) {
    crate::phone::commands::pause(ctx, paused);
}

pub fn phone_sync_stop(ctx: &Ctx) {
    crate::phone::commands::sync_stop(ctx);
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
/// (A `blocking` command: it compares the song with every analysed one, off the window's thread.)
pub fn similar_tracks(ctx: &Ctx, id: String, limit: Option<usize>) -> Result<Vec<String>, String> {
    let state = &ctx.state;
    Ok(with_sonic(state, |items, lib| {
        let Some(&i) = lib.by_id.get(&id) else {
            return Vec::new();
        };
        crate::sonic::similar(items, &lib.snapshot.tracks[i], limit.unwrap_or(25))
            .into_iter()
            .map(|t| t.id.clone())
            .collect()
    }))
}

/// A radio of songs that sound like `id`, starting with it.
pub fn sound_radio(ctx: &Ctx, id: String, count: Option<usize>) -> Vec<String> {
    let state = &ctx.state;
    with_sonic(state, |items, lib| {
        let Some(&i) = lib.by_id.get(&id) else {
            return Vec::new();
        };
        crate::sonic::radio(items, &lib.snapshot.tracks[i], count.unwrap_or(50))
            .into_iter()
            .map(|t| t.id.clone())
            .collect()
    })
}

/// (`blocking`: clustering the whole library takes a while.)
pub fn sound_mixes(ctx: &Ctx) -> Result<Vec<crate::sonic::Mix>, String> {
    let state = &ctx.state;
    Ok(with_sonic(state, |items, _| crate::sonic::mixes(items)))
}

// ---------- AI DJ ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiInfo {
    settings: crate::ai::AiSettings,
    has_key: bool,
    /// On and able to answer (a key, or a server on this computer or the home network).
    ready: bool,
}

pub fn get_ai_settings(ctx: &Ctx) -> AiInfo {
    let state = &ctx.state;
    // One lock, released before the key lookup (a second lock in the same expression deadlocks).
    let settings = state.settings.lock().ai.clone();
    let has_key = crate::secrets::get(settings.service.key_name()).is_some();
    let ready = crate::ai::ready(&settings);
    AiInfo { settings, has_key, ready }
}

pub fn set_ai_settings(ctx: &Ctx, settings: crate::ai::AiSettings) {
    let state = &ctx.state;
    state.settings.lock().ai = settings;
    state.save_settings_later();
}

/// Saves the API key for the chosen service. Accepts the bare key or a pasted .env file
/// (ANTHROPIC_API_KEY for Claude; ZAI_API_KEY / AI_API_KEY / OPENAI_API_KEY, plus AI_MODEL and
/// AI_BASE_URL, for an OpenAI-compatible service). An Anthropic key (sk-ant-…) switches to Claude.
/// Returns the new settings.
pub fn set_ai_key(ctx: &Ctx, text: String) -> Result<AiInfo, String> {
    let state = &ctx.state;
    let mut vars = env_vars(&text);
    // One line with no variable Cadence knows: a key that happens to look like one.
    let known = ["ANTHROPIC_API_KEY", "ZAI_API_KEY", "AI_API_KEY", "OPENAI_API_KEY", "API_KEY", "AI_MODEL", "AI_BASE_URL"];
    if text.trim().lines().count() == 1 && !known.iter().any(|k| vars.contains_key(*k)) {
        vars.clear();
    }
    // The key goes only where it's safe to send it (checked before anything is saved).
    if let Some(u) = vars.get("AI_BASE_URL").filter(|v| !v.is_empty()) {
        crate::ai::check_url(u)?;
    }
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

/// The variables of a pasted .env file (`NAME=value`, `export NAME="value"`, # comments). Only
/// lines whose name looks like a variable count: a bare key with "=" in it (base64 padding) is a
/// key, not a file, and gives none.
fn env_vars(text: &str) -> HashMap<String, String> {
    let name_like = |k: &str| {
        let mut c = k.chars();
        c.next().is_some_and(|f| f.is_ascii_uppercase() || f == '_') && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().trim_start_matches("export ").trim(), v))
        .filter(|(k, _)| name_like(k))
        .map(|(k, v)| (k.to_string(), v.trim().trim_matches(['"', '\'']).to_string()))
        .collect()
}

/// Everything the DJ needs, copied out of the app state so the slow request runs without locks.
struct DjInput {
    settings: crate::ai::AiSettings,
    taste: String,
    candidates: Vec<crate::ai::Candidate>,
    history: crate::history::HistoryStats,
}

fn dj_input(state: &AppState, request: &str) -> DjInput {
    let mut input = dj_library(state);
    let all = std::mem::take(&mut input.candidates);
    input.candidates = crate::ai::shortlist(all, request, &input.history, crate::history::now());
    input
}

/// Everything the DJ could pick from: every song not skipped, with its sound words.
fn dj_library(state: &AppState) -> DjInput {
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
    crate::ai::test(&settings).map(|_| "Connected".to_string())
}

/// Exactly what would be sent for this request (shown in Settings).
/// (`blocking`: the library's facts may need working out first.)
pub fn ai_preview(ctx: &Ctx, request: String) -> Result<String, String> {
    let facts = crate::dj::facts(&ctx.state);
    Ok(crate::dj::preview(&facts, &request, &crate::ai::date_and_time_words()))
}

/// Suggestions for the DJ box from what the library has (its languages, decades and genres).
/// (`blocking`: worked out once per library, which may read the saved lyrics' languages.)
pub fn dj_suggestions(ctx: &Ctx) -> Result<Vec<String>, String> {
    Ok(crate::dj::suggestions(&crate::dj::facts(&ctx.state)))
}

/// "Ask the DJ" (dj.rs): the AI plans, Cadence picks the songs, the AI orders and names them.
/// Works without the AI too (a plan from the request's own words).
pub fn ai_ask(ctx: &Ctx, request: String) -> Result<crate::ai::AiMix, String> {
    let state = &ctx.state;
    let facts = crate::dj::facts(state);
    let settings = state.settings.lock().ai.clone();
    let history = state.history.lock().stats();
    let marks = state.marks.lock().view();
    let rule = state.skip_rule();
    let never: HashSet<String> = {
        let lib = state.library.lock();
        lib.snapshot.tracks.iter().filter(|t| rule.skips(t)).map(|t| t.id.clone()).collect()
    };
    let identity = crate::identity::current();
    let credits = || crate::searchkeys::current(ctx).credits.into_iter().collect::<Vec<_>>();
    let llm = crate::dj::AppLlm(settings.clone());
    let ready = crate::ai::ready(&settings);
    let now = crate::history::now();
    let mut mix = crate::dj::ask(
        &facts,
        &crate::dj::Ask {
            request: &request,
            taste: crate::dj::Taste { plays: &history.plays, skips: &history.skips, last: &history.last_played, marks: &marks, now },
            never: &never,
            identity: &identity,
            credits: &credits,
            llm: ready.then_some(&llm as &dyn crate::dj::Llm),
            ai_order: settings.dj_order,
            now_words: &crate::ai::date_and_time_words(),
            seed: crate::shuffle::clock_seed(),
        },
    )?;
    if !mix.discover.is_empty() {
        let library = state.library.lock().snapshot.tracks.clone();
        mix.discover = crate::catalog::check(std::mem::take(&mut mix.discover), &library);
    }
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
            // (Saved by an older version, two may share a title.)
            let mut mixes = cache.mixes;
            crate::ai::distinct_titles(&mut mixes);
            return Ok(mixes);
        }
        if !cache.error.is_empty() {
            return Err(cache.error);
        }
    }
    let today = crate::ai::today();
    let input = dj_library(state);
    // Songs from the last week's mixes go to the back of the line.
    let recent: Vec<(u64, Vec<String>)> = cache.recent.into_iter().filter(|(d, _)| d + 7 > today).collect();
    let clusters: Vec<(String, Vec<String>)> = with_sonic(state, |items, _| crate::sonic::mixes(items))
        .into_iter()
        .map(|m| (m.name, m.track_ids))
        .collect();
    let now = crate::history::now();
    let utc_hour = ((now / 3600) % 24) as i64;
    let mut utc_offset = (crate::ai::local_hour() as i64 - utc_hour).rem_euclid(24);
    if utc_offset > 12 {
        utc_offset -= 24;
    }
    let pools = {
        let history = state.history.lock();
        crate::lanes::pools(crate::lanes::Input {
            tracks: input.candidates,
            events: &history.events,
            now,
            utc_offset,
            clusters,
            recent: recent.iter().flat_map(|(_, ids)| ids.iter().cloned()).collect(),
            seed: now,
            day: today,
        })
    };
    // "New mixes": tell the DJ what it made last time so it doesn't make the same again.
    let avoid = if refresh && cache.day == today { cache.mixes } else { Vec::new() };
    let result = crate::ai::daily_mixes(&input.settings, &input.taste, &pools, &input.history, &avoid);
    let mut recent = recent;
    if let Ok(mixes) = &result {
        recent.push((today, mixes.iter().flat_map(|m| m.track_ids.iter().cloned()).collect()));
    }
    crate::state::write_json(
        &file,
        &crate::ai::DailyCache {
            day: today,
            mixes: result.clone().unwrap_or_default(),
            recent,
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

/// Takes everything after the playing song out of the queue.
pub fn clear_up_next(ctx: &Ctx) {
    ctx.state.engine.send(Cmd::ClearUpNext);
}

/// Moves a waiting song (queue positions, both after the playing song).
pub fn move_in_queue(ctx: &Ctx, from: usize, to: usize) {
    ctx.state.engine.send(Cmd::Move { from, to });
}

/// Puts songs back into the queue at a place (undoing a removal).
pub fn insert_in_queue(ctx: &Ctx, at: usize, ids: Vec<String>) {
    let items = ctx.state.queue_items(&ids);
    ctx.state.engine.send(Cmd::Insert { at, items });
}

/// The sleep timer. `kind`: "off", "minutes" (`amount` minutes, the sound fades out), "songs"
/// (pause after `amount` songs; 1 = the end of this one) or "album" (when the album ends).
pub fn set_sleep_timer(ctx: &Ctx, kind: String, amount: Option<u32>) {
    let state = &ctx.state;
    state.sleep.cancel();
    state.engine.send(Cmd::StopAfter(None));
    match kind.as_str() {
        "minutes" => state.sleep.start_minutes(ctx, amount.unwrap_or(30)),
        "songs" => state.engine.send(Cmd::StopAfter(Some(crate::audio::StopAfter::Songs { left: amount.unwrap_or(1).max(1) }))),
        "album" => state.engine.send(Cmd::StopAfter(Some(crate::audio::StopAfter::Album))),
        _ => {}
    }
    ctx.emit("sleep-timer", state.sleep.ends_at());
}

/// When the minutes countdown ends (unix seconds), if one is running.
pub fn get_sleep_timer(ctx: &Ctx) -> Option<u64> {
    ctx.state.sleep.ends_at()
}

/// Heard at once; saved once the slider has stopped for a moment (it sends ~60 changes a second
/// while dragged), and at exit. `device_gen` (optional): the `Status::device_gen` the window's
/// slider was showing; a change meant for a device playback has since left is dropped
/// (`profiles::set_volume`; the engine keeps it as the playing device's own).
pub fn set_volume(ctx: &Ctx, volume: f32, device_gen: Option<u64>) {
    let state = &ctx.state;
    crate::profiles::set_volume(state, volume, device_gen);
    state.save_settings_later();
}

pub fn set_repeat(ctx: &Ctx, repeat: Repeat) {
    let state = &ctx.state;
    state.engine.send(Cmd::SetRepeat(repeat));
    state.settings.lock().repeat = repeat;
    state.save_settings_later();
}

pub fn set_shuffle(ctx: &Ctx, on: bool) {
    let state = &ctx.state;
    state.engine.send(Cmd::SetShuffle(on));
    state.settings.lock().shuffle = on;
    state.save_settings_later();
}

/// Plays a whole list shuffled (the balanced or the fully random shuffle, as set); skipped songs
/// are left out.
pub fn play_shuffled(ctx: &Ctx, ids: Vec<String>) {
    let state = &ctx.state;
    let rule = state.skip_rule();
    let (ids, _) = {
        let lib = state.library.lock();
        without_skipped(ids, 0, false, |id| lib.by_id.get(id).is_some_and(|&k| rule.skips(&lib.snapshot.tracks[k])))
    };
    if ids.is_empty() {
        return;
    }
    state.engine.send(Cmd::LoadShuffled(state.queue_items(&ids)));
}

/// Album shuffle over these albums: one at a time, whole and in order, each once (see `upnext`).
pub fn shuffle_albums(ctx: &Ctx, album_ids: Vec<String>) -> bool {
    crate::upnext::shuffle_albums(ctx, album_ids)
}

/// Plays one of these albums, picked at random (one not played lately, more likely). Its id.
pub fn surprise_album(ctx: &Ctx, album_ids: Vec<String>) -> Option<String> {
    crate::upnext::surprise_album(ctx, album_ids)
}

/// "Stop after this song" on the song at this queue place (None: off).
pub fn stop_after_song(ctx: &Ctx, index: Option<usize>) {
    ctx.state.engine.send(Cmd::StopAt(index));
}

/// Takes the songs Autoplay added out of the queue.
pub fn clear_autoplay(ctx: &Ctx) {
    ctx.state.engine.send(Cmd::ClearAutoplay);
}

/// How shuffle plays, and what happens when the queue ends.
#[derive(Clone, Copy, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueOptions {
    shuffle_mode: crate::shuffle::ShuffleMode,
    /// When the queue ends: keep playing songs like the last ones (false: stop).
    autoplay: bool,
}

pub fn get_queue_options(ctx: &Ctx) -> QueueOptions {
    let s = ctx.state.settings.lock();
    QueueOptions { shuffle_mode: s.shuffle_mode, autoplay: s.autoplay }
}

pub fn set_queue_options(ctx: &Ctx, options: QueueOptions) {
    let state = &ctx.state;
    state.engine.send(Cmd::SetShuffleMode(options.shuffle_mode));
    state.engine.send(Cmd::SetAutoplay(options.autoplay));
    {
        let mut s = state.settings.lock();
        s.shuffle_mode = options.shuffle_mode;
        s.autoplay = options.autoplay;
    }
    state.save_settings_later();
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

/// `device_gen` (optional): as for `set_volume`.
pub fn set_dsp(ctx: &Ctx, dsp: DspSettings, device_gen: Option<u64>) {
    let state = &ctx.state;
    // Stamped with the device it's for; the engine keeps it as that device's own.
    crate::profiles::set_dsp(state, dsp, device_gen);
    // The EQ sends a change every ~40 ms while a band is dragged: heard at once, saved once it
    // settles (and at exit).
    state.save_settings_later();
}

/// The Sound page's output device picker.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevices {
    /// Cadence chooses the device here (not on Android, which routes to its devices itself).
    supported: bool,
    /// Connected devices, then remembered ones that aren't (`present: false`).
    devices: Vec<crate::audio::devices::Entry>,
    /// The chosen device (None: the system default).
    chosen: Option<crate::audio::DeviceRef>,
    /// "Remember sound settings per device".
    profiles: bool,
}

/// (`blocking`: it asks the system for its devices.)
pub fn list_output_devices(ctx: &Ctx) -> Result<OutputDevices, String> {
    let state = &ctx.state;
    let (chosen, profiles, remembered, direct) = {
        let s = state.settings.lock();
        let remembered: Vec<(String, String)> = s.sound_profiles.devices.iter().map(|(id, p)| (id.clone(), p.name.clone())).collect();
        (s.output_device.clone(), s.device_profiles, remembered, s.dsp.exclusive)
    };
    let supported = !cfg!(target_os = "android");
    if !supported {
        return Ok(OutputDevices { supported, devices: Vec::new(), chosen, profiles });
    }
    let present = crate::audio::list_output_devices().map_err(|e| format!("Couldn't list the output devices: {e}"))?;
    // Linux: a sound card's raw ALSA entries only open with Direct output while a sound server runs.
    let present = crate::audio::devices::for_picker(present, direct);
    // Where the system doesn't tell Cadence about devices (not Windows), a chosen one that's back
    // is found this way.
    state.engine.send(Cmd::CheckDevices);
    let devices = crate::audio::devices::entries(&present, chosen.as_ref(), remembered.iter().map(|(id, name)| (id.as_str(), name.as_str())));
    Ok(OutputDevices { supported, devices, chosen, profiles })
}

/// Plays on this device from now on (None: the system default, followed when it changes).
pub fn set_output_device(ctx: &Ctx, device: Option<crate::audio::DeviceRef>) {
    let state = &ctx.state;
    let device = device.filter(|d| !d.id.is_empty());
    state.settings.lock().output_device = device.clone();
    state.save_settings_later();
    state.engine.send(Cmd::SetDevice(device));
}

/// The chosen device isn't connected: play on the system default until it's back.
pub fn output_use_default(ctx: &Ctx) {
    ctx.state.engine.send(Cmd::UseDefaultForNow);
}

/// "Remember sound settings per device" on or off.
pub fn set_device_profiles(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    let device = state.engine.status().device.map(|d| crate::audio::DeviceRef { id: d.id, name: d.name });
    crate::profiles::set_enabled(state, enabled, device.as_ref());
    state.save_settings_later();
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
    global_hotkeys: bool,
    lyrics_online: bool,
    fix_tags_online: bool,
    listenbrainz_user: String,
    /// Learn the artists' other names from MusicBrainz.
    artist_names_online: bool,
    /// Hearts are sent to ListenBrainz as loved songs.
    listenbrainz_loved: bool,
    /// New releases by the library's artists are looked up once a day (Discover).
    new_releases_online: bool,
    /// Paired phones can sync from this computer (a local network port is open).
    phone_sync: bool,
}

pub fn get_app_settings(ctx: &Ctx) -> AppSettings {
    let state = &ctx.state;
    let s = state.settings.lock();
    AppSettings {
        close_to_tray: s.close_to_tray,
        global_hotkeys: s.global_hotkeys,
        lyrics_online: s.lyrics_online,
        fix_tags_online: s.fix_tags_online,
        listenbrainz_user: s.listenbrainz_user.clone(),
        artist_names_online: s.artist_names_online,
        listenbrainz_loved: s.listenbrainz_loved,
        new_releases_online: s.new_releases_online,
        phone_sync: s.phone_sync,
    }
}

// ---------- library health ----------

/// What the file checks have found so far.
pub fn health_report(ctx: &Ctx) -> crate::health::Report {
    crate::health::report(&ctx.state)
}

/// Songs that are in the library more than once.
pub fn health_duplicates(ctx: &Ctx) -> Vec<crate::health::DupGroup> {
    crate::health::duplicates(&ctx.state.library.lock().snapshot.tracks)
}

/// Starts a background check: "quality" (lossless files with lossy or CD-quality content) or
/// "integrity" (decodes every file and verifies its checksum). False when one is already running.
pub fn health_start(ctx: &Ctx, kind: String) -> bool {
    let kind = match kind.as_str() {
        "quality" => crate::health::Kind::Quality,
        "integrity" => crate::health::Kind::Integrity,
        _ => return false,
    };
    crate::health::start(ctx.clone(), kind)
}

pub fn health_stop(ctx: &Ctx) {
    crate::health::stop(&ctx.state);
}

/// Deletes the files of songs (duplicates the user picked); see `health::remove` for the safety checks.
pub fn health_remove(ctx: &Ctx, ids: Vec<String>) -> Result<crate::health::Removed, String> {
    Ok(crate::health::remove(ctx, &ids))
}

// ---------- Needs Attention ----------

/// What the listener ignored, and the kinds of problem they hid.
pub fn attention_state(ctx: &Ctx) -> crate::attention::Attention {
    crate::attention::load(&ctx.state.data_dir)
}

pub fn attention_ignore(ctx: &Ctx, keys: Vec<String>, ignored: bool) -> crate::attention::Attention {
    crate::attention::ignore(ctx, keys, ignored)
}

pub fn attention_hide(ctx: &Ctx, kind: String, hidden: bool) -> crate::attention::Attention {
    crate::attention::hide(ctx, kind, hidden)
}

/// Looks for covers of these albums in the background; false if that's running already.
pub fn covers_start(ctx: &Ctx, album_ids: Vec<String>) -> bool {
    crate::libfix::covers_start(ctx.clone(), album_ids)
}

pub fn covers_stop(_ctx: &Ctx) {
    crate::libfix::covers_stop();
}

pub fn covers_list(_ctx: &Ctx) -> crate::libfix::CoversFound {
    crate::libfix::covers_list()
}

pub fn covers_save(ctx: &Ctx, album_ids: Vec<String>) -> Result<crate::libfix::CoversSaved, String> {
    crate::libfix::covers_save(ctx, album_ids)
}

pub fn covers_discard(ctx: &Ctx) {
    crate::libfix::covers_discard(ctx);
}

pub fn set_global_hotkeys(ctx: &Ctx, enabled: bool) {
    ctx.state.settings.lock().global_hotkeys = enabled;
    ctx.state.save_settings_later();
    ctx.host().set_hotkeys(enabled);
}

/// A global hotkey was pressed (see hotkeys.rs).
pub fn hotkey(ctx: &Ctx, action: &str) {
    let state = &ctx.state;
    match action {
        "toggle" | "next" | "prev" => player(ctx, action.to_string()),
        // Steps on the volume slider, for the device the volume was read from (one snapshot).
        "volume up" | "volume down" => {
            let step = if action == "volume up" { 0.05 } else { -0.05 };
            let (volume, device_gen) = crate::profiles::volume_step(&state.engine.status(), step);
            set_volume(ctx, volume, Some(device_gen));
        }
        "favorite" => {
            let status = state.engine.status();
            let Some(id) = status.index.and_then(|i| status.queue.get(i)).cloned() else { return };
            let loved = state.marks.lock().view().get(&id).is_some_and(|m| m.loved);
            set_loved(ctx, vec![id], !loved);
            ctx.emit("marks-changed", ());
        }
        _ => {}
    }
}

pub fn set_lyrics_online(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().lyrics_online = enabled;
    state.save_settings_later();
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
    state.save_settings_later();
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

/// Favourites and star ratings, by song id.
pub fn get_marks(ctx: &Ctx) -> std::collections::HashMap<String, crate::marks::Mark> {
    ctx.state.marks.lock().view()
}

/// Rates songs 1–5 stars (0 clears the rating).
pub fn set_rating(ctx: &Ctx, ids: Vec<String>, rating: u8) {
    let state = &ctx.state;
    change_marks(state, &ids, |marks, t| marks.set_rating(t, rating));
}

/// Adds songs to the favourites, or takes them out.
pub fn set_loved(ctx: &Ctx, ids: Vec<String>, loved: bool) {
    let state = &ctx.state;
    change_marks(state, &ids, |marks, t| marks.set_loved(t, loved));
    // Told to ListenBrainz too, when that's on (kept and sent in the background: never waits).
    let (user, on) = {
        let s = state.settings.lock();
        (s.listenbrainz_user.clone(), s.listenbrainz_loved)
    };
    if let (true, false, Some(sc)) = (on, user.is_empty(), crate::scrobble::running()) {
        let lib = state.library.lock();
        for id in &ids {
            if let Some(&k) = lib.by_id.get(id) {
                sc.loved(crate::lbloved::Item::of(&lib.snapshot.tracks[k], &user, i8::from(loved)));
            }
        }
    }
}

fn change_marks(state: &crate::state::AppState, ids: &[String], change: impl Fn(&mut crate::marks::Marks, &crate::library::Track)) {
    {
        let lib = state.library.lock();
        let mut marks = state.marks.lock();
        for id in ids {
            if let Some(&k) = lib.by_id.get(id) {
                change(&mut marks, &lib.snapshot.tracks[k]);
            }
        }
    }
    state.save_marks();
}

pub fn set_skip_instrumentals(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().skip_instrumentals = enabled;
    state.save_settings_later();
}

pub fn set_fix_tags_online(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().fix_tags_online = enabled;
    state.save_settings_later();
    if !enabled {
        // The artist check a scan started by itself stops too.
        crate::libfix::fix_tags_online_off();
    }
}

// ---------- one artist across scripts (identity.rs, musicbrainz.rs, searchkeys.rs) ----------

/// Learning the artists' other names from MusicBrainz, on or off (off stops a running job).
/// (`blocking`: turning it on walks the library for the names to ask about.)
pub fn set_artist_names_online(ctx: &Ctx, enabled: bool) -> Result<(), String> {
    let state = &ctx.state;
    state.settings.lock().artist_names_online = enabled;
    state.save_settings_later();
    if enabled {
        crate::musicbrainz::auto(ctx);
    } else {
        crate::musicbrainz::turned_off();
    }
    Ok(())
}

/// The artists the library writes in more than one way.
pub fn artist_identity(ctx: &Ctx) -> crate::identity::IdentityView {
    let _ = ctx;
    crate::identity::view(&crate::identity::current())
}

/// Extra words for the window's search index (`blocking`: worked out after a library change).
pub fn search_keys(ctx: &Ctx) -> Result<crate::searchkeys::SearchKeys, String> {
    Ok(crate::searchkeys::current(ctx))
}

/// A decision about artists can't be written while a restore replaces them.
fn not_while_restoring() -> Result<(), String> {
    if crate::backup::restoring() {
        return Err("A backup is being restored. Try again when it has finished.".into());
    }
    Ok(())
}

/// "Treat as the same artist" (Needs Attention): no file changes. (`blocking`: writes the
/// decisions and works the artists out again.)
pub fn artist_mark_same(ctx: &Ctx, a: String, b: String) -> Result<(), String> {
    not_while_restoring()?;
    crate::libfix::artist_mark_same(ctx, a, b);
    Ok(())
}

/// "Not the same artist" (an artist page): `name` is split out from `others`. The pairs written.
pub fn artist_mark_different(ctx: &Ctx, name: String, others: Vec<String>) -> Result<Vec<[String; 2]>, String> {
    not_while_restoring()?;
    Ok(crate::libfix::artist_mark_different(ctx, name, others))
}

/// Undoes either of the two above.
pub fn artist_forget(ctx: &Ctx, pairs: Vec<[String; 2]>) -> Result<(), String> {
    not_while_restoring()?;
    crate::libfix::artist_forget(ctx, pairs);
    Ok(())
}

/// (`blocking`: counts the library's artist names.)
pub fn artist_names_status(ctx: &Ctx) -> Result<crate::musicbrainz::Status, String> {
    Ok(crate::musicbrainz::status(ctx))
}

/// Pause (kept across restarts) or resume the MusicBrainz job. (`blocking`: saves its cache and
/// walks the library to say where it is.)
pub fn artist_names_pause(ctx: &Ctx, paused: bool) -> Result<(), String> {
    crate::musicbrainz::set_paused(ctx, paused);
    Ok(())
}

/// Stops the MusicBrainz job until Cadence starts again. (`blocking`: says where it is, which
/// walks the library.)
pub fn artist_names_stop(ctx: &Ctx) -> Result<(), String> {
    crate::musicbrainz::stop(ctx);
    Ok(())
}

/// Lyrics for a song: from the file or a .lrc next to it, else LRCLIB (if allowed). Placed on the
/// singing when Cadence has listened to the song (see `voice`).
pub fn get_lyrics(ctx: &Ctx, track_id: String) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let track = track_of(state, &track_id)?;
    let (state, voice) = (state.clone(), voice.clone());
    let lyrics = voice.apply(&track, resolve_lyrics(&state, &track));
    // Lyrics from the song's own file join the lyrics search (saved ones are in it already).
    crate::lyricindex::seen(&track.id, &lyrics);
    // Followed before Cadence told singers apart: their voices once more (separating only).
    if !lyrics.voice.is_empty() && lyrics.singers.is_empty() && state.settings.lock().voice_auto && voice.installed() && !voice.has_voices(&track) && voice.several_singers(&track) {
        crate::voice::request(ctx, track_id, resolve_lyrics);
    }
    Ok(lyrics)
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
    // (Not for a song of a single-file album: lyrics in or beside the file are the whole album's.)
    let local = if track.cue.is_some() { None } else { crate::lyrics::local(Path::new(&track.path)) };
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

/// Finds out who sings a song's lines: the AI names the singers (and the parts, when published
/// lyrics mark them), and Cadence tells their voices apart in the singing if it hasn't yet
/// (a "voice-done" event follows then).
pub fn find_singers(ctx: &Ctx, track_id: String) -> Result<crate::lyrics::Lyrics, String> {
    let state = ctx.state.clone();
    let voice = ctx.voice.clone();
    let track = track_of(&state, &track_id)?;
    let lyrics = resolve_lyrics(&state, &track);
    let shown = voice.apply(&track, lyrics.clone());
    if shown.synced.is_empty() {
        return Err("Singers can be shown on timed lyrics only.".into());
    }
    if shown.singers_from == "lyrics" {
        return Ok(shown);
    }
    // Tells the voices apart, if the song is known to have several singers and that hasn't been done.
    let listen = |voice: &crate::voice::Voice| {
        if voice.installed() && voice.several_singers(&track) && !voice.has_voices(&track) {
            crate::voice::request(ctx, track_id.clone(), resolve_lyrics);
        }
    };
    let settings = state.settings.lock().ai.clone();
    if settings.provider == crate::ai::Provider::Off {
        listen(&voice);
        return Ok(shown);
    }
    // The lines as saved (before placing on the singing: the same text).
    let texts: Vec<String> = lyrics.synced.iter().map(|l| l.text.clone()).collect();
    let found = crate::ai::find_singers(&settings, &track, &texts)?;
    log::info!(
        "AI singers for {}: {} ({} parts)",
        track.title,
        found.singers.iter().map(|s| format!("{} [{}]", s.name, s.voice)).collect::<Vec<_>>().join(", "),
        found.parts.iter().filter(|p| !p.is_empty()).count()
    );
    voice.save_ai_singers(&track.id, &texts, found.singers, found.parts);
    listen(&voice);
    Ok(voice.apply(&track, lyrics))
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
    crate::voice::remove(voice);
}

pub fn voice_set_auto(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().voice_auto = enabled;
    state.save_settings_later();
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
    let track = track_of(state, &track_id)?;
    let (state, voice) = (state.clone(), voice.clone());
    voice.set_on(&track.id, on);
    Ok(voice.apply(&track, resolve_lyrics(&state, &track)))
}

pub fn set_close_to_tray(ctx: &Ctx, enabled: bool) {
    let state = &ctx.state;
    state.settings.lock().close_to_tray = enabled;
    state.save_settings_later();
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
    // Songs that moved since the window loaded the playlist keep following their files.
    let mut playlist = playlist;
    state.follow_moves(&mut playlist);
    let saved = state.playlists.lock().upsert(playlist, now);
    state.save_playlists();
    saved
}

pub fn delete_playlist(ctx: &Ctx, id: String) {
    let state = &ctx.state;
    state.playlists.lock().remove(&id);
    state.save_playlists();
}

// ---------- playlist import and export (playlist_io.rs) ----------
// All `blocking`: they read and write files and match whole libraries.

/// Writes a playlist (the songs `ids`, in order) as M3U8 or CSV to `path` (the save dialog's; on
/// Android none: Documents › Cadence). The window resolves smart playlists to their songs.
pub fn playlist_export(
    ctx: &Ctx,
    name: String,
    ids: Vec<String>,
    path: Option<String>,
    options: crate::playlist_io::ExportOptions,
) -> Result<crate::playlist_io::ExportDone, String> {
    crate::playlist_io::export_one(ctx, &name, &ids, path, &options)
}

/// One file per playlist in `folder` (on Android none: Documents › Cadence).
pub fn playlist_export_all(
    ctx: &Ctx,
    items: Vec<crate::playlist_io::ExportItem>,
    folder: Option<String>,
    options: crate::playlist_io::ExportOptions,
) -> Result<crate::playlist_io::ExportAllDone, String> {
    crate::playlist_io::export_all(ctx, &items, folder, &options)
}

/// Reads and matches playlist files against the library, saving nothing: one preview per file.
pub fn playlist_import(ctx: &Ctx, paths: Vec<String>) -> Result<Vec<crate::playlist_io::ImportPreview>, String> {
    Ok(crate::playlist_io::import_files(ctx, &paths))
}

/// Android: the playlist files in Documents › Cadence (the file picker can't hand over a path).
pub fn playlist_import_files(ctx: &Ctx) -> Result<crate::playlist_io::ImportFiles, String> {
    let _ = ctx;
    Ok(crate::playlist_io::import_candidates())
}

/// Writes the entries of a playlist file that aren't in the library as a CSV.
pub fn playlist_missing_save(ctx: &Ctx, source: String, path: Option<String>) -> Result<crate::playlist_io::SavedFile, String> {
    crate::playlist_io::save_missing(ctx, &source, path)
}

// ---------- scrobbling ----------

/// Checks the token with ListenBrainz, then stores it in Credential Manager. Returns the user name.
pub fn connect_listenbrainz(ctx: &Ctx, token: String) -> Result<String, String> {
    let state = &ctx.state;
    let t = token.clone();
    let user = crate::scrobble::validate(&t)?;
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
    // Listens still waiting were this account's: an account connected later mustn't get them.
    crate::scrobble::clear(&state.data_dir.join("scrobble-queue.json"));
    Ok(())
}

pub fn set_listenbrainz_loved(ctx: &Ctx, enabled: bool) {
    ctx.state.settings.lock().listenbrainz_loved = enabled;
    ctx.state.save_settings_later();
    // The sender checks it before each turn (hearts already waiting aren't sent while it's off).
    crate::lbloved::set_sending(enabled);
    if enabled && let Some(s) = crate::scrobble::running() {
        s.wake();
    }
}

pub fn set_new_releases_online(ctx: &Ctx, enabled: bool) {
    ctx.state.settings.lock().new_releases_online = enabled;
    ctx.state.save_settings_later();
}

/// Restoring the listening history from ListenBrainz (`lbimport`).
/// (`blocking`, as the three after it: they read or remove the import's files.)
pub fn lb_import_status(ctx: &Ctx) -> Result<crate::lbimport::Status, String> {
    Ok(crate::lbimport::status(ctx))
}

pub fn lb_import_start(ctx: &Ctx) -> Result<crate::lbimport::Status, String> {
    crate::lbimport::start(ctx)
}

pub fn lb_import_pause(ctx: &Ctx) -> Result<(), String> {
    crate::lbimport::pause(ctx);
    Ok(())
}

pub fn lb_import_discard(ctx: &Ctx) -> Result<(), String> {
    crate::lbimport::discard(ctx);
    Ok(())
}

pub fn lb_import_apply(ctx: &Ctx) -> Result<crate::lbimport::Applied, String> {
    crate::lbimport::apply(ctx)
}

pub fn lb_import_save_unmatched(ctx: &Ctx, path: Option<String>) -> Result<crate::playlist_io::SavedFile, String> {
    crate::lbimport::save_unmatched(ctx, path)
}

/// Favorites and ListenBrainz's loved songs (`lbloved`).
pub fn lb_loved_preview(ctx: &Ctx) -> Result<crate::lbloved::SyncPreview, String> {
    crate::lbloved::preview(ctx)
}

pub fn lb_loved_apply(ctx: &Ctx) -> Result<crate::lbloved::SyncDone, String> {
    crate::lbloved::apply(ctx)
}

/// How many hearts wait to be sent to ListenBrainz.
pub fn lb_loved_waiting(_ctx: &Ctx) -> usize {
    crate::lbloved::waiting()
}

/// New releases by the library's artists (asks ListenBrainz when it's due, once a day).
pub fn new_releases(ctx: &Ctx) -> Result<crate::lbreleases::View, String> {
    Ok(crate::lbreleases::for_discover(ctx, true))
}

/// What was found last time (no network). (`blocking`: reads the kept list and matches it against
/// the library's albums.)
pub fn new_releases_cached(ctx: &Ctx) -> Result<crate::lbreleases::View, String> {
    Ok(crate::lbreleases::for_discover(ctx, false))
}

// ---------- tag editing ----------

#[derive(Serialize)]
pub struct TagsSaved {
    saved: usize,
    /// Songs playing now: shown changed already, their files are written when they stop.
    later: usize,
    /// The change, to undo it.
    change: Option<String>,
}

/// Writes the changed fields into each song's file, then rescans so the library shows them.
pub fn write_tags(ctx: &Ctx, ids: Vec<String>, changes: crate::tagedit::TagChanges) -> Result<TagsSaved, String> {
    let state = &ctx.state;
    crate::tagedit::check(&changes)?;
    {
        let lib = state.library.lock();
        if ids.iter().any(|id| lib.by_id.get(id).is_some_and(|&i| lib.snapshot.tracks[i].cue.is_some())) {
            return Err("Songs from a single-file album (CUE sheet) take their info from the .cue file, which Cadence doesn't edit.".into());
        }
    }
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
    // What the files are like now, to undo the change.
    let mut rec = crate::changes::Recorder::new(
        ctx,
        if songs.len() == 1 { "Edited song info".to_string() } else { format!("Edited the info of {} songs", songs.len()) },
    );
    for (_, path) in &songs {
        rec.add(path, false);
    }
    let change = rec.finish(ctx);
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

    // Every file is tried: one that's locked or read-only doesn't stop the rest.
    let mut shown = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    for (id, p) in &now {
        match crate::tagedit::write(Path::new(p), &changes) {
            Ok(()) => shown.push((id.clone(), changes.clone())),
            Err(e) => {
                log::warn!("couldn't save the info of {p}: {e}");
                // ("Couldn't save a.flac: Access is denied" → "Access is denied": the name is shown already.)
                let why = e.split_once(": ").map_or(e.clone(), |(_, why)| why.to_string());
                failed.push((p.clone(), why));
            }
        }
    }
    let written = shown.len();
    if written > 0 {
        // Shown at once; the rescan reads the files back (it can take a while in a big library).
        crate::tagedit::show_now(state, &shown);
        ctx.emit("library-changed", ());
        start_scan(ctx.clone());
    }
    if !failed.is_empty() {
        return Err(not_all_saved(written + later.len(), written + later.len() + failed.len(), &failed));
    }
    Ok(TagsSaved {
        saved: written + later.len(),
        later: later.len(),
        change,
    })
}

/// "Saved 3 of 5. Couldn't save: a.flac (it's read-only), b.flac (…)." `failed`: (path, reason).
pub(crate) fn not_all_saved(saved: usize, total: usize, failed: &[(String, String)]) -> String {
    let name = |p: &str| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string());
    let list: Vec<String> = failed.iter().take(5).map(|(p, why)| format!("{} ({why})", name(p))).collect();
    let more = if failed.len() > 5 { format!(" and {} more", failed.len() - 5) } else { String::new() };
    format!("Saved {saved} of {total}. Couldn't save: {}{more}.", list.join(", "))
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
    let track = track_of(state, &id)?;
    (move || {
        let term = query
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .unwrap_or_else(|| crate::autotag::search_term(&track));
        let results = crate::autotag::candidates(&track, &term)?;
        Ok(SongInfoResults { term, results })
    })()
}

/// Saves the match picked in Fix Song Info (keeping the song's own title, and for the other songs
/// of its album too, when asked).
pub fn save_song_info(
    ctx: &Ctx,
    id: String,
    info: crate::autotag::Candidate,
    keep_title: bool,
    whole_album: bool,
) -> Result<crate::autotag::Saved, String> {
    crate::autotag::save_choice(ctx, &id, info, keep_title, whole_album)
}

/// Fix Album Info: catalogue albums for an album (searching `query`, or its artist and name).
pub fn find_album_info(ctx: &Ctx, album_id: String, query: Option<String>) -> Result<crate::libfix::AlbumResults, String> {
    crate::libfix::find_album_info(ctx, &album_id, query)
}

/// Fix Album Info: the catalogue album's songs, lined up with the album's files.
pub fn album_plan(ctx: &Ctx, album_id: String, album_ref: String) -> Result<crate::libfix::AlbumPlan, String> {
    crate::libfix::album_plan(ctx, &album_id, &album_ref)
}

/// Fix Album Info: writes the picked album's info into the album's files.
pub fn save_album_info(
    ctx: &Ctx,
    album_id: String,
    album: crate::libfix::AlbumCandidate,
    songs: Vec<crate::autotag::AlbumSong>,
    assign: Vec<crate::libfix::Assign>,
    keep_titles: bool,
) -> Result<crate::libfix::AlbumSaved, String> {
    crate::libfix::save_album_info(ctx, &album_id, album, songs, assign, keep_titles)
}

/// Looks up every song without tags in the background; false if that's running already.
pub fn fix_all_start(ctx: &Ctx) -> bool {
    crate::libfix::fix_all_start(ctx.clone())
}

pub fn fix_all_stop(_ctx: &Ctx) {
    crate::libfix::fix_all_stop();
}

/// What Fix all found, for the user to check before it's saved.
pub fn fix_all_found(_ctx: &Ctx) -> Vec<crate::libfix::FoundInfo> {
    crate::libfix::fix_all_found()
}

/// Saves what Fix all found for the songs the user kept.
pub fn fix_all_save(ctx: &Ctx, ids: Vec<String>) -> Result<crate::libfix::FixAllSaved, String> {
    crate::libfix::fix_all_save(ctx, ids)
}

pub fn fix_all_discard(ctx: &Ctx) {
    crate::libfix::fix_all_discard(ctx);
}

/// Cadence's recent changes to songs' files, newest first, to undo.
pub fn recent_changes(ctx: &Ctx) -> Vec<crate::changes::ChangeInfo> {
    crate::changes::recent(ctx)
}

/// Puts the files of a change back as they were; returns how many songs.
pub fn undo_change(ctx: &Ctx, id: String) -> Result<usize, String> {
    crate::changes::undo(ctx, &id)
}

/// Asks the catalogue which artist names in Japanese or Chinese letters have a Latin one too.
pub fn artist_check_start(ctx: &Ctx) -> bool {
    crate::libfix::artist_check_start(ctx.clone())
}

pub fn artist_check_stop(_ctx: &Ctx) {
    crate::libfix::artist_check_stop();
}

/// Artists in the library under two names.
pub fn artist_pairs(ctx: &Ctx) -> crate::libfix::ArtistPairs {
    crate::libfix::artist_pairs(ctx)
}

pub fn artist_pair_dismiss(ctx: &Ctx, native: String, latin: String) {
    crate::libfix::artist_pair_dismiss(ctx, native, latin);
}

/// Renames an artist in every song (artist and album artist).
pub fn merge_artist(ctx: &Ctx, from: String, to: String) -> Result<crate::libfix::Renamed, String> {
    crate::libfix::merge_artist(ctx, from, to)
}

/// Lyrics lines in Latin letters (Japanese romaji, Chinese pinyin, Korean); empty for lines
/// that need none.
pub fn romanize_lyrics(_ctx: &Ctx, lines: Vec<String>) -> Result<Vec<String>, String> {
    Ok(crate::romanize::lines(&lines))
}

/// Readings above the characters of each line (furigana over kanji, pinyin over hanzi); an empty
/// list for lines that need none.
pub fn ruby_lyrics(_ctx: &Ctx, lines: Vec<String>) -> Result<Vec<Vec<crate::romanize::RubyPart>>, String> {
    Ok(crate::romanize::ruby(&lines))
}

/// The translation of a song's lyrics (as the lyrics view shows them) into `lang`: the one that
/// came with them, one made before, or the AI's (only when it's set up; asked once per lyrics
/// and language, then kept). Another song's request (or `translate_cancel`) stops it.
pub fn translate_lyrics(ctx: &Ctx, track_id: String, lang: String) -> Result<crate::translate::Translation, String> {
    let state = ctx.state.clone();
    let track = track_of(&state, &track_id)?;
    crate::translate::begin(&track_id);
    let lyrics = ctx.voice.apply(&track, resolve_lyrics(&state, &track));
    let lines: Vec<String> = if lyrics.instrumental {
        Vec::new()
    } else if !lyrics.synced.is_empty() {
        lyrics.synced.iter().map(|l| l.text.clone()).collect()
    } else {
        lyrics.plain.split('\n').map(str::to_string).collect()
    };
    let bundled: &[String] = if lyrics.translation.len() == lyrics.synced.len() && !lyrics.synced.is_empty() { &lyrics.translation } else { &[] };
    let song = crate::translate::Song { title: &track.title, artist: &track.artist, lines: &lines, bundled, bundled_lang: &lyrics.translation_lang };
    let settings = state.settings.lock().ai.clone();
    let ai = crate::ai::ready(&settings).then(|| crate::translate::Ai(settings));
    let cancelled = || !crate::translate::is_wanted(&track_id);
    let out = crate::translate::translate(&state.data_dir.join("lyrics"), &track_id, &song, &lang, ai.as_ref().map(|a| a as &dyn crate::translate::Model), &cancelled);
    if let Ok(t) = &out {
        log::info!("translation of {} into {lang}: {} ({} lines)", track.title, t.from, t.lines.len());
    }
    out
}

/// The lyrics view moved on from this song: its translation isn't asked for any more.
pub fn translate_cancel(_ctx: &Ctx, track_id: String) {
    crate::translate::cancel(&track_id);
}

/// Songs with a line of their lyrics matching `query`, with the line and where it's sung.
pub fn lyric_search(ctx: &Ctx, query: String, limit: Option<usize>) -> Result<crate::lyricindex::Found, String> {
    Ok(crate::lyricindex::search(ctx, &query, limit.unwrap_or(20).min(100)))
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
    crate::covers::set(ctx, &album_id, source)
}

// ---------- artist pictures ----------

pub fn artist_images(ctx: &Ctx) -> std::collections::HashMap<String, crate::artists::ArtistImage> {
    let state = &ctx.state;
    crate::artists::all(&state.data_dir)
}

/// Photos of this artist from the online catalogue.
pub fn find_artist_photos(_ctx: &Ctx, name: String) -> Result<Vec<crate::artists::Photo>, String> {
    crate::artists::search(&name)
}

pub fn set_artist_image(ctx: &Ctx, name: String, source: crate::artists::Source) -> Result<crate::artists::ArtistImage, String> {
    let state = &ctx.state;
    let (data, art) = (state.data_dir.clone(), state.art_dir());
    crate::artists::set(&data, &art, &name, source)
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

/// Searches LRCLIB for a song's lyrics by its title, artist and album (as the listener typed
/// them: tags can be wrong), best first: our artist, then the song's length (from `track_id`).
pub fn search_lyrics(ctx: &Ctx, track_id: String, title: String, artist: String, album: String) -> Result<Vec<crate::lyrics::Hit>, String> {
    let duration = track_of(&ctx.state, &track_id).map(|t| t.duration).unwrap_or(0.0);
    let mut hits = crate::lyrics::search_song(&title, &artist, &album)?;
    crate::lyrics::rank_hits(&mut hits, &crate::lyrics::Wanted { title: &title, artist: &artist, album: &album, duration });
    Ok(hits)
}

/// Uses an LRCLIB search result as this song's lyrics (kept for next time).
pub fn use_lyrics(ctx: &Ctx, track_id: String, hit_id: u64) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let cache = state.data_dir.join("lyrics");
    let track = track_of(state, &track_id)?;
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
    let track = track_of(state, &track_id)?;
    Ok(voice.apply(&track, l))
}

/// Lets the AI find lyrics: it suggests better searches for LRCLIB (for time-synced lyrics) and
/// looks the lyrics up on the web as a fallback.
pub fn ai_find_lyrics(ctx: &Ctx, track_id: String) -> Result<crate::lyrics::Lyrics, String> {
    let state = &ctx.state;
    let voice = &ctx.voice;
    let track = track_of(state, &track_id)?;
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
                if let Some(hit) = crate::lyrics::best_hit(&hits, &wanted)
                    && let Ok(l) = crate::lyrics::fetch(hit.id) {
                        crate::lyrics::pick(&cache, &track.id, &l);
                        return Ok(voice.apply(&track, l));
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
/// (`blocking`: it goes through the whole history and library.)
pub fn listening_profile(ctx: &Ctx) -> Result<crate::ai::Profile, String> {
    Ok(profile(&ctx.state))
}

fn profile(state: &AppState) -> crate::ai::Profile {
    let since = crate::history::now().saturating_sub(30 * 86_400);
    let (month, all) = {
        let h = state.history.lock();
        let mut m: HashMap<String, u32> = HashMap::new();
        for e in h.events.iter().filter(|e| e.at >= since && !e.skipped && (e.listened >= 30.0 || e.listened >= e.duration * 0.5)) {
            *m.entry(e.track_id.clone()).or_insert(0) += 1;
        }
        (m, h.stats().plays)
    };
    let marks = state.marks.lock().view();
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
        favorites: crate::mixes::discover::favorites(&lib.snapshot.tracks, &marks, &all),
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
    // The sections below (stats.rs): over the whole history (archive included); the range ones
    // say so.
    /// Songs played on this date in earlier years.
    pub on_this_day: Vec<crate::stats::DayItem>,
    /// Songs played often, loved or rated 4–5 stars, not played for 180 days.
    pub forgotten: Vec<crate::stats::Forgotten>,
    /// Plays per local hour, 0–23 (the range).
    pub clock: Vec<u32>,
    /// Plays per weekday, Monday first (the range).
    pub weekdays: Vec<u32>,
    pub streak: Option<crate::stats::Streaks>,
    /// New artists per month, the last 12 months.
    pub discovery: Vec<crate::stats::MonthCount>,
    pub first_song: Option<crate::stats::FirstSong>,
    /// Albums heard through in one go (the range); `finished` = how often.
    pub complete_albums: Vec<StatItem>,
    /// Plays by the decade of the songs' year (the range).
    pub decades: Vec<crate::stats::Decade>,
    /// Plays by the songs' language (the range), most played first.
    pub languages: Vec<crate::stats::Language>,
    /// Unix seconds of the first play recorded (0: none).
    pub history_from: u64,
}

/// (`blocking`: it goes through the whole history, the archive's year files summarised once and
/// kept, see stats.rs.)
pub fn listening_stats(ctx: &Ctx, range: String) -> Result<ListeningStats, String> {
    Ok(crate::stats::listening_stats(&ctx.state, range))
}

/// The names in a credit like "A, B & C feat. D", split as the window's `splitCredit` (api.ts)
/// splits them: at , ; / &, "feat." / "ft." / "featuring" and a lone x or ×.
fn split_credit_like_ui(credit: &str) -> Vec<String> {
    let chars: Vec<char> = credit.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let at = |i: usize, s: &str| s.chars().enumerate().all(|(k, c)| lower.get(i + k) == Some(&c));
    let space_at = |i: usize| chars.get(i).is_some_and(|c| c.is_whitespace());
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut i = 0;
    while i < chars.len() {
        let starts_word = i == 0 || !word(chars[i - 1]);
        // "feat" / "ft", an optional dot, then a space (the space stays, trimmed away below).
        let abbreviation = |s: &str| {
            let n = s.chars().count();
            if !at(i, s) {
                return 0;
            }
            let n = if chars.get(i + n) == Some(&'.') { n + 1 } else { n };
            if space_at(i + n) { n } else { 0 }
        };
        let len = if matches!(chars[i], ',' | ';' | '/' | '&') {
            1
        } else if space_at(i) && matches!(lower.get(i + 1), Some('x' | '×')) && space_at(i + 2) {
            3
        } else if starts_word {
            match abbreviation("feat") {
                0 => match abbreviation("ft") {
                    0 if at(i, "featuring") && !chars.get(i + 9).is_some_and(|&c| word(c)) => 9,
                    n => n,
                },
                n => n,
            }
        } else {
            0
        };
        if len > 0 {
            parts.push(std::mem::take(&mut part));
            i += len;
        } else {
            part.push(chars[i]);
            i += 1;
        }
    }
    parts.push(part);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// Each album-artist credit → the artists it's listed under, as the window's `artistIndex` does it:
/// a credit is split only into parts that are artists in the library (credited alone somewhere,
/// or in two or more credits), so "Simon & Garfunkel" stays one artist.
pub(crate) fn artist_index<'a>(credits: impl Iterator<Item = &'a str>) -> HashMap<String, Vec<String>> {
    let unique: Vec<&str> = {
        let mut seen = HashSet::new();
        credits.filter(|c| seen.insert(*c)).collect()
    };
    let split: Vec<Vec<String>> = unique.iter().map(|c| split_credit_like_ui(c)).collect();
    let standalone: HashSet<String> = unique.iter().zip(&split).filter(|(_, p)| p.len() == 1).map(|(c, _)| c.to_lowercase()).collect();
    let mut uses: HashMap<String, usize> = HashMap::new();
    for parts in &split {
        for p in parts.iter().map(|p| p.to_lowercase()).collect::<HashSet<_>>() {
            *uses.entry(p).or_default() += 1;
        }
    }
    unique
        .iter()
        .zip(split)
        .map(|(c, parts)| {
            let known: Vec<String> = if parts.len() > 1 {
                parts
                    .into_iter()
                    .filter(|p| standalone.contains(&p.to_lowercase()) || uses.get(&p.to_lowercase()).copied().unwrap_or(0) >= 2)
                    .collect()
            } else {
                Vec::new()
            };
            (c.to_string(), if known.is_empty() { vec![c.to_string()] } else { known })
        })
        .collect()
}

/// Every artist page the window shows (the names `artist_index` lists credits under), by their
/// lower-case name.
pub(crate) fn artist_pages(credits: &HashMap<String, Vec<String>>) -> HashMap<String, String> {
    credits.values().flatten().map(|n| (n.to_lowercase(), n.clone())).collect()
}

/// An album credit that names no one: compilations ("Various Artists", "VA", 群星…) or none.
pub(crate) fn no_one(credit: &str) -> bool {
    let n = crate::state::match_key(credit);
    ["", "variousartists", "various", "va", "compilation", "compilations", "unknownartist", "unknown", "群星", "ヴァリアスアーティスト", "オムニバス"]
        .contains(&n.as_str())
}

/// The artists a play counts under in the stats, each an artist page that exists: the album's
/// artist credit, split as the artist list splits it ("A & B" under A and B when both are artists
/// in the library). On a compilation ("Various Artists") it's the song's own performers instead,
/// those of them that have a page; when none has one, the compilation's page.
pub(crate) fn stat_artists(track_artist: &str, album_credit: &str, credits: &HashMap<String, Vec<String>>, pages: &HashMap<String, String>) -> Vec<String> {
    if no_one(album_credit) {
        let mut performers: Vec<String> = Vec::new();
        // The whole credit when it's an artist's page ("Simon & Garfunkel"), else its names.
        let whole = pages.get(&track_artist.trim().to_lowercase()).filter(|p| !no_one(p));
        let names = match whole {
            Some(page) => vec![page.clone()],
            None => split_credit_like_ui(track_artist),
        };
        for n in names {
            if let Some(page) = pages.get(&n.to_lowercase())
                && !no_one(page) && !performers.contains(page) {
                    performers.push(page.clone());
                }
        }
        if !performers.is_empty() {
            return performers;
        }
    }
    match credits.get(album_credit) {
        Some(names) => names.clone(),
        None => vec![album_credit.to_string()],
    }
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
    let profile = profile(state);
    let input = dj_input(state, value);
    let library = state.library.lock().snapshot.tracks.clone();
    // The artists played most first: what the AI is told the library has (it can't take them all).
    let plays = state.history.lock().stats().plays;
    let owned = crate::mixes::discover::artists_by_plays(&library, &plays);
    let seen_file = state.data_dir.join("ai-recommend-seen.json");
    let mut seen = seen_songs(&state.data_dir);
    let mut before: Vec<String> = seen.iter().rev().take(60).map(|k| k.replacen('|', " - ", 1)).collect();
    let known: HashSet<String> = seen.iter().cloned().collect();
    let mut picks: Vec<crate::ai::Discovery> = Vec::new();
    let mut failed = None;
    // The filters (owned, suggested before, not in the catalogues) can leave few: then it asks once
    // more, telling it what it suggested already.
    for _ in 0..crate::mixes::discover::ROUNDS {
        let found = match crate::ai::recommend(&input.settings, basis, value, &input.taste, &profile, &owned, &before) {
            Ok(f) => f,
            Err(e) => {
                failed = Some(e);
                break;
            }
        };
        before.extend(found.iter().map(|d| format!("{} - {}", d.artist, d.title)));
        let fresh: Vec<_> = found.into_iter().filter(|d| !known.contains(&seen_key(d)) || d.rating >= crate::ai::REPEAT_AT).collect();
        picks.extend(crate::catalog::check(fresh, &library));
        picks = crate::ai::distinct_picks(picks);
        if picks.len() >= crate::mixes::discover::ENOUGH_PICKS {
            break;
        }
    }
    if picks.is_empty() {
        return Err(failed.unwrap_or_else(|| "Everything the DJ found you already have, or it suggested before. Try again or ask another way.".into()));
    }
    picks.sort_by(|a, b| b.score().cmp(&a.score()));
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

// ---------- backup and restore (backup.rs) ----------
// All `blocking`: they read and write files.

/// The snapshots, the last automatic one, and the second folder.
pub fn backup_info(ctx: &Ctx) -> Result<crate::backup::BackupInfo, String> {
    Ok(crate::backup::info(&ctx.state.data_dir))
}

/// "Back up now": to `path` (from the save dialog; on Android none: the shared Documents folder).
/// `lyricOffsets`: the window's lyrics timing nudges (JSON), kept with the data first.
pub fn backup_create(ctx: &Ctx, path: Option<String>, lyric_offsets: Option<String>) -> Result<crate::backup::Made, String> {
    crate::backup::create_now(&ctx.state, path, lyric_offsets)
}

/// Reads and checks a backup without using it: what a restore would replace.
pub fn backup_preview(ctx: &Ctx, path: String) -> Result<crate::backup::Preview, String> {
    let _ = ctx;
    crate::backup::preview(std::path::Path::new(&path))
}

/// Restores a backup (a snapshot of the data as it is now is made first).
pub fn backup_restore(ctx: &Ctx, path: String) -> Result<crate::backup::Restored, String> {
    crate::backup::restore(ctx, std::path::Path::new(&path))
}

/// The second folder automatic snapshots are copied to (None or "" turns it off).
pub fn backup_set_folder(ctx: &Ctx, folder: Option<String>) -> Result<crate::backup::BackupInfo, String> {
    crate::backup::set_folder(&ctx.state.data_dir, folder)
}

/// Keeps the window's lyrics timing nudges in the data folder, so automatic snapshots have them.
pub fn backup_keep_offsets(ctx: &Ctx, lyric_offsets: String) -> Result<(), String> {
    crate::backup::keep_offsets(&ctx.state.data_dir, &lyric_offsets)
}

/// Every command (made into Tauri commands by commands.rs).
/// `plain` returns a value, `fallible` a Result, `blocking` a Result after slow work (network,
/// files, AI), which Tauri runs on a worker thread. Argument names are the UI's, in camelCase.
#[macro_export]
macro_rules! cadence_commands {
    ($m:ident) => {
        $m! {
            plain get_library() -> $crate::library::Snapshot;
            plain get_library_part(track_ids: Vec<String>) -> $crate::library::Snapshot;
            plain get_folders() -> Vec<String>;
            plain add_folders(paths: Vec<String>) -> Vec<String>;
            fallible open_file(path: String) -> $crate::api::Opened;
            blocking download_update_apk(url: String) -> String;
            fallible list_dirs(path: Option<String>, volumes: Option<Vec<$crate::api::Volume>>) -> Vec<$crate::api::DirEntry>;
            plain remove_folder(path: String) -> Vec<String>;
            plain rescan() -> ();
            plain reanalyze() -> ();
            plain sonic_status() -> $crate::api::SonicStatus;
            blocking similar_tracks(id: String, limit: Option<usize>) -> Vec<String>;
            plain sound_radio(id: String, count: Option<usize>) -> Vec<String>;
            blocking sound_mixes() -> Vec<$crate::sonic::Mix>;
            plain get_ai_settings() -> $crate::api::AiInfo;
            plain set_ai_settings(settings: $crate::ai::AiSettings) -> ();
            fallible set_ai_key(text: String) -> $crate::api::AiInfo;
            blocking ai_test() -> String;
            blocking ai_preview(request: String) -> String;
            blocking ai_ask(request: String) -> $crate::ai::AiMix;
            blocking dj_suggestions() -> Vec<String>;
            blocking ai_mixes(refresh: bool) -> Vec<$crate::ai::AiMix>;
            blocking ai_discover(refresh: bool) -> Vec<$crate::ai::Discovery>;
            blocking listening_profile() -> $crate::ai::Profile;
            plain ui_busy() -> ();
            blocking listening_stats(range: String) -> $crate::api::ListeningStats;
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
            plain clear_up_next() -> ();
            plain move_in_queue(from: usize, to: usize) -> ();
            plain insert_in_queue(at: usize, ids: Vec<String>) -> ();
            plain set_sleep_timer(kind: String, amount: Option<u32>) -> ();
            plain get_sleep_timer() -> Option<u64>;
            plain set_volume(volume: f32, device_gen: Option<u64>) -> ();
            plain set_repeat(repeat: $crate::audio::Repeat) -> ();
            plain set_shuffle(on: bool) -> ();
            plain play_shuffled(ids: Vec<String>) -> ();
            plain shuffle_albums(album_ids: Vec<String>) -> bool;
            plain surprise_album(album_ids: Vec<String>) -> Option<String>;
            plain stop_after_song(index: Option<usize>) -> ();
            plain clear_autoplay() -> ();
            plain get_queue_options() -> $crate::api::QueueOptions;
            plain set_queue_options(options: $crate::api::QueueOptions) -> ();
            plain get_status() -> $crate::audio::Status;
            plain get_dsp() -> $crate::audio::DspSettings;
            plain set_dsp(dsp: $crate::audio::DspSettings, device_gen: Option<u64>) -> ();
            blocking list_output_devices() -> $crate::api::OutputDevices;
            plain set_output_device(device: Option<$crate::audio::DeviceRef>) -> ();
            plain output_use_default() -> ();
            plain set_device_profiles(enabled: bool) -> ();
            plain eq_presets() -> Vec<$crate::api::Preset>;
            fallible parse_eq_file(text: Option<String>, path: Option<String>) -> $crate::api::ParametricImport;
            plain get_history() -> $crate::history::HistoryStats;
            plain get_app_settings() -> $crate::api::AppSettings;
            plain set_close_to_tray(enabled: bool) -> ();
            plain set_global_hotkeys(enabled: bool) -> ();
            plain health_report() -> $crate::health::Report;
            plain health_duplicates() -> Vec<$crate::health::DupGroup>;
            plain health_start(kind: String) -> bool;
            plain health_stop() -> ();
            blocking health_remove(ids: Vec<String>) -> $crate::health::Removed;
            plain attention_state() -> $crate::attention::Attention;
            plain attention_ignore(keys: Vec<String>, ignored: bool) -> $crate::attention::Attention;
            plain attention_hide(kind: String, hidden: bool) -> $crate::attention::Attention;
            plain covers_start(album_ids: Vec<String>) -> bool;
            plain covers_stop() -> ();
            plain covers_list() -> $crate::libfix::CoversFound;
            blocking covers_save(album_ids: Vec<String>) -> $crate::libfix::CoversSaved;
            plain covers_discard() -> ();
            plain set_lyrics_online(enabled: bool) -> ();
            plain set_fix_tags_online(enabled: bool) -> ();
            plain get_skips() -> $crate::api::Skips;
            plain set_skipped(ids: Vec<String>, skip: bool) -> ();
            plain set_skip_instrumentals(enabled: bool) -> ();
            plain get_marks() -> std::collections::HashMap<String, $crate::marks::Mark>;
            plain set_rating(ids: Vec<String>, rating: u8) -> ();
            plain set_loved(ids: Vec<String>, loved: bool) -> ();
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
            blocking playlist_export(name: String, ids: Vec<String>, path: Option<String>, options: $crate::playlist_io::ExportOptions) -> $crate::playlist_io::ExportDone;
            blocking playlist_export_all(items: Vec<$crate::playlist_io::ExportItem>, folder: Option<String>, options: $crate::playlist_io::ExportOptions) -> $crate::playlist_io::ExportAllDone;
            blocking playlist_import(paths: Vec<String>) -> Vec<$crate::playlist_io::ImportPreview>;
            blocking playlist_import_files() -> $crate::playlist_io::ImportFiles;
            blocking playlist_missing_save(source: String, path: Option<String>) -> $crate::playlist_io::SavedFile;
            blocking connect_listenbrainz(token: String) -> String;
            fallible disconnect_listenbrainz() -> ();
            plain set_listenbrainz_loved(enabled: bool) -> ();
            plain set_new_releases_online(enabled: bool) -> ();
            blocking lb_import_status() -> $crate::lbimport::Status;
            blocking lb_import_start() -> $crate::lbimport::Status;
            blocking lb_import_pause() -> ();
            blocking lb_import_discard() -> ();
            blocking lb_import_apply() -> $crate::lbimport::Applied;
            blocking lb_import_save_unmatched(path: Option<String>) -> $crate::playlist_io::SavedFile;
            blocking lb_loved_preview() -> $crate::lbloved::SyncPreview;
            blocking lb_loved_apply() -> $crate::lbloved::SyncDone;
            plain lb_loved_waiting() -> usize;
            blocking new_releases() -> $crate::lbreleases::View;
            blocking new_releases_cached() -> $crate::lbreleases::View;
            blocking write_tags(ids: Vec<String>, changes: $crate::tagedit::TagChanges) -> $crate::api::TagsSaved;
            blocking find_song_info(id: String, query: Option<String>) -> $crate::api::SongInfoResults;
            blocking save_song_info(id: String, info: $crate::autotag::Candidate, keep_title: bool, whole_album: bool) -> $crate::autotag::Saved;
            blocking find_album_info(album_id: String, query: Option<String>) -> $crate::libfix::AlbumResults;
            blocking album_plan(album_id: String, album_ref: String) -> $crate::libfix::AlbumPlan;
            blocking save_album_info(album_id: String, album: $crate::libfix::AlbumCandidate, songs: Vec<$crate::autotag::AlbumSong>, assign: Vec<$crate::libfix::Assign>, keep_titles: bool) -> $crate::libfix::AlbumSaved;
            plain fix_all_start() -> bool;
            plain fix_all_stop() -> ();
            plain artist_check_start() -> bool;
            plain artist_check_stop() -> ();
            plain artist_pairs() -> $crate::libfix::ArtistPairs;
            plain artist_pair_dismiss(native: String, latin: String) -> ();
            blocking merge_artist(from: String, to: String) -> $crate::libfix::Renamed;
            blocking set_artist_names_online(enabled: bool) -> ();
            plain artist_identity() -> $crate::identity::IdentityView;
            blocking search_keys() -> $crate::searchkeys::SearchKeys;
            blocking artist_mark_same(a: String, b: String) -> ();
            blocking artist_mark_different(name: String, others: Vec<String>) -> Vec<[String; 2]>;
            blocking artist_forget(pairs: Vec<[String; 2]>) -> ();
            blocking artist_names_status() -> $crate::musicbrainz::Status;
            blocking artist_names_pause(paused: bool) -> ();
            blocking artist_names_stop() -> ();
            plain fix_all_found() -> Vec<$crate::libfix::FoundInfo>;
            blocking fix_all_save(ids: Vec<String>) -> $crate::libfix::FixAllSaved;
            plain fix_all_discard() -> ();
            plain recent_changes() -> Vec<$crate::changes::ChangeInfo>;
            blocking undo_change(id: String) -> usize;
            blocking romanize_lyrics(lines: Vec<String>) -> Vec<String>;
            blocking ruby_lyrics(lines: Vec<String>) -> Vec<Vec<$crate::romanize::RubyPart>>;
            blocking translate_lyrics(track_id: String, lang: String) -> $crate::translate::Translation;
            plain translate_cancel(track_id: String) -> ();
            blocking lyric_search(query: String, limit: Option<usize>) -> $crate::lyricindex::Found;
            blocking find_covers(album_id: String, query: Option<String>) -> $crate::api::CoverResults;
            blocking set_album_cover(album_id: String, source: $crate::artists::Source) -> $crate::covers::Changed;
            plain artist_images() -> std::collections::HashMap<String, $crate::artists::ArtistImage>;
            blocking find_artist_photos(name: String) -> Vec<$crate::artists::Photo>;
            blocking set_artist_image(name: String, source: $crate::artists::Source) -> $crate::artists::ArtistImage;
            plain reset_artist_image(name: String) -> ();
            blocking search_lyrics(track_id: String, title: String, artist: String, album: String) -> Vec<$crate::lyrics::Hit>;
            blocking use_lyrics(track_id: String, hit_id: u64) -> $crate::lyrics::Lyrics;
            plain hide_lyrics(track_id: String) -> ();
            fallible paste_lyrics(track_id: String, text: String) -> $crate::lyrics::Lyrics;
            blocking ai_find_lyrics(track_id: String) -> $crate::lyrics::Lyrics;
            blocking find_singers(track_id: String) -> $crate::lyrics::Lyrics;
            plain debug_tap(on: bool) -> String;
            blocking backup_info() -> $crate::backup::BackupInfo;
            blocking backup_create(path: Option<String>, lyric_offsets: Option<String>) -> $crate::backup::Made;
            blocking backup_preview(path: String) -> $crate::backup::Preview;
            blocking backup_restore(path: String) -> $crate::backup::Restored;
            blocking backup_set_folder(folder: Option<String>) -> $crate::backup::BackupInfo;
            blocking backup_keep_offsets(lyric_offsets: String) -> ();
            bytes waveform(track_id: String) -> Vec<u8>;
            plain waveforms_prepare() -> bool;
            plain waveforms_pause(paused: bool) -> ();
            plain waveforms_stop() -> ();
            plain waveforms_status() -> $crate::waveform::JobStatus;
            blocking phone_summary() -> $crate::phone::Summary;
            plain phone_kept() -> $crate::phone::commands::Kept;
            plain phone_keep(kind: $crate::phone::Kind, id: String, keep: bool) -> ();
            plain phone_set_top(top: u32) -> ();
            plain phone_service() -> $crate::phone::commands::Service;
            plain set_phone_sync(enabled: bool) -> $crate::phone::commands::Service;
            fallible phone_pair() -> $crate::phone::commands::Service;
            fallible phone_unpair(phone_id: String) -> $crate::phone::commands::Service;
            fallible phone_pair_offer(link: String) -> $crate::phone::pair_link::Offer;
            plain phone_link() -> $crate::phone::commands::LinkView;
            blocking phone_link_pair(address: String, code: String, name: String) -> $crate::phone::commands::LinkView;
            fallible phone_link_forget() -> $crate::phone::commands::LinkView;
            fallible phone_sync_now() -> bool;
            plain phone_pause(paused: bool) -> ();
            plain phone_sync_stop() -> ();
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{artist_index, artist_pages, env_vars, not_all_saved, split_credit_like_ui, stat_artists, without_skipped};

    #[test]
    fn credits_split_like_the_window_splits_them() {
        let s = |c: &str| split_credit_like_ui(c);
        assert_eq!(s("A, B & C feat. D"), ["A", "B", "C", "D"]);
        assert_eq!(s("Hikaru Utada ft. Kenshi Yonezu"), ["Hikaru Utada", "Kenshi Yonezu"]);
        assert_eq!(s("Kenshi Yonezu featuring DAOKO"), ["Kenshi Yonezu", "DAOKO"]);
        assert_eq!(s("Hoshimachi Suisei x Kenshi"), ["Hoshimachi Suisei", "Kenshi"]);
        assert_eq!(s("A × B"), ["A", "B"]);
        // Inside words, nothing splits.
        assert_eq!(s("Defeat the Night"), ["Defeat the Night"]);
        assert_eq!(s("Rex Orange County"), ["Rex Orange County"]);
        assert_eq!(s("Featured"), ["Featured"]);
    }

    #[test]
    fn stats_artists_are_the_artist_pages() {
        let credits = ["Kenshi Yonezu", "Kenshi Yonezu & Masaki Suda", "Simon & Garfunkel", "Wuthering Waves & Crywolf", "Wuthering Waves"];
        let index = artist_index(credits.iter().copied());
        assert_eq!(index["Kenshi Yonezu & Masaki Suda"], ["Kenshi Yonezu"]);
        assert_eq!(index["Simon & Garfunkel"], ["Simon & Garfunkel"]);
        assert_eq!(index["Wuthering Waves & Crywolf"], ["Wuthering Waves"]);
        assert_eq!(index["Kenshi Yonezu"], ["Kenshi Yonezu"]);
    }

    #[test]
    fn compilation_plays_count_under_their_performers() {
        let credits = ["Kenshi Yonezu", "Various Artists", "YOASOBI", "Simon & Garfunkel"];
        let index = artist_index(credits.iter().copied());
        let pages = artist_pages(&index);
        let under = |artist: &str, album: &str| stat_artists(artist, album, &index, &pages);
        // A compilation: its performers, each one a page.
        assert_eq!(under("Kenshi Yonezu", "Various Artists"), ["Kenshi Yonezu"]);
        assert_eq!(under("YOASOBI & kenshi yonezu", "Various Artists"), ["YOASOBI", "Kenshi Yonezu"], "the page's spelling");
        assert_eq!(under("Simon & Garfunkel", "VA"), ["Simon & Garfunkel"], "a page's own name isn't split");
        // A performer without a page of their own: the compilation's page (it exists).
        assert_eq!(under("Nobody Here", "Various Artists"), ["Various Artists"]);
        // No album artist: the song's artist when it has a page.
        assert_eq!(under("YOASOBI", ""), ["YOASOBI"]);
        // An ordinary album stays under its album artist (a guest isn't counted).
        assert_eq!(under("Kenshi Yonezu feat. Hatsune Miku", "Kenshi Yonezu"), ["Kenshi Yonezu"]);
        // Every name returned is a page.
        for (a, b) in [("Kenshi Yonezu", "Various Artists"), ("Nobody", "Various Artists"), ("YOASOBI", "YOASOBI")] {
            for n in under(a, b) {
                assert!(pages.contains_key(&n.to_lowercase()), "{n}");
            }
        }
    }

    #[test]
    fn a_key_with_equals_signs_is_a_key() {
        assert!(env_vars("c2stYWJjZGVm==").is_empty());
        assert!(env_vars("sk-proj-abc=def").is_empty());
        let v = env_vars("# mine\nexport ZAI_API_KEY=\"abc==\"\nAI_MODEL=glm-5\n");
        assert_eq!(v["ZAI_API_KEY"], "abc==");
        assert_eq!(v["AI_MODEL"], "glm-5");
    }

    #[test]
    fn says_which_files_werent_saved() {
        let msg = not_all_saved(2, 3, &[("D:/Music/Lemon.flac".into(), "Access is denied.".into())]);
        assert_eq!(msg, "Saved 2 of 3. Couldn't save: Lemon.flac (Access is denied.).");
    }

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
