//! Fixing the library's info in bulk, from Needs Attention and the album menu:
//!
//! - Fix Album Info: find the album in the iTunes or Deezer catalogue, line its songs up with the
//!   files (by length and kind of recording, then track number; the user can change any), and write
//!   the album's and each song's info at once.
//! - Fix all: looks up every song without tags, with the same strict check as when one plays.
//! - Artists under two names: an artist tagged in Japanese or Chinese letters on some songs and in
//!   Latin ones on others (米津玄師 / Kenshi Yonezu) shows up twice. The catalogue says which names
//!   are the same artist; the user picks the one to keep.
//!
//! The two batch jobs run one at a time on a low-priority thread, ask the iTunes catalogue at most
//! once every 3 seconds (`autotag::paced`), report their progress for the sidebar and can be stopped.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autotag::{self, AlbumSong, Found, cjk, norm, split_credit, untagged};
use crate::ctx::Ctx;
use crate::library::Track;

fn client() -> Result<reqwest::blocking::Client, String> {
    crate::net::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

fn get(client: &reqwest::blocking::Client, url: &str) -> Option<Value> {
    client.get(url).send().ok()?.json().ok()
}

/// The album's songs in the library (not songs of a single-file album, which can't be written one by one).
fn album_tracks(ctx: &Ctx, album_id: &str) -> Vec<Track> {
    let lib = ctx.state.library.lock();
    let mut tracks: Vec<Track> = lib.snapshot.tracks.iter().filter(|t| t.album_id == album_id && t.cue.is_none()).cloned().collect();
    tracks.sort_by(|a, b| (a.disc_no, a.track_no, &a.title).cmp(&(b.disc_no, b.track_no, &b.title)));
    tracks
}

// ---------- Fix Album Info ----------

/// An album in the catalogue.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumCandidate {
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    #[serde(default)]
    pub genre: String,
    pub cover_url: Option<String>,
    pub thumb: Option<String>,
    /// How many songs it has.
    pub tracks: u32,
    /// "deezer:696220481" or "itunes:US:1440833098".
    pub album_ref: String,
    /// "iTunes" or "Deezer".
    pub source: String,
    /// Every song of the album in the library matches one of its songs (by length and kind).
    #[serde(default)]
    pub likely: bool,
    /// How many of the album's files match one of its songs (for the likeliest few results).
    #[serde(default)]
    pub matched: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumResults {
    term: String,
    results: Vec<AlbumCandidate>,
}

/// Catalogue albums for `query` (or the album's artist and name).
pub fn find_album_info(ctx: &Ctx, album_id: &str, query: Option<String>) -> Result<AlbumResults, String> {
    let tracks = album_tracks(ctx, album_id);
    let first = tracks.first().ok_or("This album has no songs Cadence can change one by one.")?;
    let artist = if first.album_artist == "Unknown Artist" { String::new() } else { first.album_artist.clone() };
    let term = query
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| format!("{artist} {}", first.album).trim().to_string());
    let client = client()?;
    let mut results = search_albums(&client, &term);
    // An album named in Chinese or Japanese is often listed in English only: its artists' albums
    // with the same number of songs, found by the names in Latin letters.
    if !results.iter().any(|c| c.tracks as usize == tracks.len()) && cjk(&first.album) {
        let mut names: Vec<String> = Vec::new();
        for t in &tracks {
            for n in split_credit(&t.artist).into_iter().chain(split_credit(&t.album_artist)) {
                if !cjk(&n) && !names.iter().any(|x| norm(x) == norm(&n)) {
                    names.push(n);
                }
            }
        }
        if !names.is_empty() {
            let more = search_albums(&client, &names.join(" "));
            results.extend(more.into_iter().filter(|c| c.tracks as usize == tracks.len()));
        }
    }
    let words = autotag::search_words(&term);
    let mut seen = HashSet::new();
    let mut ranked: Vec<(f64, u32, AlbumCandidate)> = results
        .into_iter()
        .filter(|c| seen.insert(format!("{}|{}|{}", norm(&c.title), norm(&c.artist), c.tracks)))
        .map(|c| {
            let off = (c.tracks as i64 - tracks.len() as i64).unsigned_abs() as u32;
            (-autotag::relevance(&words, &c.title, &c.artist, ""), off, c)
        })
        .collect();
    ranked.sort_by(|a, b| (a.1, a.0).partial_cmp(&(b.1, b.0)).unwrap_or(std::cmp::Ordering::Equal));
    let mut results: Vec<AlbumCandidate> = ranked.into_iter().take(12).map(|r| r.2).collect();
    // The names can mislead (another single of the same artists shares most words): the songs
    // can't. The likeliest few albums' track lists are read, and an album is the one when every
    // file matches one of its songs by length and kind of recording.
    std::thread::scope(|s| {
        for c in results.iter_mut().filter(|c| c.tracks as usize + 2 >= tracks.len()).take(6) {
            let (client, tracks) = (&client, &tracks);
            s.spawn(move || {
                let songs = autotag::album_songs(client, &c.album_ref);
                let mut used = HashSet::new();
                for t in tracks {
                    if let Some(i) = autotag::match_song(t, &songs, &used) {
                        used.insert(i);
                    }
                }
                c.matched = used.len() as u32;
                c.likely = !tracks.is_empty() && used.len() == tracks.len();
            });
        }
    });
    // Stable: within the same number of matching songs, the order above stays.
    results.sort_by_key(|c| (!c.likely, std::cmp::Reverse(c.matched)));
    Ok(AlbumResults { term, results })
}

/// Albums for `term` in a few iTunes stores and Deezer.
fn search_albums(client: &reqwest::blocking::Client, term: &str) -> Vec<AlbumCandidate> {
    let q = urlencoding::encode(term).into_owned();
    let mut out = Vec::new();
    for country in ["US", "JP", "ID"] {
        autotag::pace_itunes();
        let Some(v) = get(client, &format!("https://itunes.apple.com/search?media=music&entity=album&limit=12&country={country}&term={q}")) else {
            continue;
        };
        for h in v["results"].as_array().into_iter().flatten() {
            let (Some(title), Some(id)) = (h["collectionName"].as_str(), h["collectionId"].as_u64()) else { continue };
            let art = h["artworkUrl100"].as_str().map(str::to_string);
            out.push(AlbumCandidate {
                title: title.to_string(),
                artist: h["artistName"].as_str().unwrap_or("").to_string(),
                year: h["releaseDate"].as_str().and_then(|d| d.get(..4)?.parse().ok()),
                genre: h["primaryGenreName"].as_str().unwrap_or("").to_string(),
                cover_url: art.as_ref().map(|u| u.replace("100x100bb", "600x600bb")),
                thumb: art,
                tracks: h["trackCount"].as_u64().unwrap_or(0) as u32,
                album_ref: format!("itunes:{country}:{id}"),
                source: "iTunes".into(),
                likely: false,
                matched: 0,
            });
        }
    }
    if let Some(v) = get(client, &format!("https://api.deezer.com/search/album?q={q}&limit=15")) {
        for h in v["data"].as_array().into_iter().flatten() {
            let (Some(title), Some(id)) = (h["title"].as_str(), h["id"].as_u64()) else { continue };
            let cover = |size: &str| h[size].as_str().filter(|u| !u.is_empty()).map(str::to_string);
            out.push(AlbumCandidate {
                title: title.to_string(),
                artist: h["artist"]["name"].as_str().unwrap_or("").to_string(),
                year: None,
                genre: String::new(),
                cover_url: cover("cover_xl").or_else(|| cover("cover_big")),
                thumb: cover("cover_medium"),
                tracks: h["nb_tracks"].as_u64().unwrap_or(0) as u32,
                album_ref: format!("deezer:{id}"),
                source: "Deezer".into(),
                likely: false,
                matched: 0,
            });
        }
    }
    out
}

/// Which catalogue song each of the album's files is.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assign {
    pub track_id: String,
    /// Index into the album's songs; none when the file isn't on the catalogue's album.
    pub song: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPlan {
    songs: Vec<AlbumSong>,
    assign: Vec<Assign>,
    /// From the album's page (Deezer's search gives neither).
    year: Option<u32>,
    genre: String,
}

/// The catalogue album's songs, and the likeliest one for each file of the album.
pub fn album_plan(ctx: &Ctx, album_id: &str, album_ref: &str) -> Result<AlbumPlan, String> {
    let client = client()?;
    let songs = autotag::album_songs(&client, album_ref);
    if songs.is_empty() {
        return Err("Couldn't read this album's songs from the catalogue. Check your internet connection.".into());
    }
    let tracks = album_tracks(ctx, album_id);
    let mut used = HashSet::new();
    let mut assign: Vec<Assign> = tracks
        .iter()
        .map(|t| {
            let song = autotag::match_song(t, &songs, &used);
            used.extend(song);
            Assign { track_id: t.id.clone(), song }
        })
        .collect();
    // Same number of songs, same order, lengths close (a rip's gaps can add a few seconds): the
    // rest go by position.
    if tracks.len() == songs.len() {
        for (i, a) in assign.iter_mut().enumerate() {
            if a.song.is_none() && !used.contains(&i) && (songs[i].duration - tracks[i].duration).abs() < 10.0 {
                a.song = Some(i);
                used.insert(i);
            }
        }
    }
    let (mut year, mut genre) = (None, String::new());
    if let Some(id) = album_ref.strip_prefix("deezer:")
        && let Some(v) = get(&client, &format!("https://api.deezer.com/album/{id}")) {
            year = v["release_date"].as_str().and_then(|d| d.get(..4)?.parse().ok()).filter(|&y| y > 0);
            genre = v["genres"]["data"][0]["name"].as_str().unwrap_or("").to_string();
        }
    Ok(AlbumPlan { songs, assign, year, genre })
}

/// What Fix Album Info saved.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSaved {
    songs: usize,
    /// Songs given their own title, artist and number from the catalogue.
    matched: usize,
    /// The playing song's file is written when it ends.
    later: bool,
    /// The change, to undo it.
    change: Option<String>,
}

/// Writes the album's info into all its files, and each matched song's own title (unless
/// `keep_titles`: then the catalogue's goes into the subtitle), artist and number.
pub fn save_album_info(
    ctx: &Ctx,
    album_id: &str,
    album: AlbumCandidate,
    songs: Vec<AlbumSong>,
    assign: Vec<Assign>,
    keep_titles: bool,
) -> Result<AlbumSaved, String> {
    let client = client()?;
    let tracks = album_tracks(ctx, album_id);
    let by_id: HashMap<&str, &Track> = tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    // Deezer's track list names only the main artist: each song's page has all the credits.
    let mut credits: Vec<Option<String>> = vec![None; songs.len()];
    let wanted: HashSet<usize> = assign.iter().filter_map(|a| a.song).collect();
    std::thread::scope(|s| {
        for (i, slot) in credits.iter_mut().enumerate() {
            let Some(id) = songs[i].deezer_id.filter(|_| wanted.contains(&i)) else { continue };
            let client = &client;
            s.spawn(move || *slot = autotag::deezer_details(client, id).and_then(|d| d.artist));
        }
    });
    let genre = if album.genre.is_empty() { autotag::album_genre(&client, &album.album_ref).unwrap_or_default() } else { album.genre.clone() };
    let mut plan: Vec<(Track, Found)> = Vec::new();
    let mut matched = 0;
    for a in &assign {
        let Some(&t) = by_id.get(a.track_id.as_str()) else { continue };
        let song = a.song.and_then(|i| songs.get(i).map(|s| (i, s)));
        matched += song.is_some() as usize;
        let (title, artist, track_no, disc_no, subtitle) = match song {
            Some((i, s)) => {
                let other = (keep_titles && norm(&s.title) != norm(&t.title)).then(|| s.title.clone());
                let title = if keep_titles { t.title.clone() } else { s.title.clone() };
                (title, credits[i].clone().unwrap_or_else(|| s.artist.clone()), s.track_no, s.disc_no, other)
            }
            None => (t.title.clone(), t.artist.clone(), t.track_no, t.disc_no, None),
        };
        plan.push((
            t.clone(),
            Found {
                title,
                artist,
                album: album.title.clone(),
                album_artist: album.artist.clone(),
                year: album.year,
                track_no,
                disc_no,
                genre: genre.clone(),
                cover_url: album.cover_url.clone(),
                replace_cover: true,
                subtitle,
            },
        ));
    }
    if plan.is_empty() {
        return Err("No songs to save.".into());
    }
    let songs_n = plan.len();
    // The playing song is written when it ends; `commit` says so only for the first song.
    let playing = {
        let s = ctx.state.engine.status();
        s.index.and_then(|i| s.queue.get(i).cloned())
    };
    let later = plan.iter().any(|(t, _)| playing.as_deref() == Some(t.id.as_str()));
    let mut rec = crate::changes::Recorder::new(ctx, format!("Fix album info: “{}”", album.title));
    autotag::commit(ctx, &plan, album.cover_url.as_deref(), true, &mut rec)?;
    Ok(AlbumSaved { songs: songs_n, matched, later, change: rec.finish(ctx) })
}

// ---------- Fix all ----------

static FIX_ALL_RUNNING: AtomicBool = AtomicBool::new(false);
static FIX_ALL_STOP: AtomicBool = AtomicBool::new(false);

/// What Fix all found, waiting for the user to check it: the song and the info for it.
fn found() -> &'static Mutex<Vec<(Track, Found)>> {
    static FOUND: OnceLock<Mutex<Vec<(Track, Found)>>> = OnceLock::new();
    FOUND.get_or_init(Default::default)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixAllProgress {
    done: usize,
    total: usize,
    /// Songs found so far (to check before saving).
    found: usize,
    running: bool,
}

/// Info Fix all found for a song, for the user to check.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundInfo {
    track_id: String,
    /// What the song is called now (often its file name).
    now: String,
    title: String,
    artist: String,
    album: String,
    year: Option<u32>,
    cover: Option<String>,
}

/// Looks up every song without tags (one at a time, in the background) and keeps what it finds
/// for the user to check; nothing is written yet. False if it's running.
pub fn fix_all_start(ctx: Ctx) -> bool {
    if FIX_ALL_RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    FIX_ALL_STOP.store(false, Ordering::SeqCst);
    let todo: Vec<Track> = {
        let lib = ctx.state.library.lock();
        let waiting: HashSet<String> = found().lock().unwrap().iter().map(|(t, _)| t.id.clone()).collect();
        lib.snapshot.tracks.iter().filter(|t| untagged(t) && t.cue.is_none() && !waiting.contains(&t.id)).cloned().collect()
    };
    std::thread::spawn(move || {
        let total = todo.len();
        let progress = |done: usize, running: bool| {
            let found = found().lock().unwrap().len();
            ctx.emit("fix-all-progress", FixAllProgress { done, total, found, running })
        };
        let pool = crate::jobs::background_pool();
        pool.install(|| {
            autotag::paced(|| {
                for (i, t) in todo.iter().enumerate() {
                    if FIX_ALL_STOP.load(Ordering::SeqCst) {
                        break;
                    }
                    progress(i, true);
                    match autotag::lookup(t) {
                        Some(f) => found().lock().unwrap().push((t.clone(), f)),
                        None => log::info!("fix all: no catalogue match for {}", t.path),
                    }
                }
            })
        });
        FIX_ALL_RUNNING.store(false, Ordering::SeqCst);
        progress(total, false);
    });
    true
}

pub fn fix_all_stop() {
    FIX_ALL_STOP.store(true, Ordering::SeqCst);
}

/// What Fix all found, to check.
pub fn fix_all_found() -> Vec<FoundInfo> {
    found()
        .lock()
        .unwrap()
        .iter()
        .map(|(t, f)| FoundInfo {
            track_id: t.id.clone(),
            now: t.title.clone(),
            title: f.title.clone(),
            artist: f.artist.clone(),
            album: f.album.clone(),
            year: f.year,
            cover: f.cover_url.clone(),
        })
        .collect()
}

/// What Fix all saved.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixAllSaved {
    songs: usize,
    /// The change, to undo it.
    change: Option<String>,
}

/// Saves the found info of the songs the user kept (`ids`); the rest is let go.
pub fn fix_all_save(ctx: &Ctx, ids: Vec<String>) -> Result<FixAllSaved, String> {
    let picked: Vec<(Track, Found)> = {
        let mut all = found().lock().unwrap();
        let keep: HashSet<&String> = ids.iter().collect();
        let picked = all.iter().filter(|(t, _)| keep.contains(&t.id)).cloned().collect();
        all.clear();
        picked
    };
    let songs = picked.len();
    let mut rec = crate::changes::Recorder::new(ctx, format!("Fix all: {songs} {}", if songs == 1 { "song" } else { "songs" }));
    for (t, f) in &picked {
        let cover = f.cover_url.clone();
        if let Err(e) = autotag::commit(ctx, std::slice::from_ref(&(t.clone(), f.clone())), cover.as_deref(), false, &mut rec) {
            log::warn!("fix all: couldn't tag {}: {e}", t.path);
        }
    }
    if songs > 0 {
        crate::jobs::start_scan(ctx.clone());
    }
    ctx.emit("fix-all-progress", FixAllProgress { done: 0, total: 0, found: 0, running: false });
    Ok(FixAllSaved { songs, change: rec.finish(ctx) })
}

/// Lets go of what Fix all found.
pub fn fix_all_discard(ctx: &Ctx) {
    found().lock().unwrap().clear();
    ctx.emit("fix-all-progress", FixAllProgress { done: 0, total: 0, found: 0, running: false });
}

// ---------- Artists under two names ----------

/// What the catalogue says about the library's artist names, kept in artist-names.json so each
/// name is asked about once.
#[derive(Default, Serialize, Deserialize)]
struct ArtistNames {
    /// A name in Japanese or Chinese letters → the same artist's name in Latin ones (none: not found).
    #[serde(default)]
    latin: HashMap<String, Option<String>>,
    /// Pairs the user said aren't the same artist.
    #[serde(default)]
    different: Vec<[String; 2]>,
}

fn names_file(dir: &Path) -> PathBuf {
    dir.join("artist-names.json")
}
fn load_names(dir: &Path) -> ArtistNames {
    std::fs::read(names_file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn save_names(dir: &Path, names: &ArtistNames) {
    if let Ok(b) = serde_json::to_vec(names) {
        let _ = std::fs::write(names_file(dir), b);
    }
}

/// Every artist name in the library (each name of a credit like "A & B"), as written, with its songs.
fn library_artists(ctx: &Ctx) -> HashMap<String, (String, usize)> {
    let lib = ctx.state.library.lock();
    let mut out: HashMap<String, (String, usize)> = HashMap::new();
    for t in &lib.snapshot.tracks {
        if untagged(t) {
            continue;
        }
        let mut names: Vec<String> = split_credit(&t.artist);
        names.extend(split_credit(&t.album_artist));
        let mut seen = HashSet::new();
        for n in names {
            let key = norm(&n);
            if key.is_empty() || !seen.insert(key.clone()) {
                continue;
            }
            out.entry(key).or_insert_with(|| (n, 0)).1 += 1;
        }
    }
    out
}

static ARTISTS_RUNNING: AtomicBool = AtomicBool::new(false);
static ARTISTS_STOP: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistCheckProgress {
    done: usize,
    total: usize,
    running: bool,
}

/// Asks the catalogue about every artist name in Japanese or Chinese letters not asked about yet.
pub fn artist_check_start(ctx: Ctx) -> bool {
    if ARTISTS_RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    ARTISTS_STOP.store(false, Ordering::SeqCst);
    let dir = ctx.state.data_dir.clone();
    let known = load_names(&dir);
    let todo: Vec<String> = library_artists(&ctx).into_values().map(|(n, _)| n).filter(|n| cjk(n) && !known.latin.contains_key(n)).collect();
    std::thread::spawn(move || {
        let total = todo.len();
        let progress = |done: usize, running: bool| ctx.emit("artist-check-progress", ArtistCheckProgress { done, total, running });
        if let Ok(client) = client() {
            let pool = crate::jobs::background_pool();
            pool.install(|| {
                autotag::paced(|| {
                    for (i, name) in todo.iter().enumerate() {
                        if ARTISTS_STOP.load(Ordering::SeqCst) {
                            break;
                        }
                        progress(i, true);
                        let Some(answer) = latin_name(&client, name) else { continue };
                        let mut names = load_names(&dir);
                        names.latin.insert(name.clone(), answer);
                        save_names(&dir, &names);
                    }
                })
            });
        }
        ARTISTS_RUNNING.store(false, Ordering::SeqCst);
        progress(total, false);
    });
    true
}

pub fn artist_check_stop() {
    ARTISTS_STOP.store(true, Ordering::SeqCst);
}

/// The artist's name in Latin letters: the artist of their songs in the Japanese (or Taiwanese)
/// store, credited there as `name`, looked up in the US one. (Searching artists instead finds
/// "Kenshi Yonezu" and a second, empty 米津玄師 profile.) Some(None): the catalogue has nothing
/// else. None: it couldn't be asked.
fn latin_name(client: &reqwest::blocking::Client, name: &str) -> Option<Option<String>> {
    let q = urlencoding::encode(name).into_owned();
    let mut reached = false;
    for country in ["JP", "TW"] {
        autotag::pace_itunes();
        let Some(v) = get(client, &format!("https://itunes.apple.com/search?media=music&entity=song&limit=10&country={country}&term={q}")) else {
            continue;
        };
        reached = true;
        let Some(id) = v["results"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|h| h["artistName"].as_str().is_some_and(|n| norm(n) == norm(name)))
            .and_then(|h| h["artistId"].as_u64())
        else {
            continue;
        };
        autotag::pace_itunes();
        let us = get(client, &format!("https://itunes.apple.com/lookup?id={id}&country=US"))?;
        let other = us["results"][0]["artistName"].as_str().filter(|n| !cjk(n) && norm(n) != norm(name)).map(str::to_string);
        return Some(other);
    }
    reached.then_some(None)
}

/// Two names of one artist in the library.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistPair {
    /// In Japanese or Chinese letters, as written in the library.
    native: String,
    native_songs: usize,
    /// In Latin letters, as written in the library.
    latin: String,
    latin_songs: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistPairs {
    pairs: Vec<ArtistPair>,
    /// Names in Japanese or Chinese letters not asked about yet.
    unchecked: usize,
    running: bool,
}

pub fn artist_pairs(ctx: &Ctx) -> ArtistPairs {
    let names = load_names(&ctx.state.data_dir);
    let artists = library_artists(ctx);
    let different: HashSet<(String, String)> = names.different.iter().map(|[a, b]| (norm(a), norm(b))).collect();
    let mut pairs: Vec<ArtistPair> = names
        .latin
        .iter()
        .filter_map(|(native, latin)| {
            let latin = latin.as_ref()?;
            let (n, l) = (norm(native), norm(latin));
            if different.contains(&(n.clone(), l.clone())) {
                return None;
            }
            let (native, native_songs) = artists.get(&n)?.clone();
            let (latin, latin_songs) = artists.get(&l)?.clone();
            Some(ArtistPair { native, native_songs, latin, latin_songs })
        })
        .collect();
    pairs.sort_by(|a, b| a.native.cmp(&b.native));
    let unchecked = artists.values().filter(|(n, _)| cjk(n) && !names.latin.contains_key(n)).count();
    ArtistPairs { pairs, unchecked, running: ARTISTS_RUNNING.load(Ordering::SeqCst) }
}

/// The user says these two names are different artists: not suggested again.
pub fn artist_pair_dismiss(ctx: &Ctx, native: String, latin: String) {
    let dir = &ctx.state.data_dir;
    let mut names = load_names(dir);
    names.different.push([native, latin]);
    save_names(dir, &names);
}

/// `credit` with the name `from` (a whole name of it) changed to `to`; None when it doesn't have it.
fn renamed(credit: &str, from: &str, to: &str) -> Option<String> {
    if norm(credit) == norm(from) {
        return Some(to.to_string());
    }
    let part = split_credit(credit).into_iter().find(|n| norm(n) == norm(from))?;
    Some(credit.replacen(&part, to, 1))
}

/// What renaming an artist changed.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Renamed {
    songs: usize,
    /// The change, to undo it.
    change: Option<String>,
}

/// Writes `to` in place of `from` in every song's artist and album artist.
pub fn merge_artist(ctx: &Ctx, from: String, to: String) -> Result<Renamed, String> {
    let changes: Vec<(String, PathBuf, crate::tagedit::TagChanges)> = {
        let lib = ctx.state.library.lock();
        lib.snapshot
            .tracks
            .iter()
            .filter(|t| t.cue.is_none() && !untagged(t))
            .filter_map(|t| {
                let artist = renamed(&t.artist, &from, &to);
                let album_artist = renamed(&t.album_artist, &from, &to);
                (artist.is_some() || album_artist.is_some()).then(|| {
                    (t.id.clone(), PathBuf::from(&t.path), crate::tagedit::TagChanges { artist, album_artist, ..Default::default() })
                })
            })
            .collect()
    };
    let playing = {
        let s = ctx.state.engine.status();
        s.index.and_then(|i| s.queue.get(i).cloned())
    };
    let mut rec = crate::changes::Recorder::new(ctx, format!("Renamed “{from}” to “{to}”"));
    for (_, path, _) in &changes {
        rec.add(&path.to_string_lossy(), false);
    }
    let mut written = 0;
    for (id, path, c) in &changes {
        if playing.as_deref() == Some(id.as_str()) {
            let (ctx, path, c) = (ctx.clone(), path.clone(), c.clone());
            autotag::after_song(
                id,
                Box::new(move || {
                    if crate::tagedit::write(&path, &c).is_ok() {
                        crate::jobs::start_scan(ctx);
                    }
                }),
            );
        } else {
            match crate::tagedit::write(path, c) {
                Ok(()) => written += 1,
                Err(e) => log::warn!("couldn't rename the artist in {}: {e}", path.display()),
            }
        }
    }
    if written > 0 {
        crate::jobs::start_scan(ctx.clone());
    }
    Ok(Renamed { songs: changes.len(), change: rec.finish(ctx) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_whole_name_in_a_credit() {
        assert_eq!(renamed("米津玄師", "米津玄師", "Kenshi Yonezu").as_deref(), Some("Kenshi Yonezu"));
        assert_eq!(renamed("Hikaru Utada & 米津玄師", "米津玄師", "Kenshi Yonezu").as_deref(), Some("Hikaru Utada & Kenshi Yonezu"));
        // Part of a name isn't the name.
        assert_eq!(renamed("米津玄師バンド", "米津玄師", "Kenshi Yonezu"), None);
    }

    /// Needs the internet: the catalogue knows 米津玄師 is Kenshi Yonezu.
    #[test]
    #[ignore]
    fn finds_an_artists_latin_name() {
        let client = client().unwrap();
        assert_eq!(latin_name(&client, "米津玄師"), Some(Some("Kenshi Yonezu".into())));
        assert_eq!(latin_name(&client, "YOASOBI"), Some(None));
    }

    /// Needs the internet: the Wuthering Waves single, found by its artists when its name is in Chinese.
    #[test]
    #[ignore]
    fn finds_albums() {
        let client = client().unwrap();
        let found = search_albums(&client, "Wuthering Waves Dancing Through Fantasies");
        assert!(found.iter().any(|c| c.source == "Deezer" && c.title == "Dancing Through Fantasies" && c.tracks == 2));
    }
}
