//! Looking up songs that aren't in the library (for "New music for you"): does the song exist,
//! and what does its cover (or else its artist) look like. Uses Deezer's public catalogue API,
//! which needs no account or key. Only the artist and title of the suggestion are sent.

use std::time::Duration;

use serde_json::Value;

use crate::ai::Discovery;
use crate::library::Track;

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

/// Lower-case letters and digits only, so "Centuries!" and "centuries" compare equal.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A title without "(feat. …)", "[Remix]", " - Single" and the like.
fn core_title(s: &str) -> String {
    let cut = s.find(['(', '[', '（', '【']).unwrap_or(s.len());
    let t = &s[..cut];
    norm(t.split(" - ").next().unwrap_or(t))
}

fn artist_names(credit: &str) -> Vec<String> {
    credit
        .split([',', '&', '/', ';', '、'])
        .flat_map(|p| p.split(" feat"))
        .map(norm)
        .filter(|n| n.len() >= 2)
        .collect()
}

fn same_artist(a: &str, b: &str) -> bool {
    let (a, b) = (artist_names(a), artist_names(b));
    a.iter().any(|x| b.iter().any(|y| x == y || x.contains(y.as_str()) || y.contains(x.as_str())))
}

/// Whether the library already has this song (same title, a shared artist).
pub fn owned(d: &Discovery, library: &[Track]) -> bool {
    let title = core_title(&d.title);
    !title.is_empty()
        && library.iter().any(|t| {
            let ours = core_title(&t.title);
            (ours == title || (title.len() >= 6 && ours.contains(&title)))
                && (same_artist(&t.artist, &d.artist) || same_artist(&t.album_artist, &d.artist))
        })
}

/// Fills in the cover (or the artist's picture). Returns false when the catalogue knows neither
/// the song nor the artist, which usually means the suggestion was made up.
fn enrich(http: &reqwest::blocking::Client, d: &mut Discovery) -> bool {
    let get = |url: String| -> Option<Value> { http.get(url).send().ok()?.json().ok() };
    let q = urlencoding::encode(&format!("{} {}", d.artist, d.title)).into_owned();
    if let Some(v) = get(format!("https://api.deezer.com/search?q={q}&limit=8")) {
        let want = core_title(&d.title);
        let hit = v["data"].as_array().into_iter().flatten().find(|h| {
            let title = core_title(h["title"].as_str().unwrap_or(""));
            same_artist(h["artist"]["name"].as_str().unwrap_or(""), &d.artist)
                && (title == want || title.starts_with(&want) || want.starts_with(&title))
        });
        if let Some(h) = hit {
            d.image = h["album"]["cover_medium"].as_str().unwrap_or("").to_string();
            d.album = h["album"]["title"].as_str().unwrap_or("").to_string();
            if d.image.is_empty() {
                d.image = h["artist"]["picture_medium"].as_str().unwrap_or("").to_string();
                d.image_is_artist = true;
            }
            return true;
        }
    }
    // The song isn't listed: show the artist instead, if they exist.
    let a = urlencoding::encode(&d.artist).into_owned();
    if let Some(v) = get(format!("https://api.deezer.com/search/artist?q={a}&limit=3")) {
        let hit = v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|h| same_artist(h["name"].as_str().unwrap_or(""), &d.artist));
        if let Some(h) = hit {
            d.image = h["picture_medium"].as_str().unwrap_or("").to_string();
            d.image_is_artist = true;
            return true;
        }
    }
    false
}

/// Drops suggestions the listener already owns and ones the catalogue can't find; adds pictures.
/// Without internet (no answers at all) the suggestions are kept as they are.
pub fn check(picks: Vec<Discovery>, library: &[Track]) -> Vec<Discovery> {
    let http = client();
    let mut out = Vec::new();
    for mut d in picks.into_iter().filter(|d| !owned(d, library)) {
        if d.checked || enrich(&http, &mut d) {
            d.checked = true;
            out.push(d);
        } else {
            log::info!("dropping suggestion not found in the catalogue: {} - {}", d.artist, d.title);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str) -> Track {
        Track {
            title: title.into(),
            artist: artist.into(),
            ..Default::default()
        }
    }

    fn pick(artist: &str, title: &str) -> Discovery {
        Discovery {
            artist: artist.into(),
            title: title.into(),
            why: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn spots_songs_already_in_the_library() {
        let lib = vec![track("Centuries", "Fall Out Boy"), track("Brave Shine (TV Size)", "Aimer")];
        assert!(owned(&pick("Fall Out Boy", "Centuries"), &lib));
        assert!(owned(&pick("Aimer", "Brave Shine"), &lib));
        assert!(!owned(&pick("Fall Out Boy", "Immortals"), &lib));
        assert!(!owned(&pick("Someone Else", "Centuries"), &lib));
    }
}
