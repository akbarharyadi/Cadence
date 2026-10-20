//! Fixing songs with no tags, like downloads named "Artist - Title.m4a" that show up as
//! "Unknown Artist".
//!
//! When such a song starts playing, Cadence looks it up in the iTunes catalogue (Apple's public
//! search, no account or key), checks that the match really is this song (title, artist and
//! length), and writes the title, artist, album, year, track number, genre and cover into the
//! file. The file is only written once it's no longer playing, then the library is rescanned.
//! Each file is looked up once; misses aren't retried.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use serde::Deserialize;
use tauri::{AppHandle, Manager};

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
pub fn on_track_change(app: &AppHandle, previous: Option<&str>, current: Option<&str>) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if !state.settings.lock().fix_tags_online {
        return;
    }
    let ready = {
        let mut rt = runtime().lock().unwrap();
        rt.playing = current.map(str::to_string);
        previous.and_then(|p| rt.waiting.remove(p))
    };
    if let Some((path, found)) = ready {
        let app = app.clone();
        std::thread::spawn(move || apply(&app, &path, &found));
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
    let app = app.clone();
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
            apply(&app, &path, &found);
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
fn apply(app: &AppHandle, path: &Path, f: &Found) {
    let cover = f.cover_url.as_deref().and_then(|u| download(u).ok());
    match write(path, f, cover) {
        Ok(()) => {
            log::info!("tagged {}", path.display());
            crate::jobs::start_scan(app.clone());
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
        if let Some((rank, found)) = best(&s.results, artist.as_deref(), &title, t.duration) {
            if best_so_far.as_ref().is_none_or(|(r, _)| rank < *r) {
                best_so_far = Some((rank, found));
            }
            // The original recording with the exact title: no need to ask other stores.
            if best_so_far.as_ref().is_some_and(|(r, _)| r.0 == 0 && r.1 == 0) {
                break;
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

/// The result that is really this song, if any, preferring the original recording.
fn best(hits: &[Hit], artist: Option<&str>, title: &str, duration: f64) -> Option<(Rank, Found)> {
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
                    cover_url: h.artwork_url100.as_ref().map(|u| u.replace("100x100bb", "600x600bb")),
                },
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
