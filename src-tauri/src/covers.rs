//! Change Cover: finding an album's cover in the online catalogues (iTunes and Deezer, no account
//! or key; only the search words are sent) and putting a new cover on an album.
//!
//! A new cover is shown at once (the album's artwork files are replaced), then written into the
//! songs' files, so it stays after a rescan and travels with the music. A song that is playing
//! gets its file written when it stops.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// A cover offered from the online catalogues.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cover {
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    /// Large image, written into the files.
    pub url: String,
    /// Small image, for the list.
    pub thumb: String,
    /// "iTunes" or "Deezer".
    pub source: String,
    /// Same album title and artist as ours.
    pub likely: bool,
}

/// What changed, so the interface can reload the artwork.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changed {
    pub art: String,
    pub art_small: String,
    /// Songs whose files are written when they stop playing.
    pub later: usize,
    /// Songs whose files couldn't be written (e.g. read-only).
    pub failed: usize,
}

/// What to search for, most precise first: "Artist Album", the album alone, then the main
/// artist (without "feat. ...") with the start of the title. Long compilation names often only
/// match in part.
pub fn search_terms(artist: &str, title: &str) -> Vec<String> {
    let known = !(artist == "Unknown Artist" || artist.is_empty());
    let lead = artist
        .split([',', '&'])
        .next()
        .unwrap_or(artist)
        .split(" feat")
        .next()
        .unwrap_or(artist)
        .split(" Feat")
        .next()
        .unwrap_or(artist)
        .trim();
    let mut words: Vec<&str> = title.split_whitespace().take(3).collect();
    while words.len() > 1 && ["the", "of", "a", "and", "-", "&"].contains(&words[words.len() - 1].to_lowercase().as_str()) {
        words.pop();
    }
    let short = words.join(" ");
    let mut terms = Vec::new();
    if known {
        terms.push(format!("{artist} {title}"));
    }
    terms.push(title.to_string());
    if known && !lead.is_empty() {
        terms.push(format!("{lead} {short}"));
    }
    terms.push(short);
    terms.dedup();
    terms
}

fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// "Album - Single" / "Album - EP" → "Album".
fn release(title: &str) -> &str {
    title.trim_end_matches(" - Single").trim_end_matches(" - EP")
}

fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (letters(a), letters(b));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

/// Covers for `term` from iTunes (a few stores) and Deezer, the likeliest for this album first.
pub fn search(term: &str, artist: &str, title: &str) -> Result<Vec<Cover>, String> {
    let http = crate::net::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let get = |url: String| -> Option<Value> { http.get(url).send().ok()?.json().ok() };
    let q = urlencoding::encode(term).into_owned();
    let mut out: Vec<Cover> = Vec::new();
    let mut reached = false;
    for country in ["US", "JP", "ID"] {
        let Some(v) = get(format!(
            "https://itunes.apple.com/search?media=music&entity=album&limit=12&country={country}&term={q}"
        )) else {
            continue;
        };
        reached = true;
        for h in v["results"].as_array().into_iter().flatten() {
            let Some(art) = h["artworkUrl100"].as_str() else { continue };
            out.push(Cover {
                title: h["collectionName"].as_str().unwrap_or("").to_string(),
                artist: h["artistName"].as_str().unwrap_or("").to_string(),
                year: h["releaseDate"].as_str().and_then(|d| d.get(..4)?.parse().ok()),
                url: art.replace("100x100bb", "1200x1200bb"),
                thumb: art.replace("100x100bb", "300x300bb"),
                source: "iTunes".into(),
                likely: false,
            });
        }
        if out.len() >= 8 {
            break;
        }
    }
    if let Some(v) = get(format!("https://api.deezer.com/search/album?q={q}&limit=10")) {
        reached = true;
        for h in v["data"].as_array().into_iter().flatten() {
            let Some(big) = h["cover_xl"].as_str().filter(|s| !s.is_empty() && !s.contains("/cover//")) else { continue };
            out.push(Cover {
                title: h["title"].as_str().unwrap_or("").to_string(),
                artist: h["artist"]["name"].as_str().unwrap_or("").to_string(),
                year: None,
                url: big.to_string(),
                thumb: h["cover_medium"].as_str().unwrap_or(big).to_string(),
                source: "Deezer".into(),
                likely: false,
            });
        }
    }
    if !reached {
        return Err("Couldn't reach the online catalogues. Check your internet connection.".into());
    }
    let unknown = artist == "Unknown Artist";
    // 2: same album and artist; 1: same album title (the artist may be written in another script).
    let score = |c: &Cover| -> u8 {
        let t = similar(release(&c.title), title);
        let a = unknown || similar(&c.artist, artist);
        (t as u8) * (1 + a as u8)
    };
    for c in &mut out {
        c.likely = score(c) == 2;
    }
    // One of each album (both catalogues often have it), likeliest first, iTunes before Deezer.
    let mut seen = HashSet::new();
    out.retain(|c| seen.insert(letters(&format!("{}|{}", release(&c.title), c.artist))));
    out.sort_by_key(|c| std::cmp::Reverse(score(c)));
    out.truncate(16);
    Ok(out)
}

/// Puts a new cover on an album.
pub fn set(app: &AppHandle, album_id: &str, source: crate::artists::Source) -> Result<Changed, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let bytes = crate::artists::read_source(source)?;
    let art_dir = state.art_dir();
    let (art, art_small) = crate::library::write_art(album_id, &bytes, &art_dir);
    let (Some(art), Some(art_small)) = (art, art_small) else {
        return Err("That isn't an image Cadence can read (use JPEG, PNG or WebP).".into());
    };
    // The resized JPEG is what goes into the files (a 10 MB PNG would bloat every song).
    let jpeg = std::fs::read(&art).map_err(|e| e.to_string())?;
    let songs: Vec<(String, PathBuf)> = {
        let mut lib = state.library.lock();
        let songs = lib
            .snapshot
            .tracks
            .iter()
            .filter(|t| t.album_id == album_id)
            .map(|t| (t.id.clone(), PathBuf::from(&t.path)))
            .collect();
        // Show it now: the albums are rebuilt from the new artwork (and its colours).
        let mut tracks = lib.snapshot.tracks.clone();
        let albums = crate::library::build_albums(&mut tracks, &art_dir);
        lib.set_tracks(tracks, albums);
        songs
    };
    let _ = app.emit("library-changed", ());

    let playing = {
        let s = state.engine.status();
        s.index.and_then(|i| s.queue.get(i).cloned())
    };
    let (mut later, mut failed, mut written) = (0, 0, 0);
    for (id, path) in songs {
        if playing.as_deref() == Some(id.as_str()) {
            later += 1;
            let (app, jpeg) = (app.clone(), jpeg.clone());
            crate::autotag::after_song(
                &id,
                Box::new(move || {
                    if embed(&path, &jpeg).is_ok() {
                        crate::jobs::start_scan(app);
                    }
                }),
            );
        } else if let Err(e) = embed(&path, &jpeg) {
            log::warn!("couldn't write the cover into {}: {e}", path.display());
            failed += 1;
        } else {
            written += 1;
        }
    }
    if written > 0 {
        crate::jobs::start_scan(app.clone());
    }
    Ok(Changed { art, art_small, later, failed })
}

/// Replaces the front cover in a song's tags.
fn embed(path: &Path, jpeg: &[u8]) -> Result<(), String> {
    let mut tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if tagged.primary_tag().is_none() {
        let kind = tagged.primary_tag_type();
        tagged.insert_tag(lofty::tag::Tag::new(kind));
    }
    let tag = tagged.primary_tag_mut().ok_or("no tag")?;
    // MP4 files have no picture types (every picture is "Other") and show the first one.
    if tag.tag_type() == lofty::tag::TagType::Mp4Ilst {
        while !tag.pictures().is_empty() {
            tag.remove_picture(0);
        }
    } else {
        tag.remove_picture_type(PictureType::CoverFront);
    }
    tag.push_picture(
        Picture::unchecked(jpeg.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Jpeg)
            .build(),
    );
    tag.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// Live: `cargo test --lib covers -- --ignored --nocapture`.
    #[test]
    fn shorter_searches() {
        assert_eq!(
            super::search_terms("Adaro Feat. Danny", "Hardstyle World The Best Of Outdoor Festival Megamix Vol. 1"),
            vec![
                "Adaro Feat. Danny Hardstyle World The Best Of Outdoor Festival Megamix Vol. 1",
                "Hardstyle World The Best Of Outdoor Festival Megamix Vol. 1",
                "Adaro Hardstyle World",
                "Hardstyle World",
            ]
        );
        assert_eq!(super::search_terms("Unknown Artist", "YouTube"), vec!["YouTube"]);
    }

    #[test]
    #[ignore]
    fn finds_covers() {
        for term in super::search_terms("Adaro Feat. Danny", "Hardstyle World The Best Of Outdoor Festival Megamix Vol. 1") {
            let found = super::search(&term, "Adaro Feat. Danny", "Hardstyle World The Best Of Outdoor Festival Megamix Vol. 1").unwrap();
            println!("{term}: {} result(s){}", found.len(), found.first().map(|c| format!(", first {} | {}", c.title, c.artist)).unwrap_or_default());
        }
        let found = super::search("Utada Hikaru Simple And Clean", "宇多田ヒカル", "Simple And Clean").unwrap();
        for c in &found {
            println!("{} | {} | {:?} | {} | likely {}", c.title, c.artist, c.year, c.source, c.likely);
        }
        assert!(!found.is_empty());
    }

    /// Writing a cover into a copy of a real file: `CADENCE_COVER_FILE=path cargo test --lib
    /// covers::tests::embeds -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn embeds() {
        let Ok(src) = std::env::var("CADENCE_COVER_FILE") else { return };
        let dir = std::env::temp_dir().join("cadence-cover-test");
        let _ = std::fs::create_dir_all(&dir);
        let copy = dir.join(std::path::Path::new(&src).file_name().unwrap());
        std::fs::copy(&src, &copy).unwrap();
        let mut img = image::RgbImage::new(64, 64);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .unwrap();
        super::embed(&copy, &jpeg).unwrap();
        let tagged = lofty::read_from_path(&copy).unwrap();
        use lofty::prelude::*;
        let pics = tagged.primary_tag().unwrap().pictures();
        println!("{} picture(s), first {} bytes", pics.len(), pics[0].data().len());
        assert_eq!(pics.iter().filter(|p| p.data() != &jpeg[..]).count(), 0, "old covers left");
        assert_eq!(pics[0].data(), &jpeg[..]);
    }
}
