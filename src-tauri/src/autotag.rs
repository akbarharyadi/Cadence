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
    /// Put the cover in place of the file's own (the user picked this match); otherwise it's
    /// only added to a file without one.
    pub replace_cover: bool,
    /// The catalogue's title, kept as the subtitle when the song keeps its own (only into a file
    /// without a subtitle).
    pub subtitle: Option<String>,
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
    // (Not a song of a single-file album: its info comes from the cue sheet, and writing tags
    // would change the whole album's file.)
    t.artist == "Unknown Artist" && t.cue.is_none()
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
    if let (true, Some(cover)) = (f.replace_cover, &cover) {
        crate::covers::embed(path, cover)?;
    }
    let mut tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let Some(tag) = tagged.primary_tag_mut() else { return Ok(()) };
    let mut changed = false;
    if let Some(sub) = f.subtitle.as_deref().filter(|_| tag.get_string(ItemKey::TrackSubtitle).is_none_or(|s| s.trim().is_empty())) {
        tag.insert_text(ItemKey::TrackSubtitle, sub.to_string());
        changed = true;
    }
    if let (false, Some(cover)) = (f.replace_cover, cover) {
        if !tag.pictures().iter().any(|p| p.pic_type() == PictureType::CoverFront) {
            tag.push_picture(Picture::unchecked(cover).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build());
            changed = true;
        }
    }
    if changed {
        tag.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn download(url: &str) -> Result<Vec<u8>, String> {
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
    /// Same title (and artist, when the file name has one) and about the same length; or, for a
    /// title in another script, the same artists and length.
    pub likely: bool,
    /// The catalogue it's from: "iTunes" or "Deezer".
    #[serde(default)]
    pub source: String,
    /// Its album in that catalogue ("deezer:696220481", "itunes:US:1440833098"), for the other
    /// songs of the album and its genre.
    #[serde(default)]
    pub album_ref: Option<String>,
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
            replace_cover: true,
            subtitle: None,
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
pub(crate) fn relevance(words: &[String], name: &str, by: &str, album: &str) -> f64 {
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
pub(crate) fn search_words(s: &str) -> Vec<String> {
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
    let want = Want {
        title: title.clone(),
        artists: artist.as_deref().map(artist_names).unwrap_or_default(),
        credit: artist.as_deref().map(split_credit).unwrap_or_default(),
        duration: t.duration,
    };
    let mut seen = HashSet::new();
    let mut out: Vec<(Rank4, Candidate)> = Vec::new();
    // Scores a result against the song; None when it's neither the song nor close to the search.
    let rank = |c: &Candidate| -> Option<Rank4> {
        let off = (c.duration - t.duration).abs();
        let same_title = similar(&c.title, &title) && artist.as_deref().is_none_or(|a| similar(&c.artist, a)) && off < 75.0;
        let likely = same_title || want.same_song_by_credits(c);
        let score = relevance(&words, &c.title, &c.artist, &c.album);
        if !likely && score < MIN_RELEVANCE {
            return None;
        }
        // Not the song: the ones sharing most of the search first, then the closest length.
        Some(Rank4(!likely as u8, if likely { 0.0 } else { -(score * 10.0).round() }, want.other_version(c) as u8, off))
    };
    let mut reached = false;
    for country in ["US", "ID", "JP", "KR", "GB"] {
        let Some(hits) = itunes_candidates(&client, term, country, 15) else { continue };
        reached = true;
        for c in hits {
            if !seen.insert(norm(&format!("{}|{}|{}", c.title, c.artist, c.album))) {
                continue;
            }
            if let Some(r) = rank(&c) {
                let likely = r.0 == 0;
                out.push((r, Candidate { likely, ..c }));
            }
        }
        // Enough good matches: no need to ask other stores.
        if out.iter().filter(|(r, _)| r.0 == 0).count() >= 3 {
            break;
        }
    }
    // Deezer has releases Apple doesn't list (many game soundtracks and Asian singles).
    if let Some(found) = deezer_candidates(&client, term, 15) {
        reached = true;
        let mut fresh: Vec<(Rank4, Candidate, u64)> = Vec::new();
        for (c, id) in found {
            if is_dupe(&out, &c) || !seen.insert(format!("deezer|{id}")) {
                continue;
            }
            if let Some(r) = rank(&c) {
                fresh.push((r, c, id));
            }
        }
        // The search lists only the main artist and no date: ask for the best few in full, then
        // rank them again (all the credits can show it's the song).
        fresh.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        fresh.truncate(6);
        complete_from_deezer(&client, fresh.iter_mut().map(|(_, c, id)| (c, *id)));
        for (r, c, _) in fresh {
            let r = rank(&c).unwrap_or(r);
            let likely = r.0 == 0;
            out.push((r, Candidate { likely, ..c }));
        }
    }
    if !reached {
        return Err("Couldn't reach the iTunes or Deezer catalogue. Check your internet connection.".into());
    }
    // Nothing is clearly the song (often a title in Chinese, Japanese or Korean that the
    // catalogues list in English): look through the artists' songs for one of the same length.
    if !out.iter().any(|(r, _)| r.0 == 0) && !want.artists.is_empty() {
        for c in by_credits(&client, &want) {
            if is_dupe(&out, &c) {
                continue;
            }
            let likely = want.same_song_by_credits(&c);
            let off = (c.duration - t.duration).abs();
            out.push((Rank4(!likely as u8, -10.0, want.other_version(&c) as u8, off), Candidate { likely, ..c }));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out.into_iter().take(12).map(|(_, c)| c).collect())
}

/// What Fix Song Info looks for, to tell the song from other songs by its artists.
struct Want {
    title: String,
    /// Everyone credited in the file, letters and digits only ("jixwang", "visionsound").
    artists: Vec<String>,
    /// The same names as written, for searching ("VISION SOUND").
    credit: Vec<String>,
    duration: f64,
}

impl Want {
    /// A live, instrumental, remix… recording, unless the song's own title says so.
    fn other_version(&self, c: &Candidate) -> bool {
        other_version(&format!("{} {}", c.title, c.album), &self.title)
    }

    /// The song under another name: titles in different scripts (悠忽舞于梦中 is listed as
    /// "Dancing Through Fantasies") can't be compared, so the credits and length must agree: two
    /// artists in common within two seconds, or the only artist within one.
    fn same_song_by_credits(&self, c: &Candidate) -> bool {
        if cjk(&self.title) == cjk(&c.title) || self.other_version(c) {
            return false;
        }
        let theirs = artist_names(&c.artist);
        let shared = self.artists.iter().filter(|a| theirs.contains(a)).count();
        let off = (c.duration - self.duration).abs();
        (shared >= 2 && off < 2.0) || (shared >= 1 && self.artists.len() == 1 && off < 1.0)
    }
}

/// Written with Chinese, Japanese or Korean letters.
pub(crate) fn cjk(s: &str) -> bool {
    use crate::romanize::{is_han, is_hangul, is_kana};
    s.chars().any(|c| is_han(c) || is_hangul(c) || is_kana(c))
}

/// The names in an artist credit, letters and digits only:
/// "鸣潮先约电台/jixwang/VISION SOUND" → ["鸣潮先约电台", "jixwang", "visionsound"].
fn artist_names(credit: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in split_credit(credit) {
        let n = norm(&name);
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    }
    names
}

/// The names in an artist credit as written.
pub(crate) fn split_credit(credit: &str) -> Vec<String> {
    let mut s = format!(" {credit} ");
    for joiner in [" feat. ", " feat ", " ft. ", " featuring ", " x ", " X ", " with "] {
        s = s.replace(joiner, "/");
    }
    s.split(['/', ';', ',', '&', '、', '，', '＆'])
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

/// Already offered: same title, artist and length (iTunes adds " - Single" to album names, so
/// those can't be compared).
fn is_dupe(out: &[(Rank4, Candidate)], c: &Candidate) -> bool {
    out.iter().any(|(_, o)| norm(&o.title) == norm(&c.title) && norm(&o.artist) == norm(&c.artist) && (o.duration - c.duration).abs() < 2.0)
}

/// Songs by the file's artists about as long as it, from both catalogues. Names in Latin letters
/// are searched (the catalogues spell the others differently, 鸣潮 / 鳴潮); a credit with
/// none is searched as written.
fn by_credits(client: &reqwest::blocking::Client, want: &Want) -> Vec<Candidate> {
    let latin: Vec<&str> = want.credit.iter().filter(|a| !cjk(a)).map(String::as_str).collect();
    let term = if latin.is_empty() { want.credit.join(" ") } else { latin.join(" ") };
    let close = |c: &Candidate| (c.duration - want.duration).abs() < 3.0;
    let known = |out: &[Candidate], c: &Candidate| out.iter().any(|o| norm(&o.title) == norm(&c.title) && (o.duration - c.duration).abs() < 2.0);
    let mut out: Vec<Candidate> = Vec::new();
    for country in ["US", "JP"] {
        for c in itunes_candidates(client, &term, country, 50).unwrap_or_default() {
            if close(&c) && !known(&out, &c) {
                out.push(c);
            }
        }
    }
    let mut deezer: Vec<(Candidate, u64)> = deezer_candidates(client, &term, 50)
        .unwrap_or_default()
        .into_iter()
        .filter(|(c, _)| close(c) && !known(&out, c))
        .take(6)
        .collect();
    // The search names only the main artist ("Wuthering Waves"); the song's page has them all.
    complete_from_deezer(client, deezer.iter_mut().map(|(c, id)| (c, *id)));
    out.extend(deezer.into_iter().map(|(c, _)| c));
    // Keep only songs that share an artist with the file.
    out.retain(|c| artist_names(&c.artist).iter().any(|a| want.artists.contains(a)));
    out
}

/// Songs for `term` in one iTunes store; None when it can't be reached.
fn itunes_candidates(client: &reqwest::blocking::Client, term: &str, country: &str, limit: u32) -> Option<Vec<Candidate>> {
    let url = format!(
        "https://itunes.apple.com/search?media=music&entity=song&limit={limit}&country={country}&term={}",
        urlencoding::encode(term)
    );
    let s: Search = client.get(&url).send().ok()?.json().ok()?;
    Some(
        s.results
            .into_iter()
            .filter_map(|h| {
                let (name, by) = (h.track_name?, h.artist_name?);
                Some(Candidate {
                    title: name,
                    album_artist: h.collection_artist_name.unwrap_or_else(|| by.clone()),
                    artist: by,
                    album: h.collection_name.unwrap_or_default(),
                    year: h.release_date.as_deref().and_then(|d| d.get(..4)?.parse().ok()),
                    track_no: h.track_number,
                    disc_no: h.disc_number,
                    genre: h.primary_genre_name.unwrap_or_default(),
                    cover_url: h.artwork_url100.as_ref().map(|u| u.replace("100x100bb", "600x600bb")),
                    thumb: h.artwork_url100,
                    duration: h.track_time_millis.unwrap_or(0.0) / 1000.0,
                    likely: false,
                    source: "iTunes".into(),
                    album_ref: h.collection_id.map(|id| format!("itunes:{country}:{id}")),
                })
            })
            .collect(),
    )
}

/// Fills in Deezer results from their song pages, all at once.
fn complete_from_deezer<'a>(client: &reqwest::blocking::Client, songs: impl Iterator<Item = (&'a mut Candidate, u64)>) {
    std::thread::scope(|s| {
        for (c, id) in songs {
            s.spawn(move || {
                if let Some(more) = deezer_details(client, id) {
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
}

/// Songs for `term` in Deezer's public catalogue (no account or key), with their Deezer ids.
/// None when Deezer can't be reached.
fn deezer_candidates(client: &reqwest::blocking::Client, term: &str, limit: u32) -> Option<Vec<(Candidate, u64)>> {
    let url = format!("https://api.deezer.com/search?q={}&limit={limit}", urlencoding::encode(term));
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
                        source: "Deezer".into(),
                        album_ref: h["album"]["id"].as_u64().map(|id| format!("deezer:{id}")),
                    },
                    id,
                ))
            })
            .collect(),
    )
}

/// What Deezer's page for a song adds to its search result.
pub(crate) struct DeezerMore {
    /// Everyone credited ("Wuthering Waves, jixwang & Vision Sound"), when more than one.
    pub artist: Option<String>,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub album_id: Option<u64>,
}

/// Deezer's page for song `id`: the search lists only the main artist and no date.
pub(crate) fn deezer_details(client: &reqwest::blocking::Client, id: u64) -> Option<DeezerMore> {
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
        album_id: v["album"]["id"].as_u64(),
    })
}

/// The album's genre: Deezer's song search and pages don't give one, its album page does.
pub(crate) fn album_genre(client: &reqwest::blocking::Client, album_ref: &str) -> Option<String> {
    let id = album_ref.strip_prefix("deezer:")?;
    let v: serde_json::Value = client.get(format!("https://api.deezer.com/album/{id}")).send().ok()?.json().ok()?;
    v["genres"]["data"][0]["name"].as_str().filter(|g| !g.is_empty()).map(str::to_string)
}

/// A song in an album's track list.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSong {
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub deezer_id: Option<u64>,
}

/// The songs of the album `album_ref` names, in order.
pub(crate) fn album_songs(client: &reqwest::blocking::Client, album_ref: &str) -> Vec<AlbumSong> {
    if let Some(id) = album_ref.strip_prefix("deezer:") {
        let url = format!("https://api.deezer.com/album/{id}/tracks?limit=300");
        let Some(v) = client.get(url).send().ok().and_then(|r| r.json::<serde_json::Value>().ok()) else { return Vec::new() };
        return v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                Some(AlbumSong {
                    title: t["title"].as_str()?.to_string(),
                    artist: t["artist"]["name"].as_str().unwrap_or("").to_string(),
                    duration: t["duration"].as_f64().unwrap_or(0.0),
                    track_no: t["track_position"].as_u64().map(|n| n as u32).filter(|&n| n > 0),
                    disc_no: t["disk_number"].as_u64().map(|n| n as u32).filter(|&n| n > 0),
                    deezer_id: t["id"].as_u64(),
                })
            })
            .collect();
    }
    let Some((country, id)) = album_ref.strip_prefix("itunes:").and_then(|r| r.split_once(':')) else { return Vec::new() };
    let url = format!("https://itunes.apple.com/lookup?id={id}&entity=song&limit=300&country={country}");
    let Some(s) = client.get(url).send().ok().and_then(|r| r.json::<Search>().ok()) else { return Vec::new() };
    s.results
        .into_iter()
        .filter_map(|h| {
            Some(AlbumSong {
                title: h.track_name?,
                artist: h.artist_name.unwrap_or_default(),
                duration: h.track_time_millis.unwrap_or(0.0) / 1000.0,
                track_no: h.track_number,
                disc_no: h.disc_number,
                deezer_id: None,
            })
        })
        .collect()
}

/// Which of the album's songs (not yet `used`) is `t`: the same length (within 3 s) and the same
/// kind of recording (the instrumental for 伴奏), then the same track number, then the closest.
pub(crate) fn match_song(t: &Track, songs: &[AlbumSong], used: &HashSet<usize>) -> Option<usize> {
    let kinds = version_kinds(&t.title);
    songs
        .iter()
        .enumerate()
        .filter(|(i, s)| !used.contains(i) && (s.duration - t.duration).abs() < 3.0 && version_kinds(&s.title) == kinds)
        .min_by(|(_, a), (_, b)| {
            let key = |s: &AlbumSong| ((t.track_no.is_some() && s.track_no != t.track_no) as u8, (s.duration - t.duration).abs());
            key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// What Fix Song Info saved.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    /// The song is playing: its file is written when it ends.
    pub later: bool,
    /// Other songs of its album given the album's info.
    pub others: usize,
    /// Of those, the ones found on the album, which got their own title, artist and track number.
    pub matched: usize,
}

/// Saves the info the user picked: its title too, or the song's own (`keep_title`); and with
/// `whole_album`, the album, year, genre and cover for the other songs of its album, each matched
/// to its place on the catalogue's album where it can be. Files are written at once, except the
/// playing song's, which is written when it ends (its new info shows at once).
pub fn save_choice(ctx: &Ctx, id: &str, mut choice: Candidate, keep_title: bool, whole_album: bool) -> Result<Saved, String> {
    let state = ctx.state.clone();
    let client = crate::net::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let (track, mates) = {
        let lib = state.library.lock();
        let t = lib.by_id.get(id).map(|&i| lib.snapshot.tracks[i].clone()).ok_or("Unknown song")?;
        if t.cue.is_some() {
            return Err("Songs from a single-file album (CUE sheet) take their info from the .cue file, which Cadence doesn't edit.".into());
        }
        // Songs without tags share a made-up album (their folder): never theirs to change. And
        // only songs in the album's own folder: a copy elsewhere is another file to leave alone.
        let mates: Vec<Track> = if whole_album && !untagged(&t) {
            let home = album_folder(&t.path);
            lib.snapshot
                .tracks
                .iter()
                .filter(|m| m.album_id == t.album_id && m.id != t.id && !untagged(m) && album_folder(&m.path) == home)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        (t, mates)
    };
    if choice.genre.is_empty() {
        if let Some(g) = choice.album_ref.as_deref().and_then(|r| album_genre(&client, r)) {
            choice.genre = g;
        }
    }
    let album = |title: String, artist: String, track_no: Option<u32>, disc_no: Option<u32>| Found {
        title,
        artist,
        album: choice.album.clone(),
        album_artist: choice.album_artist.clone(),
        year: choice.year,
        track_no,
        disc_no,
        genre: choice.genre.clone(),
        cover_url: choice.cover_url.clone(),
        replace_cover: true,
        subtitle: None,
    };
    // The catalogue's name for a song that keeps its own, so search finds it by either.
    let other_name = |mine: &str, theirs: &str| (keep_title && norm(mine) != norm(theirs)).then(|| theirs.to_string());
    let mut plan: Vec<(Track, Found)> = vec![(
        track.clone(),
        Found {
            subtitle: other_name(&track.title, &choice.title),
            ..album(
                if keep_title { track.title.clone() } else { choice.title.clone() },
                choice.artist.clone(),
                choice.track_no,
                choice.disc_no,
            )
        },
    )];

    let mut matched = 0;
    if !mates.is_empty() {
        let songs = choice.album_ref.as_deref().map(|r| album_songs(&client, r)).unwrap_or_default();
        let mut used = HashSet::new();
        // The picked song's own place on the album.
        if let Some(i) = songs.iter().position(|s| norm(&s.title) == norm(&choice.title) && (s.duration - choice.duration).abs() < 2.0) {
            used.insert(i);
        }
        let places: Vec<Option<usize>> = mates
            .iter()
            .map(|m| {
                let i = match_song(m, &songs, &used);
                used.extend(i);
                i
            })
            .collect();
        // Deezer's track list names only the main artist; each song's page has all the credits.
        let mut credits: Vec<Option<String>> = vec![None; mates.len()];
        std::thread::scope(|s| {
            for (slot, place) in credits.iter_mut().zip(&places) {
                let Some(deezer_id) = place.and_then(|i| songs[i].deezer_id) else { continue };
                let client = &client;
                s.spawn(move || *slot = deezer_details(client, deezer_id).and_then(|d| d.artist));
            }
        });
        for ((m, place), credit) in mates.into_iter().zip(places).zip(credits) {
            let f = match place {
                Some(i) => {
                    matched += 1;
                    let song = &songs[i];
                    let title = if keep_title { m.title.clone() } else { song.title.clone() };
                    Found {
                        subtitle: other_name(&m.title, &song.title),
                        ..album(title, credit.unwrap_or_else(|| song.artist.clone()), song.track_no, song.disc_no)
                    }
                }
                // Not on the catalogue's album (a bonus track, another length): only the album's info.
                None => album(m.title.clone(), m.artist.clone(), m.track_no, m.disc_no),
            };
            plan.push((m, f));
        }
    }
    let others = plan.len() - 1;
    let later = commit(ctx, &plan, choice.cover_url.as_deref(), true)?;
    Ok(Saved { later, others, matched })
}

/// Writes the songs' new info (the first song's failure is an error, the others' are only logged)
/// and shows it at once. The playing song's file is written when it ends; returns whether the
/// first song is that one. All the songs are of one album, with `cover_url`'s cover.
pub(crate) fn commit(ctx: &Ctx, plan: &[(Track, Found)], cover_url: Option<&str>, rescan: bool) -> Result<bool, String> {
    let state = ctx.state.clone();
    let Some((first, _)) = plan.first() else { return Ok(false) };
    let playing = {
        let s = state.engine.status();
        s.index.and_then(|i| s.queue.get(i).cloned())
    };
    let cover = cover_url.and_then(|u| download(u).ok());
    let mut later = false;
    let mut written = 0;
    {
        let mut rt = runtime().lock().unwrap();
        let tried = rt.tried.get_or_insert_with(|| load_tried(&state.data_dir));
        let mut new = false;
        for (t, _) in plan {
            new |= tried.insert(t.path.clone());
        }
        if new {
            save_tried(&state.data_dir, tried);
        }
        for (t, f) in plan {
            if playing.as_deref() == Some(t.id.as_str()) {
                rt.playing = Some(t.id.clone());
                rt.waiting.insert(t.id.clone(), (PathBuf::from(&t.path), f.clone()));
                later |= t.id == first.id;
            } else {
                rt.waiting.remove(&t.id);
            }
        }
    }
    for (i, (t, f)) in plan.iter().enumerate() {
        if playing.as_deref() == Some(t.id.as_str()) {
            continue;
        }
        match write(Path::new(&t.path), f, cover.clone()) {
            Ok(()) => written += 1,
            // The picked song failing is an error; another of the album is only logged.
            Err(e) if i == 0 => return Err(e),
            Err(e) => log::warn!("couldn't tag {}: {e}", t.path),
        }
    }
    show_now(&state, plan, cover.as_deref());
    ctx.emit("library-changed", ());
    // A rescan would read the playing song's old tags back: it runs when that song ends.
    if rescan && written > 0 && playing.as_deref().is_none_or(|p| !plan.iter().any(|(t, _)| t.id == p)) {
        crate::jobs::start_scan(ctx.clone());
    }
    Ok(later)
}

/// The folder an album's files are in: the song's folder, or the one above a disc folder
/// ("Album/CD2/04.flac" → "Album").
fn album_folder(path: &str) -> PathBuf {
    let dir = Path::new(path).parent().unwrap_or(Path::new(""));
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    let disc = ["cd", "disc", "disk", "dvd"].iter().any(|p| {
        name.strip_prefix(p).is_some_and(|rest| rest.trim_start_matches([' ', '-', '_', '.']).starts_with(|c: char| c.is_ascii_digit()))
    });
    if disc { dir.parent().unwrap_or(dir).to_path_buf() } else { dir.to_path_buf() }
}

/// Shows the new info in the library at once (the playing song's file isn't written yet), with
/// the album's cover.
fn show_now(state: &AppState, plan: &[(Track, Found)], cover: Option<&[u8]>) {
    let Some((_, first)) = plan.first() else { return };
    let art_dir = state.art_dir();
    let album_id = crate::library::album_id_for(&first.album_artist, &first.album);
    if first.replace_cover || !art_dir.join(format!("{album_id}_s.jpg")).exists() {
        if let Some(bytes) = cover {
            crate::library::write_art(&album_id, bytes, &art_dir);
        }
    }
    let mut lib = state.library.lock();
    let mut tracks = lib.snapshot.tracks.clone();
    for (t, f) in plan {
        let Some(&i) = lib.by_id.get(&t.id) else { continue };
        let x = &mut tracks[i];
        x.title = f.title.clone();
        x.artist = f.artist.clone();
        x.album = f.album.clone();
        x.album_artist = f.album_artist.clone();
        x.year = f.year;
        x.genre = f.genre.clone();
        if let Some(sub) = f.subtitle.as_ref().filter(|_| x.subtitle.is_empty()) {
            x.subtitle = sub.clone();
        }
        x.track_no = f.track_no;
        x.disc_no = f.disc_no;
        x.album_id = album_id.clone();
    }
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
pub(crate) fn norm(s: &str) -> String {
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
    collection_id: Option<u64>,
    /// Full-size cover, for a Deezer result.
    #[serde(skip)]
    cover: Option<String>,
    #[serde(skip)]
    deezer_id: Option<u64>,
}

thread_local! {
    static PACED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with the iTunes catalogue asked at most once every 3 seconds: it allows about 20
/// searches a minute, and a batch job (Fix all, the artist check) would otherwise be refused.
pub(crate) fn paced<T>(f: impl FnOnce() -> T) -> T {
    PACED.with(|p| p.set(true));
    let r = f();
    PACED.with(|p| p.set(false));
    r
}

/// Before each iTunes request: waits its turn inside `paced`, returns at once elsewhere.
pub(crate) fn pace_itunes() {
    if !PACED.with(|p| p.get()) {
        return;
    }
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if let Some(t) = *last {
        std::thread::sleep(std::time::Duration::from_millis(3100).saturating_sub(t.elapsed()));
    }
    *last = Some(std::time::Instant::now());
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
        pace_itunes();
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
        let hits: Vec<Hit> = deezer_candidates(&client, &term, 15)
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
                collection_id: None,
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
                    if let Some(g) = more.album_id.and_then(|a| album_genre(&client, &format!("deezer:{a}"))) {
                        found.genre = g;
                    }
                }
                best_so_far = Some((rank, found));
            }
        }
    }
    best_so_far.map(|(_, f)| f)
}

/// Words that mark another recording of a song, by kind, in the languages of the library
/// (伴奏 and カラオケ are instrumentals, like "Instrumental" and "Off Vocal").
const VERSION_KINDS: [&[&str]; 9] = [
    &["instrumental", "karaoke", "off vocal", "inst.", "inst)", "伴奏", "纯音乐", "純音樂", "純音楽", "カラオケ", "インスト", "반주"],
    &["live", "现场", "現場", "ライブ", "라이브"],
    &["acoustic", "不插电", "アコースティック", "어쿠스틱"],
    &["piano", "钢琴", "鋼琴", "ピアノ", "피아노"],
    &["remix", "mix)", "edit"],
    &["cover", "翻唱", "カバー", "커버"],
    &["version"],
    &["remaster"],
    &["orchestra", "管弦"],
];

/// The kinds of other recording `s` names (one bit per entry of VERSION_KINDS).
fn version_kinds(s: &str) -> u16 {
    let s = s.to_lowercase();
    VERSION_KINDS
        .iter()
        .enumerate()
        .filter(|(_, words)| words.iter().any(|w| s.contains(w)))
        .fold(0, |bits, (i, _)| bits | 1 << i)
}

/// `label` (a result's title and album) is another recording than `wanted` names: a live,
/// instrumental, remix… version that the song's own title doesn't mention.
fn other_version(label: &str, wanted: &str) -> bool {
    version_kinds(label) & !version_kinds(wanted) != 0
}

/// Lower is better: (another version?, title not exact?, seconds off).
#[derive(PartialEq, PartialOrd)]
struct Rank(u8, u8, f64);

/// Fix Song Info order, lower first: (not the song?, minus share of the search, another version?, seconds off).
#[derive(PartialEq, PartialOrd)]
struct Rank4(u8, f64, u8, f64);

/// The result that is really this song, if any, preferring the original recording.
/// Also its Deezer id, for a Deezer result.
fn best(hits: &[Hit], artist: Option<&str>, title: &str, duration: f64) -> Option<(Rank, Found, Option<u64>)> {
    let wanted = title;
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
            let label = format!("{} {}", name, h.collection_name.as_deref().unwrap_or(""));
            Some((Rank(other_version(&label, &wanted) as u8, (!exact) as u8, off), h))
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
                    replace_cover: false,
                    subtitle: None,
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
        assert_eq!(top.source, "Deezer");
        // Its title is in English, but two of the file's artists and the length agree.
        assert!(top.likely);
    }

    /// Needs the internet: the song is found from its Chinese tags alone, with no typing.
    #[test]
    #[ignore]
    fn finds_a_song_listed_under_another_name() {
        let t = Track {
            title: "悠忽舞于梦中".into(),
            artist: "鸣潮先约电台/jixwang/VISION SOUND".into(),
            duration: 200.0,
            ..Default::default()
        };
        let found = candidates(&t, &search_term(&t)).unwrap();
        for c in &found {
            println!("{} | {} | {} | {} | {} | likely {}", c.title, c.artist, c.album, c.duration, c.source, c.likely);
        }
        assert_eq!(found[0].title, "Dancing Through Fantasies");
        assert!(found[0].likely);
        assert_eq!(found.iter().filter(|c| c.likely).count(), 1, "the instrumental isn't the song");
    }

    #[test]
    fn knows_versions_in_other_languages() {
        // 伴奏 (Chinese), カラオケ (Japanese), 반주 (Korean): all instrumentals.
        assert!(version_kinds("悠忽舞于梦中 (伴奏)") == version_kinds("Dancing Through Fantasies (Instrumental)"));
        assert!(version_kinds("夜に駆ける (カラオケ)") == version_kinds("Into the Night (Instrumental)"));
        assert!(version_kinds("봄날 (반주)") == version_kinds("Spring Day (Inst.)"));
        assert!(other_version("Dancing Through Fantasies (Instrumental)", "悠忽舞于梦中"));
        assert!(!other_version("Dancing Through Fantasies (Instrumental)", "悠忽舞于梦中 (伴奏)"));
        assert!(other_version("Lemon (ライブ)", "Lemon"));
        assert!(!other_version("Lemon", "Lemon"));
    }

    #[test]
    fn matches_album_songs_by_length_and_kind() {
        let song = |title: &str, secs: f64, n: u32| AlbumSong {
            title: title.into(),
            artist: "Wuthering Waves".into(),
            duration: secs,
            track_no: Some(n),
            disc_no: Some(1),
            deezer_id: None,
        };
        let songs = [song("Dancing Through Fantasies", 200.0, 1), song("Dancing Through Fantasies (Instrumental)", 200.0, 2)];
        let mine = |title: &str, secs: f64, n: u32| Track { title: title.into(), duration: secs, track_no: Some(n), ..Default::default() };
        let used: HashSet<usize> = [0].into();
        // Same length as both: the instrumental is the one for 伴奏.
        assert_eq!(match_song(&mine("悠忽舞于梦中 (伴奏)", 200.1, 2), &songs, &HashSet::new()), Some(1));
        assert_eq!(match_song(&mine("悠忽舞于梦中", 200.1, 1), &songs, &HashSet::new()), Some(0));
        // The original is taken already and the song isn't an instrumental: no place on the album.
        assert_eq!(match_song(&mine("悠忽舞于梦中 (Remastered)", 200.1, 3), &songs, &used), None);
        // Another length: not on the album.
        assert_eq!(match_song(&mine("Bonus", 250.0, 3), &songs, &HashSet::new()), None);
    }

    /// Needs the internet: an album's songs and genre from Deezer.
    #[test]
    #[ignore]
    fn reads_deezer_albums() {
        let client = reqwest::blocking::Client::new();
        let songs = album_songs(&client, "deezer:696220481");
        let titles: Vec<&str> = songs.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Dancing Through Fantasies", "Dancing Through Fantasies (Instrumental)"]);
        assert_eq!((songs[1].track_no, songs[1].disc_no), (Some(2), Some(1)));
        assert_eq!(album_genre(&client, "deezer:696220481").as_deref(), Some("Pop"));
        // iTunes: an album's songs from the store it was found in.
        let songs = album_songs(&client, "itunes:US:1440833098");
        assert_eq!(songs.first().map(|s| (s.title.as_str(), s.track_no)), Some(("Love Me Do", Some(1))));
    }

    #[test]
    fn album_folder_skips_disc_folders() {
        assert_eq!(album_folder(r"D:\M\Album\01.flac"), Path::new(r"D:\M\Album"));
        assert_eq!(album_folder(r"D:\M\Album\CD2\04.flac"), Path::new(r"D:\M\Album"));
        assert_eq!(album_folder(r"D:\M\Album\Disc 1\04.flac"), Path::new(r"D:\M\Album"));
        // Not a disc folder: a name that only starts like one.
        assert_eq!(album_folder(r"D:\M\Album\Cdrama OST\04.flac"), Path::new(r"D:\M\Album\Cdrama OST"));
    }

    #[test]
    fn splits_artist_credits() {
        assert_eq!(artist_names("鸣潮先约电台/jixwang/VISION SOUND"), ["鸣潮先约电台", "jixwang", "visionsound"]);
        assert_eq!(artist_names("Wuthering Waves, jixwang & Vision Sound"), ["wutheringwaves", "jixwang", "visionsound"]);
        assert_eq!(artist_names("YOASOBI feat. Ayase x ikura"), ["yoasobi", "ayase", "ikura"]);
    }

    #[test]
    fn same_song_by_credits_needs_artists_and_length() {
        let want = Want { title: "悠忽舞于梦中".into(), artists: artist_names("鸣潮先约电台/jixwang/VISION SOUND"), credit: vec![], duration: 200.0 };
        let c = |title: &str, artist: &str, secs: f64| Candidate {
            title: title.into(),
            artist: artist.into(),
            album: "Album".into(),
            album_artist: String::new(),
            year: None,
            track_no: None,
            disc_no: None,
            genre: String::new(),
            cover_url: None,
            thumb: None,
            duration: secs,
            likely: false,
            source: String::new(),
            album_ref: None,
        };
        assert!(want.same_song_by_credits(&c("Dancing Through Fantasies", "Wuthering Waves, jixwang & Vision Sound", 200.0)));
        // Only one artist in common, another length, or another recording: not enough.
        assert!(!want.same_song_by_credits(&c("Dancing Through Fantasies", "Wuthering Waves & jixwang", 200.0)));
        assert!(!want.same_song_by_credits(&c("Reveries in the Crimson Beyond", "Wuthering Waves, jixwang & Vision Sound", 205.0)));
        assert!(!want.same_song_by_credits(&c("Dancing Through Fantasies (Instrumental)", "Wuthering Waves, jixwang & Vision Sound", 200.0)));
        // Titles in the same script are compared by name instead.
        assert!(!want.same_song_by_credits(&c("梦中", "jixwang & VISION SOUND", 200.0)));
        // A single artist must match within a second.
        let one = Want { title: "レモン".into(), artists: artist_names("Kenshi Yonezu"), credit: vec![], duration: 255.0 };
        assert!(one.same_song_by_credits(&c("Lemon", "Kenshi Yonezu", 255.5)));
        assert!(!one.same_song_by_credits(&c("Lemon", "Kenshi Yonezu", 257.0)));
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

    /// A picked match replaces the file's cover and keeps the catalogue's title as the subtitle,
    /// without overwriting a subtitle the file has. On a copy of a tagged song with a cover:
    /// `CADENCE_TAGGED_FILE=path cargo test --lib autotag::tests::picked_match_replaces_cover -- --ignored`.
    #[test]
    #[ignore]
    fn picked_match_replaces_cover_and_keeps_other_title() {
        let Ok(src) = std::env::var("CADENCE_TAGGED_FILE") else { return };
        let dir = std::env::temp_dir().join("cadence-autotag-cover-test");
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join(Path::new(&src).file_name().unwrap());
        std::fs::copy(&src, &copy).unwrap();
        let before = crate::library::read_track(&copy).expect("readable");
        assert!(before.has_embedded_art, "test file should have a cover");
        // A new 8×8 red cover.
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([255, 0, 0])))
            .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .unwrap();
        let found = |subtitle: &str| Found {
            title: before.title.clone(),
            artist: "Wuthering Waves, jixwang & Vision Sound".into(),
            album: "Dancing Through Fantasies".into(),
            album_artist: "Wuthering Waves".into(),
            year: Some(2025),
            track_no: Some(1),
            disc_no: Some(1),
            genre: "Pop".into(),
            cover_url: None,
            replace_cover: true,
            subtitle: Some(subtitle.into()),
        };
        write(&copy, &found("Dancing Through Fantasies"), Some(jpeg.clone())).unwrap();
        write(&copy, &found("Something Else"), Some(jpeg.clone())).unwrap();
        let tagged = lofty::read_from_path(&copy).unwrap();
        let tag = tagged.primary_tag().unwrap();
        let fronts: Vec<_> = tag.pictures().iter().filter(|p| p.pic_type() == PictureType::CoverFront).collect();
        assert_eq!(fronts.len(), 1, "one front cover");
        assert_eq!(fronts[0].data(), jpeg.as_slice(), "the new cover replaced the old one");
        let after = crate::library::read_track(&copy).unwrap();
        assert_eq!(after.title, before.title);
        assert_eq!(after.subtitle, "Dancing Through Fantasies", "a subtitle already there isn't overwritten");
        assert!((after.duration - before.duration).abs() < 0.05);
        let _ = std::fs::remove_dir_all(&dir);
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
        assert_eq!(f.genre, "Pop");
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
            collection_id: None,
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
