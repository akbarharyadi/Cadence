//! Fixing songs with no tags, like downloads named "Artist - Title.m4a" that show up as
//! "Unknown Artist".
//!
//! When such a song starts playing, Cadence looks it up in the iTunes catalogue (Apple's public
//! search, no account or key; then Deezer's, when iTunes has no exact match), checks that the
//! match really is this song (title, artist and length), and writes the title, artist, album,
//! year, track number, genre and cover into the file. The file is only written once it's no
//! longer playing, then the library is rescanned.
//! Each file is looked up once; misses aren't retried.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use serde::{Deserialize, Serialize};
use crate::ctx::Ctx;

use crate::library::Track;
use crate::state::AppState;

/// What the catalogue says about a song.
#[derive(Debug, Clone)]
pub struct Found {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub genre: String,
    pub cover_url: Option<String>,
}

#[derive(Default)]
struct Runtime {
    /// The song playing now (its file isn't written until it stops).
    playing: Option<String>,
    /// Lookups that finished while their song was playing: track id → (file, result).
    waiting: HashMap<String, (PathBuf, Found)>,
    /// Files looked up already (kept in autotag.json).
    tried: Option<HashSet<String>>,
    /// Other writes waiting for their song to stop (e.g. a new cover): track id → jobs.
    later: HashMap<String, Vec<Box<dyn FnOnce() + Send>>>,
}

/// Runs `job` (which writes the file of song `id`) once that song is no longer playing.
pub fn after_song(id: &str, job: Box<dyn FnOnce() + Send>) {
    runtime().lock().unwrap().later.entry(id.to_string()).or_default().push(job);
}

fn runtime() -> &'static Mutex<Runtime> {
    static RT: OnceLock<Mutex<Runtime>> = OnceLock::new();
    RT.get_or_init(Default::default)
}

/// Whether a song is missing the tags that matter.
pub fn untagged(t: &Track) -> bool {
    t.artist == "Unknown Artist"
}

/// Called when the playing song changes.
pub fn on_track_change(ctx: &Ctx, previous: Option<&str>, current: Option<&str>) {
    let state = ctx.state.clone();
    // Info waiting for its song to stop (found automatically, or picked in Fix Song Info).
    let (ready, jobs) = {
        let mut rt = runtime().lock().unwrap();
        rt.playing = current.map(str::to_string);
        let ready = previous.and_then(|p| rt.waiting.remove(p));
        let jobs = previous.and_then(|p| rt.later.remove(p)).unwrap_or_default();
        (ready, jobs)
    };
    if let Some((path, found)) = ready {
        let ctx = ctx.clone();
        std::thread::spawn(move || apply(&ctx, &path, &found));
    }
    if !jobs.is_empty() {
        std::thread::spawn(move || jobs.into_iter().for_each(|job| job()));
    }
    if !state.settings.lock().fix_tags_online {
        return;
    }

    let Some(id) = current else { return };
    let track = {
        let lib = state.library.lock();
        lib.by_id.get(id).map(|&i| lib.snapshot.tracks[i].clone())
    };
    let Some(track) = track.filter(untagged) else { return };
    {
        let mut rt = runtime().lock().unwrap();
        let tried = rt.tried.get_or_insert_with(|| load_tried(&state.data_dir));
        if !tried.insert(track.path.clone()) {
            return;
        }
        save_tried(&state.data_dir, tried);
    }
    let ctx = ctx.clone();
    let id = id.to_string();
    std::thread::spawn(move || {
        let Some(found) = lookup(&track) else {
            log::info!("no catalogue match for {}", track.path);
            return;
        };
        log::info!("matched {} → {} — {} ({})", track.path, found.artist, found.title, found.album);
        let path = PathBuf::from(&track.path);
        let mut rt = runtime().lock().unwrap();
        if rt.playing.as_deref() == Some(id.as_str()) {
            rt.waiting.insert(id, (path, found));
        } else {
            drop(rt);
            apply(&ctx, &path, &found);
        }
    });
}

fn tried_file(dir: &Path) -> PathBuf {
    dir.join("autotag.json")
}
fn load_tried(dir: &Path) -> HashSet<String> {
    std::fs::read(tried_file(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_tried(dir: &Path, tried: &HashSet<String>) {
    if let Ok(b) = serde_json::to_vec(tried) {
        let _ = std::fs::write(tried_file(dir), b);
    }
}

/// Writes what was found into the file and refreshes the library.
fn apply(ctx: &Ctx, path: &Path, f: &Found) {
    let cover = f.cover_url.as_deref().and_then(|u| download(u).ok());
    match write(path, f, cover) {
        Ok(()) => {
            log::info!("tagged {}", path.display());
            crate::jobs::start_scan(ctx.clone());
        }
        Err(e) => log::warn!("couldn't tag {}: {e}", path.display()),
    }
}

fn write(path: &Path, f: &Found, cover: Option<Vec<u8>>) -> Result<(), String> {
    let changes = crate::tagedit::TagChanges {
        title: Some(f.title.clone()),
        artist: Some(f.artist.clone()),
        album: Some(f.album.clone()),
        album_artist: Some(f.album_artist.clone()),
        genre: (!f.genre.is_empty()).then(|| f.genre.clone()),
        year: f.year.map(|y| y.to_string()),
        track_no: f.track_no.map(|n| n.to_string()),
        disc_no: f.disc_no.map(|n| n.to_string()),
    };
    crate::tagedit::write(path, &changes)?;
    let Some(cover) = cover else { return Ok(()) };
    let mut tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let Some(tag) = tagged.primary_tag_mut() else { return Ok(()) };
    if tag.pictures().iter().any(|p| p.pic_type() == PictureType::CoverFront) {
        return Ok(());
    }
    tag.push_picture(
        Picture::unchecked(cover)
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Jpeg)
            .build(),
    );
    tag.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let r = crate::net::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(r.status().to_string());
    }
    r.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
}

// ---------- Fix Song Info: the user picks the match ----------

/// A catalogue result offered in Fix Song Info.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub genre: String,
    /// 600 px cover, written into the file.
    pub cover_url: Option<String>,
    /// 100 px cover, for the list.
    pub thumb: Option<String>,
    pub duration: f64,
    /// Same title (and artist, when the file name has one) and about the same length.
    pub likely: bool,
}

impl From<Candidate> for Found {
    fn from(c: Candidate) -> Self {
        Found {
            title: c.title,
            artist: c.artist,
            album: c.album,
            album_artist: c.album_artist,
            year: c.year,
            track_no: c.track_no,
            disc_no: c.disc_no,
            genre: c.genre,
            cover_url: c.cover_url,
        }
    }
}

/// What to search for: "Artist Title" from the file name (or from the tags, if it has them).
pub fn search_term(t: &Track) -> String {
    match wanted(t) {
        (Some(a), title) => format!("{a} {title}"),
        (None, title) => title,
    }
}

/// The artist (if known) and title the song should have.
fn wanted(t: &Track) -> (Option<String>, String) {
    if untagged(t) {
        guess(&t.title)
    } else {
        (Some(t.artist.clone()), t.title.clone())
    }
}

/// How much of the search a result contains (0 to 1): the share of the search's words found in
/// its title, artist and album. Words in scripts without spaces (Japanese, Chinese) count when
/// they appear anywhere in it.
fn relevance(words: &[String], name: &str, by: &str, album: &str) -> f64 {
    if words.is_empty() {
        return 1.0;
    }
    let hay = format!("{name} {by} {album}");
    let hay_words: HashSet<String> = search_words(&hay).into_iter().collect();
    let hay_all = norm(&hay);
    let found = words.iter().filter(|w| hay_words.contains(*w) || (!w.is_ascii() && hay_all.contains(w.as_str()))).count();
    found as f64 / words.len() as f64
}

/// Lower-case words of letters and digits ("Don't" → "dont").
fn search_words(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || matches!(c, '-' | '–' | '—' | '|' | '/' | ',' | '&' | '(' | ')' | '[' | ']'))
        .map(norm)
        .filter(|w| !w.is_empty())
        .collect()
}

/// Results sharing less of the search than this are left out (unless they are the song).
const MIN_RELEVANCE: f64 = 0.4;

/// Catalogue results for `term`, the likeliest matches for `t` first.
pub fn candidates(t: &Track, term: &str) -> Result<Vec<Candidate>, String> {
    // "Artist - Title" typed into the search box says what the song should be, like a file name.
    let (artist, title) = match guess(term) {
        (Some(a), title) => (Some(a), title),
        _ => wanted(t),
    };
    let words = search_words(term);
    // The catalogue reads "Artist - Title" worse than "Artist Title".
    let term = match guess(term) {
        (Some(a), title) => format!("{a} {title}"),
        (None, _) => term.to_string(),
    };
    let term = term.as_str();
    let client = crate::net::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let mut out: Vec<(Rank4, Candidate)> = Vec::new();
    // Scores a result against the song; None when it's neither the song nor close to the search.
    let rank = |c: &Candidate| -> Option<Rank4> {
        let off = (c.duration - t.duration).abs();
        let likely = similar(&c.title, &title) && artist.as_deref().is_none_or(|a| similar(&c.artist, a)) && off < 75.0;
        let score = relevance(&words, &c.title, &c.artist, &c.album);
        if !likely && score < MIN_RELEVANCE {
            return None;
        }
        let label = format!("{} {}", c.title, c.album).to_lowercase();
        let other_version = VERSIONS.iter().any(|v| label.contains(v) && !title.to_lowercase().contains(v));
        // Not the song: the ones sharing most of the search first, then the closest length.
        Some(Rank4(!likely as u8, if likely { 0.0 } else { -(score * 10.0).round() }, other_version as u8, off))
    };
    let mut reached = false;
    for country in ["US", "ID", "JP", "KR", "GB"] {
        let url = format!(
            "https://itunes.apple.com/search?media=music&entity=song&limit=15&country={country}&term={}",
            urlencoding::encode(term)
        );
        let Ok(r) = client.get(&url).send() else { continue };
        let Ok(s) = r.json::<Search>() else { continue };
        reached = true;
        for h in s.results {
            let (Some(name), Some(by)) = (h.track_name.clone(), h.artist_name.clone()) else { continue };
            let album = h.collection_name.clone().unwrap_or_default();
            if !seen.insert(norm(&format!("{name}|{by}|{album}"))) {
                continue;
            }
            let mut c = Candidate {
                title: name,
                album_artist: h.collection_artist_name.clone().unwrap_or_else(|| by.clone()),
                artist: by,
                album,
                year: h.release_date.as_deref().and_then(|d| d.get(..4)?.parse().ok()),
                track_no: h.track_number,
                disc_no: h.disc_number,
                genre: h.primary_genre_name.clone().unwrap_or_default(),
                cover_url: h.artwork_url100.as_ref().map(|u| u.replace("100x100bb", "600x600bb")),
                thumb: h.artwork_url100.clone(),
                duration: h.track_time_millis.unwrap_or(0.0) / 1000.0,
                likely: false,
            };
            if let Some(r) = rank(&c) {
                c.likely = r.0 == 0;
                out.push((r, c));
            }
        }
        // Enough good matches: no need to ask other stores.
        if out.iter().filter(|(r, _)| r.0 == 0).count() >= 3 {
            break;
        }
    }
    // Deezer has releases Apple doesn't list (many game soundtracks and Asian singles).
    if let Some(found) = deezer_candidates(&client, term) {
        reached = true;
        let mut fresh: Vec<(Rank4, Candidate, u64)> = Vec::new();
        for (c, id) in found {
            // Already offered from iTunes: same title, artist and length (iTunes adds " - Single"
            // to album names, so those can't be compared).
            let dupe = out.iter().any(|(_, o)| {
                norm(&o.title) == norm(&c.title) && norm(&o.artist) == norm(&c.artist) && (o.duration - c.duration).abs() < 2.0
            });
            if dupe || !seen.insert(norm(&format!("deezer|{id}"))) {
                continue;
            }
            if let Some(r) = rank(&c) {
                let mut c = c;
                c.likely = r.0 == 0;
                fresh.push((r, c, id));
            }
        }
        // The search lists only the main artist and no date: ask for the best few in full.
        fresh.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        fresh.truncate(6);
        std::thread::scope(|s| {
            for (_, c, id) in fresh.iter_mut() {
                let client = &client;
                s.spawn(move || {
                    if let Some(more) = deezer_details(client, *id) {
                        if let Some(a) = more.artist {
                            c.artist = a;
                        }
                        c.year = more.year;
                        c.track_no = more.track_no;
                        c.disc_no = more.disc_no;
                    }
                });
            }
        });
        out.extend(fresh.into_iter().map(|(r, c, _)| (r, c)));
    }
    if !reached {
        return Err("Couldn't reach the iTunes or Deezer catalogue. Check your internet connection.".into());
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out.into_iter().take(12).map(|(_, c)| c).collect())
}

/// Songs for `term` in Deezer's public catalogue (no account or key), with their Deezer ids.
/// None when Deezer can't be reached.
fn deezer_candidates(client: &reqwest::blocking::Client, term: &str) -> Option<Vec<(Candidate, u64)>> {
    let url = format!("https://api.deezer.com/search?q={}&limit=15", urlencoding::encode(term));
    let v: serde_json::Value = client.get(&url).send().ok()?.json().ok()?;
    let hits = v["data"].as_array()?;
    Some(
        hits.iter()
            .filter_map(|h| {
                let id = h["id"].as_u64()?;
                let title = h["title"].as_str()?.to_string();
                let by = h["artist"]["name"].as_str()?.to_string();
                let album = h["album"]["title"].as_str().unwrap_or("").to_string();
                let album = if matches!(album.to_lowercase().as_str(), "unknown" | "unknown album") { String::new() } else { album };
                let cover = |size: &str| h["album"][size].as_str().filter(|u| !u.is_empty()).map(str::to_string);
                Some((
                    Candidate {
                        title,
                        album_artist: by.clone(),
                        artist: by,
                        album,
                        year: None,
                        track_no: None,
                        disc_no: None,
                        genre: String::new(),
                        cover_url: cover("cover_xl").or_else(|| cover("cover_big")),
                        thumb: cover("cover_medium"),
                        duration: h["duration"].as_f64().unwrap_or(0.0),
                        likely: false,
                    },
                    id,
                ))
            })
            .collect(),
    )
}

/// What Deezer's page for a song adds to its search result.
struct DeezerMore {
    /// Everyone credited ("Wuthering Waves, jixwang & Vision Sound"), when more than one.
    artist: Option<String>,
    year: Option<u32>,
    track_no: Option<u32>,
    disc_no: Option<u32>,
}

/// Deezer's page for song `id`: the search lists only the main artist and no date.
fn deezer_details(client: &reqwest::blocking::Client, id: u64) -> Option<DeezerMore> {
    let v: serde_json::Value = client.get(format!("https://api.deezer.com/track/{id}")).send().ok()?.json().ok()?;
    if v.get("error").is_some() {
        return None;
    }
    let names: Vec<&str> = v["contributors"].as_array().into_iter().flatten().filter_map(|a| a["name"].as_str()).collect();
    let artist = names.split_last().filter(|(_, rest)| !rest.is_empty()).map(|(last, rest)| format!("{} & {last}", rest.join(", ")));
    Some(DeezerMore {
        artist,
        year: v["release_date"].as_str().and_then(|d| d.get(..4)?.parse().ok()).filter(|&y| y > 0),
        track_no: v["track_position"].as_u64().map(|n| n as u32).filter(|&n| n > 0),
        disc_no: v["disk_number"].as_u64().map(|n| n as u32).filter(|&n| n > 0),
    })
}

/// Saves the info the user picked. A playing song shows it at once and its file is written
/// when it stops (returns true then).
pub fn save_choice(ctx: &Ctx, id: &str, choice: Candidate) -> Result<bool, String> {
    let state = ctx.state.clone();
    let path = {
        let mut lib = state.library.lock();
        let i = *lib.by_id.get(id).ok_or("Unknown song")?;
        let t = &mut lib.snapshot.tracks[i];
        t.title = choice.title.clone();
        t.artist = choice.artist.clone();
        t.album = choice.album.clone();
        t.album_artist = choice.album_artist.clone();
        t.year = choice.year;
        PathBuf::from(&t.path)
    };
    let playing = {
        let s = state.engine.status();
        s.index.and_then(|i| s.queue.get(i).cloned())
    };
    let found: Found = choice.into();
    let mut rt = runtime().lock().unwrap();
    let tried = rt.tried.get_or_insert_with(|| load_tried(&state.data_dir));
    if tried.insert(path.to_string_lossy().into_owned()) {
        save_tried(&state.data_dir, tried);
    }
    if playing.as_deref() == Some(id) {
        rt.playing = Some(id.to_string());
        rt.waiting.insert(id.to_string(), (path, found.clone()));
        drop(rt);
        show_now(&state, id, &found);
        ctx.emit("library-changed", ());
        return Ok(true);
    }
    rt.waiting.remove(id);
    drop(rt);
    let cover = found.cover_url.as_deref().and_then(|u| download(u).ok());
    write(&path, &found, cover)?;
    crate::jobs::start_scan(ctx.clone());
    Ok(false)
}

/// A playing song's file can't be written yet: show its new album and cover in the library now.
fn show_now(state: &AppState, id: &str, found: &Found) {
    let art_dir = state.art_dir();
    let album_id = crate::library::album_id_for(&found.album_artist, &found.album);
    let has_art = art_dir.join(format!("{album_id}_s.jpg")).exists();
    if !has_art {
        if let Some(bytes) = found.cover_url.as_deref().and_then(|u| download(u).ok()) {
            crate::library::write_art(&album_id, &bytes, &art_dir);
        }
    }
    let mut lib = state.library.lock();
    let Some(&i) = lib.by_id.get(id) else { return };
    lib.snapshot.tracks[i].album_id = album_id;
    let mut tracks = lib.snapshot.tracks.clone();
    let albums = crate::library::build_albums(&mut tracks, &art_dir);
    lib.set_tracks(tracks, albums);
}

// ---------- finding the song ----------

/// "ADA Band - Haruskah Kumati (Official Music Video)" → (Some("ADA Band"), "Haruskah Kumati").
pub fn guess(title: &str) -> (Option<String>, String) {
    let clean = strip_copy_number(&strip_noise(title));
    for sep in [" - ", " – ", " — ", " | "] {
        if let Some((a, t)) = clean.split_once(sep) {
            let (a, t) = (a.trim(), strip_noise(t.trim()));
            if !a.is_empty() && !t.is_empty() {
                return (Some(a.to_string()), t);
            }
        }
    }
    (None, clean)
}

/// "Song 1" / "Song (2)": the number a download tool adds to a second copy of a file.
fn strip_copy_number(s: &str) -> String {
    let t = s.trim_end();
    let inner = t.strip_suffix(')').and_then(|r| r.strip_suffix(|c: char| c.is_ascii_digit())?.strip_suffix('('));
    let plain = t.strip_suffix(|c: char| c.is_ascii_digit()).filter(|r| r.ends_with(' '));
    inner.or(plain).map(str::trim_end).filter(|r| !r.is_empty()).unwrap_or(t).to_string()
}

/// Removes bracketed extras that video uploads add: (Official Video), [Lyrics], 【MV】, | HD...
fn strip_noise(s: &str) -> String {
    const NOISE: [&str; 14] = [
        "official", "video", "audio", "lyric", "lirik", "mv", "m/v", "hd", "4k", "visualizer", "music video", "clip", "full", "with lyrics",
    ];
    let mut out = String::new();
    let mut depth = 0usize;
    let mut chunk = String::new();
    let open = |c: char| matches!(c, '(' | '[' | '【' | '「');
    let close = |c: char| matches!(c, ')' | ']' | '】' | '」');
    for c in s.chars() {
        if open(c) {
            if depth == 0 {
                chunk.clear();
            }
            depth += 1;
            chunk.push(c);
        } else if close(c) && depth > 0 {
            depth -= 1;
            chunk.push(c);
            if depth == 0 {
                let lower = chunk.to_lowercase();
                // Keep meaningful brackets such as (feat. X), (Live), (Acoustic).
                if !NOISE.iter().any(|n| lower.contains(n)) {
                    out.push_str(&chunk);
                }
            }
        } else if depth > 0 {
            chunk.push(c);
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Letters and digits only, lower case, for comparing names.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    results: Vec<Hit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    track_name: Option<String>,
    artist_name: Option<String>,
    collection_name: Option<String>,
    collection_artist_name: Option<String>,
    track_time_millis: Option<f64>,
    release_date: Option<String>,
    track_number: Option<u32>,
    disc_number: Option<u32>,
    primary_genre_name: Option<String>,
    artwork_url100: Option<String>,
    /// Full-size cover, for a Deezer result.
    #[serde(skip)]
    cover: Option<String>,
    #[serde(skip)]
    deezer_id: Option<u64>,
}

/// Searches the catalogue for this song; several stores, since local music is often only
/// listed in its own country's store.
pub fn lookup(t: &Track) -> Option<Found> {
    let (artist, title) = guess(&t.title);
    let term = match &artist {
        Some(a) => format!("{a} {title}"),
        None => title.clone(),
    };
    let client = crate::net::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let mut best_so_far: Option<(Rank, Found)> = None;
    for country in ["US", "ID", "JP", "KR", "GB"] {
        let url = format!(
            "https://itunes.apple.com/search?media=music&entity=song&limit=25&country={country}&term={}",
            urlencoding::encode(&term)
        );
        let Ok(r) = client.get(&url).send() else { continue };
        let Ok(s) = r.json::<Search>() else { continue };
        if let Some((rank, found, _)) = best(&s.results, artist.as_deref(), &title, t.duration) {
            if best_so_far.as_ref().is_none_or(|(r, _)| rank < *r) {
                best_so_far = Some((rank, found));
            }
            // The original recording with the exact title: no need to ask other stores.
            if best_so_far.as_ref().is_some_and(|(r, _)| r.0 == 0 && r.1 == 0) {
                break;
            }
        }
    }
    // No exact original on iTunes: Deezer lists releases Apple doesn't (game soundtracks, Asian singles).
    if !best_so_far.as_ref().is_some_and(|(r, _)| r.0 == 0 && r.1 == 0) {
        let hits: Vec<Hit> = deezer_candidates(&client, &term)
            .unwrap_or_default()
            .into_iter()
            .map(|(c, id)| Hit {
                track_name: Some(c.title),
                artist_name: Some(c.artist),
                collection_name: Some(c.album),
                collection_artist_name: None,
                track_time_millis: Some(c.duration * 1000.0),
                release_date: None,
                track_number: None,
                disc_number: None,
                primary_genre_name: None,
                artwork_url100: None,
                cover: c.cover_url,
                deezer_id: Some(id),
            })
            .collect();
        if let Some((rank, mut found, id)) = best(&hits, artist.as_deref(), &title, t.duration) {
            if best_so_far.as_ref().is_none_or(|(r, _)| rank < *r) {
                if let Some(more) = id.and_then(|id| deezer_details(&client, id)) {
                    if let Some(a) = more.artist {
                        found.artist = a;
                    }
                    found.year = more.year;
                    found.track_no = more.track_no;
                    found.disc_no = more.disc_no;
                }
                best_so_far = Some((rank, found));
            }
        }
    }
    best_so_far.map(|(_, f)| f)
}

/// Words that mark another recording of a song (unless the file name has them too).
const VERSIONS: [&str; 12] = [
    "live", "acoustic", "piano", "remix", "instrumental", "karaoke", "version", "cover", "remaster", "mix)", "edit", "orchestra",
];

/// Lower is better: (another version?, title not exact?, seconds off).
#[derive(PartialEq, PartialOrd)]
struct Rank(u8, u8, f64);

/// Fix Song Info order, lower first: (not the song?, minus share of the search, another version?, seconds off).
#[derive(PartialEq, PartialOrd)]
struct Rank4(u8, f64, u8, f64);

/// The result that is really this song, if any, preferring the original recording.
/// Also its Deezer id, for a Deezer result.
fn best(hits: &[Hit], artist: Option<&str>, title: &str, duration: f64) -> Option<(Rank, Found, Option<u64>)> {
    let wanted = title.to_lowercase();
    hits.iter()
        .filter_map(|h| {
            let name = h.track_name.as_deref()?;
            let by = h.artist_name.as_deref()?;
            let length = h.track_time_millis.unwrap_or(0.0) / 1000.0;
            let off = (length - duration).abs();
            let title_ok = similar(name, title);
            let artist_ok = artist.is_some_and(|a| similar(by, a));
            let exact = norm(name) == norm(title);
            // With the artist and title both matching, a video's longer intro or outro is fine;
            // with only the title to go on, the title must be exact and the length match closely.
            let ok = title_ok && (if artist_ok { off < 75.0 } else { artist.is_none() && exact && off < 4.0 });
            if !ok {
                return None;
            }
            let label = format!("{} {}", name, h.collection_name.as_deref().unwrap_or("")).to_lowercase();
            let other_version = VERSIONS.iter().any(|v| label.contains(v) && !wanted.contains(v));
            Some((Rank(other_version as u8, (!exact) as u8, off), h))
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(rank, h)| {
            (
                rank,
                Found {
                    title: h.track_name.clone().unwrap_or_default(),
                    artist: h.artist_name.clone().unwrap_or_default(),
                    album: h.collection_name.clone().unwrap_or_default(),
                    album_artist: h
                        .collection_artist_name
                        .clone()
                        .or_else(|| h.artist_name.clone())
                        .unwrap_or_default(),
                    year: h.release_date.as_deref().and_then(|d| d.get(..4)?.parse().ok()),
                    track_no: h.track_number,
                    disc_no: h.disc_number,
                    genre: h.primary_genre_name.clone().unwrap_or_default(),
                    // The search gives a 100 px cover; the same address serves larger sizes.
                    cover_url: h.cover.clone().or_else(|| h.artwork_url100.as_ref().map(|u| u.replace("100x100bb", "600x600bb"))),
                },
                h.deezer_id,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the internet: `cargo test deezer_finds -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn deezer_finds_songs_apple_doesnt_list() {
        let t = Track {
            title: "悠忽舞于梦中".into(),
            artist: "鸣潮先约电台/jixwang/VISION SOUND".into(),
            duration: 200.0,
            ..Default::default()
        };
        let found = candidates(&t, "wuthering waves Dancing through Fantasies").unwrap();
        for c in &found {
            println!("{} | {} | {} | {:?} | {}", c.title, c.artist, c.album, c.year, c.duration);
        }
        let top = &found[0];
        assert_eq!(top.title, "Dancing Through Fantasies");
        assert!(top.artist.contains("jixwang"), "{}", top.artist);
        assert_eq!(top.year, Some(2025));
    }

    #[test]
    fn guesses_artist_and_title_from_upload_names() {
        assert_eq!(
            guess("ADA Band - Haruskah Kumati (Official Music Video)"),
            (Some("ADA Band".into()), "Haruskah Kumati".into())
        );
        assert_eq!(
            guess("My Chemical Romance - It's Not a Fashion Statement [Lyrics]"),
            (Some("My Chemical Romance".into()), "It's Not a Fashion Statement".into())
        );
        assert_eq!(guess("Song (feat. Someone)"), (None, "Song (feat. Someone)".into()));
        assert_eq!(guess("【MV】Title"), (None, "Title".into()));
        assert_eq!(guess("ChouCho - Yasashisa no Riyu 1"), (Some("ChouCho".into()), "Yasashisa no Riyu".into()));
        assert_eq!(guess("Ello - Masih Ada (2)"), (Some("Ello".into()), "Masih Ada".into()));
        assert_eq!(guess("D_MASIV - Rindu 1_2 Mati"), (Some("D_MASIV".into()), "Rindu 1_2 Mati".into()));
    }

    /// Live lookups against the catalogue: `cargo test --lib autotag -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_lookups() {
        for (name, secs) in [
            ("ALEXA - Jangan Kau Lepas", 242.0),
            ("Chiai Fujikawa - Sagashimono", 291.0),
            ("ChouCho - Yasashisa no Riyu 1", 260.0),
            ("Compilations - Platinum Disco", 257.0),
            ("D_MASIV - Rindu 1_2 Mati", 293.0),
            ("Drive - Katakanlah", 280.0),
            ("Ello - Masih Ada", 239.0),
            ("ADA Band - Haruskah Kumati", 280.0),
        ] {
            let t = Track { title: name.into(), artist: "Unknown Artist".into(), duration: secs, ..Default::default() };
            match lookup(&t) {
                Some(f) => println!("{name:40} -> {} | {} | {} | {:?}", f.artist, f.title, f.album, f.year),
                None => println!("{name:40} -> (no match)"),
            }
        }
    }

    /// Looks up a real file and writes the result into a copy of it:
    /// `CADENCE_AUTOTAG_FILE=path cargo test --lib autotag::tests::tags_a_copy -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn tags_a_copy() {
        let Ok(src) = std::env::var("CADENCE_AUTOTAG_FILE") else { return };
        let dir = std::env::temp_dir().join("cadence-autotag-test");
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join(Path::new(&src).file_name().unwrap());
        std::fs::copy(&src, &copy).unwrap();
        let t = crate::library::read_track(&copy).expect("readable");
        assert!(untagged(&t), "test file should be untagged");
        let found = lookup(&t).expect("a match");
        let cover = found.cover_url.as_deref().and_then(|u| download(u).ok());
        println!("cover: {} bytes", cover.as_ref().map_or(0, Vec::len));
        write(&copy, &found, cover).unwrap();
        let after = crate::library::read_track(&copy).unwrap();
        println!("{} | {} | {} | {:?} | track {:?} | art {}", after.artist, after.title, after.album, after.year, after.track_no, after.has_embedded_art);
        assert_eq!(after.artist, found.artist);
        assert!(after.has_embedded_art);
        // The audio is untouched: same length.
        assert!((after.duration - t.duration).abs() < 0.05);
    }

    /// Needs the internet: a download Apple doesn't list is tagged from Deezer.
    #[test]
    #[ignore]
    fn lookup_falls_back_to_deezer() {
        let t = Track {
            title: "Wuthering Waves - Dancing Through Fantasies".into(),
            artist: "Unknown Artist".into(),
            duration: 200.0,
            ..Default::default()
        };
        let f = lookup(&t).expect("found");
        println!("{f:?}");
        assert_eq!(f.title, "Dancing Through Fantasies");
        assert_eq!(f.artist, "Wuthering Waves, jixwang & Vision Sound");
        assert_eq!(f.album_artist, "Wuthering Waves");
        assert_eq!(f.year, Some(2025));
        assert!(f.cover_url.is_some_and(|u| u.contains("dzcdn")));
    }

    #[test]
    fn only_accepts_real_matches() {
        let hit = |name: &str, by: &str, secs: f64| Hit {
            track_name: Some(name.into()),
            artist_name: Some(by.into()),
            collection_name: Some("Album".into()),
            collection_artist_name: None,
            track_time_millis: Some(secs * 1000.0),
            release_date: Some("2004-05-01T07:00:00Z".into()),
            track_number: Some(3),
            disc_number: Some(1),
            primary_genre_name: Some("Rock".into()),
            artwork_url100: Some("https://x/100x100bb.jpg".into()),
            cover: None,
            deezer_id: None,
        };
        let hits = [
            hit("Other Song", "ADA Band", 240.0),
            hit("Haruskah Kumati (Live)", "ADA Band", 279.0),
            hit("Haruskah Kumati", "ADA Band", 251.0),
        ];
        let f = best(&hits, Some("ADA Band"), "Haruskah Kumati", 280.0).unwrap().1;
        assert_eq!((f.title.as_str(), f.year), ("Haruskah Kumati", Some(2004)));
        assert_eq!(f.cover_url.as_deref(), Some("https://x/600x600bb.jpg"));
        // The live recording is closer in length, but the original is preferred.
        assert_eq!(best(&hits, Some("ADA Band"), "Haruskah Kumati (Live)", 279.0).unwrap().1.title, "Haruskah Kumati (Live)");
        // Same title by someone else: rejected.
        assert!(best(&hits, Some("Somebody"), "Haruskah Kumati", 251.0).is_none());
        // No artist known: the length has to match closely.
        assert!(best(&hits, None, "Haruskah Kumati", 280.0).is_none());
        assert!(best(&hits, None, "Haruskah Kumati", 252.0).is_some());
    }
}

#[cfg(test)]
mod fix_tests {
    #[test]
    fn relevance_counts_shared_words() {
        let words = super::search_words("My Chemical Romance - My Way Home Is Through You");
        // Same band, other song: kept, below the song itself.
        assert!(super::relevance(&words, "I Don't Love You", "My Chemical Romance", "The Black Parade") >= super::MIN_RELEVANCE);
        // Shares one word ("Chemical", "You"): left out.
        assert!(super::relevance(&words, "Chemical", "VIXX", "CONTINUUM - EP") < super::MIN_RELEVANCE);
        assert!(super::relevance(&words, "You", "benny blanco, Marshmello & Vance Joy", "You - Single") < super::MIN_RELEVANCE);
        assert!(super::relevance(&words, "The Nights", "Avicii", "Stories") < super::MIN_RELEVANCE);
        // Japanese searches have no spaces between words.
        let jp = super::search_words("米津玄師 Lemon");
        assert_eq!(super::relevance(&jp, "Lemon", "米津玄師", "Lemon - Single"), 1.0);
    }

    /// Live: `cargo test --lib fix_tests -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn offers_matches_for_a_download() {
        let t = crate::library::Track {
            title: "ST12 - Biarkan Jatuh Cinta".into(),
            artist: "Unknown Artist".into(),
            duration: 242.0,
            ..Default::default()
        };
        let term = super::search_term(&t);
        let found = super::candidates(&t, &term).unwrap();
        for c in &found {
            println!("{} | {} | {} | {:.0}s | likely {}", c.title, c.artist, c.album, c.duration, c.likely);
        }
        assert!(found.first().is_some_and(|c| c.likely));
    }

    /// Live: a demo the iTunes search doesn't find gets only related results there. It came out on
    /// The Black Parade's 20th anniversary edition (2026), which Deezer lists: that's the only
    /// result marked as the song.
    #[test]
    #[ignore]
    fn unreleased_song_gets_related_results_only() {
        let t = crate::library::Track {
            title: "My Chemical Romance - My Way Home Is Through Yo".into(),
            artist: "Unknown Artist".into(),
            duration: 178.0,
            ..Default::default()
        };
        for term in [super::search_term(&t), "My Chemical Romance - My Way Home Is Through You".to_string()] {
            let found = super::candidates(&t, &term).unwrap();
            println!("== {term}");
            for c in &found {
                println!("{} | {} | {} | {:.0}s | likely {}", c.title, c.artist, c.album, c.duration, c.likely);
            }
            assert!(found.iter().all(|c| c.artist == "My Chemical Romance"));
            assert!(found.iter().filter(|c| c.likely).all(|c| c.title.starts_with("My Way Home Is Through You")));
            assert!(found[0].likely && (found[0].duration - 178.0).abs() < 2.0);
        }
    }
}
