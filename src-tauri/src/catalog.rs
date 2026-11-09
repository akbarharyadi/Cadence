//! Looking up songs that aren't in the library (for "New music for you"): does the song exist,
//! and what does its cover (or else its artist) look like. Uses Deezer's public catalogue API,
//! which needs no account or key. Only the artist and title of the suggestion are sent.

use std::time::Duration;

use serde_json::Value;

use crate::ai::Discovery;
use crate::library::Track;

fn client() -> Option<reqwest::blocking::Client> {
    crate::net::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .inspect_err(|e| log::warn!("no HTTP client for the catalogue: {e}"))
        .ok()
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

/// A suggestion's artist, title or album: as suggested, and as the catalogue's other store has it.
fn spellings<'a>(first: &'a str, other: &'a [String]) -> impl Iterator<Item = &'a str> {
    std::iter::once(first).chain(other.iter().map(String::as_str))
}

/// The library track is by the suggestion's artist, under any of the artist's names.
fn by(d: &Discovery, t: &Track) -> bool {
    spellings(&d.artist, &d.other_names.artists).any(|a| same_artist(&t.artist, a) || same_artist(&t.album_artist, a))
}

/// Whether the library already has this song (same title, a shared artist, under any of their names).
pub fn owned(d: &Discovery, library: &[Track]) -> bool {
    let titles: Vec<String> = spellings(&d.title, &d.other_names.titles).map(core_title).filter(|t| !t.is_empty()).collect();
    !titles.is_empty()
        && library.iter().any(|t| {
            let ours = core_title(&t.title);
            titles.iter().any(|title| ours == *title || (title.len() >= 6 && ours.contains(title.as_str()))) && by(d, t)
        })
}

/// The library has the album this suggestion is on (same album title, a shared artist): the
/// listener has it, or has that release, so it isn't new to them.
pub fn album_owned(d: &Discovery, library: &[Track]) -> bool {
    let albums: Vec<String> = spellings(&d.album, &d.other_names.albums).map(core_title).filter(|a| a.len() >= 2).collect();
    !albums.is_empty() && library.iter().any(|t| albums.contains(&core_title(&t.album)) && by(d, t))
}

/// A song with this title is in the library, but by another artist. The AI sometimes credits a
/// song to the wrong artist ("Anytime Anywhere" by Aimer instead of milet); such a suggestion is
/// only kept if the catalogue confirms that artist really has a song of that name. It may also be
/// the same artist spelt another way (米津玄師, Kenshi Yonezu), which the song's other names show.
pub fn title_clash(d: &Discovery, library: &[Track]) -> bool {
    let title = core_title(&d.title);
    title.len() >= 3 && library.iter().any(|t| core_title(&t.title) == title && !by(d, t))
}

fn cjk(s: &str) -> bool {
    use crate::romanize::{is_han, is_hangul, is_kana};
    s.chars().any(|c| is_kana(c) || is_han(c) || is_hangul(c))
}

/// Written in Japanese, Chinese or Korean letters: the catalogue may also have it in Latin ones.
fn non_latin(d: &Discovery) -> bool {
    cjk(&d.artist) || cjk(&d.title)
}

/// The library has songs or artists in Japanese (Chinese, Korean) letters, so a suggestion in
/// Latin ones ("Kizuna no Kiseki") may be one of them (絆ノ奇跡) under its other name.
fn library_has_non_latin(library: &[Track]) -> bool {
    library.iter().any(|t| cjk(&t.title) || cjk(&t.artist))
}

/// Worth asking the catalogue what else the song is called: it's written in Japanese (or Chinese,
/// Korean) letters, the library has such names, or it has a song of that title by an artist
/// spelt differently.
fn wants_other_names(d: &Discovery, library: &[Track]) -> bool {
    !d.other_names.checked && (non_latin(d) || title_clash(d, library) || library_has_non_latin(library))
}

fn get(http: &reqwest::blocking::Client, url: &str) -> Option<Value> {
    http.get(url).send().ok()?.json().ok()
}

/// The song in the iTunes catalogue (Japanese releases are often only there), and the other store
/// to ask for its other names. The store for the suggestion's letters is searched first.
fn itunes_find(http: &reqwest::blocking::Client, d: &Discovery) -> Option<(Value, &'static str)> {
    let q = urlencoding::encode(&format!("{} {}", d.artist, d.title)).into_owned();
    let want = core_title(&d.title);
    let stores = if non_latin(d) { ["JP", "US"] } else { ["US", "JP"] };
    for (country, other) in [(stores[0], stores[1]), (stores[1], stores[0])] {
        let Some(v) = get(http, &format!("https://itunes.apple.com/search?media=music&entity=song&limit=15&country={country}&term={q}")) else {
            continue;
        };
        let hit = v["results"].as_array().into_iter().flatten().find(|h| {
            let title = core_title(h["trackName"].as_str().unwrap_or(""));
            same_artist(h["artistName"].as_str().unwrap_or(""), &d.artist)
                && (title == want || title.starts_with(&want) || want.starts_with(&title))
        });
        if let Some(h) = hit {
            return Some((h.clone(), other));
        }
    }
    None
}

/// Notes what the song (`hit`, from one iTunes store) is called there and in the `other` store.
fn note_other_names(http: &reqwest::blocking::Client, d: &mut Discovery, hit: &Value, other: &str) {
    d.other_names.checked = true;
    let there = hit["trackId"]
        .as_u64()
        .and_then(|id| get(http, &format!("https://itunes.apple.com/lookup?id={id}&country={other}")))
        .map(|v| v["results"][0].clone())
        .unwrap_or_default();
    for h in [hit, &there] {
        add_name(&mut d.other_names.artists, &d.artist, &h["artistName"]);
        add_name(&mut d.other_names.titles, &d.title, &h["trackName"]);
        add_name(&mut d.other_names.albums, &d.album, &h["collectionName"]);
    }
}

/// Adds a name unless it's the suggestion's own or already noted.
fn add_name(names: &mut Vec<String>, ours: &str, name: &Value) {
    let Some(name) = name.as_str().map(str::trim).filter(|n| !n.is_empty()) else {
        return;
    };
    let n = norm(name);
    if !n.is_empty() && norm(ours) != n && !names.iter().any(|x| norm(x) == n) {
        names.push(name.to_string());
    }
}

/// Its other names, for a song found on Deezer or saved before they were noted.
fn look_up_other_names(http: &reqwest::blocking::Client, d: &mut Discovery) {
    match itunes_find(http, d) {
        Some((hit, other)) => note_other_names(http, d, &hit, other),
        None => d.other_names.checked = true,
    }
}

/// Fills in the cover (or the artist's picture when the song has no cover). Returns false when
/// neither Deezer nor iTunes has this song by this artist, which means the suggestion was made up
/// or credited to the wrong artist.
fn enrich(http: &reqwest::blocking::Client, d: &mut Discovery) -> bool {
    let q = urlencoding::encode(&format!("{} {}", d.artist, d.title)).into_owned();
    if let Some(v) = get(http, &format!("https://api.deezer.com/search?q={q}&limit=8")) {
        let want = core_title(&d.title);
        let hit = v["data"].as_array().into_iter().flatten().find(|h| {
            let title = core_title(h["title"].as_str().unwrap_or(""));
            same_artist(h["artist"]["name"].as_str().unwrap_or(""), &d.artist)
                && (title == want || title.starts_with(&want) || want.starts_with(&title))
        });
        if let Some(h) = hit {
            d.image = h["album"]["cover_medium"].as_str().unwrap_or("").to_string();
            d.album = album_name(h["album"]["title"].as_str());
            if d.image.is_empty() {
                d.image = h["artist"]["picture_medium"].as_str().unwrap_or("").to_string();
                d.image_is_artist = true;
            }
            return true;
        }
    }
    // Not on Deezer: the iTunes catalogue, which also says what it's called in its other store.
    let Some((h, other)) = itunes_find(http, d) else {
        return false;
    };
    d.image = h["artworkUrl100"].as_str().unwrap_or("").replace("100x100bb", "300x300bb");
    d.album = album_name(h["collectionName"].as_str());
    note_other_names(http, d, &h, other);
    true
}

/// The catalogue's album name, or nothing for its placeholders (Deezer lists some songs on "unknown").
fn album_name(name: Option<&str>) -> String {
    let name = name.unwrap_or("").trim();
    match name.to_lowercase().as_str() {
        "unknown" | "unknown album" | "n/a" | "-" => String::new(),
        _ => name.to_string(),
    }
}

/// Drops suggestions the listener already owns (under any of the song's names, or on an album
/// they have) and ones the catalogue can't find; adds pictures. Without internet (no answers at
/// all) the suggestions are kept as they are.
pub fn check(picks: Vec<Discovery>, library: &[Track]) -> Vec<Discovery> {
    let Some(http) = client() else {
        return picks.into_iter().filter(|d| !owned(d, library)).collect();
    };
    let mut out = Vec::new();
    for mut d in picks.into_iter().filter(|d| !owned(d, library)) {
        // Checked before, unless it's a title the listener owns by another artist (see title_clash).
        if (d.checked && !title_clash(&d, library)) || enrich(&http, &mut d) {
            d.checked = true;
            if wants_other_names(&d, library) {
                look_up_other_names(&http, &mut d);
            }
            // Known now (from the catalogue): in the library under other names, or on an album there.
            if owned(&d, library) || album_owned(&d, library) {
                log::info!("dropping suggestion already in the library: {} - {} ({})", d.artist, d.title, d.album);
                continue;
            }
            out.push(d);
        } else {
            log::info!("dropping suggestion not found in the catalogue: {} - {}", d.artist, d.title);
        }
    }
    out
}

/// Saved suggestions without the ones the listener has now (got since, or had under other names).
pub fn still_new(picks: Vec<Discovery>, library: &[Track]) -> Vec<Discovery> {
    picks.into_iter().filter(|d| !owned(d, library) && !album_owned(d, library)).collect()
}

/// Notes the other names of saved suggestions that don't have them yet (saved by an older version,
/// or the library now has their title by another spelling), once each. True when any were.
pub fn add_other_names(picks: &mut [Discovery], library: &[Track]) -> bool {
    if !picks.iter().any(|d| wants_other_names(d, library)) {
        return false;
    }
    let Some(http) = client() else {
        return false;
    };
    for d in picks.iter_mut() {
        if wants_other_names(d, library) {
            look_up_other_names(&http, d);
        }
    }
    true
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

    #[test]
    fn skips_songs_from_albums_in_the_library() {
        let mut t = track("Something Else", "Aimer");
        t.album = "Walpurgis".into();
        t.album_artist = "Aimer".into();
        let lib = vec![t];
        let mut d = pick("Aimer", "Brave Shine");
        d.album = "Walpurgis (Deluxe Edition)".into();
        assert!(album_owned(&d, &lib));
        d.album = "Sun Dance".into();
        assert!(!album_owned(&d, &lib));
    }

    #[test]
    fn spots_songs_the_library_has_under_other_names() {
        let mut kitani = track("Have a nice end!", "Tatsuya Kitani");
        kitani.album = "Where Our Blue Is".into();
        let lib = vec![track("KICK BACK", "Kenshi Yonezu"), track("KICK BACK - From CrosSing", "Chiai Fujikawa"), kitani];

        let mut kick = pick("米津玄師", "KICK BACK");
        assert!(!owned(&kick, &lib));
        assert!(title_clash(&kick, &lib) && non_latin(&kick));
        kick.other_names.artists = vec!["Kenshi Yonezu".into()];
        assert!(owned(&kick, &lib));

        let mut blue = pick("キタニタツヤ", "青のすみか");
        blue.album = "青のすみか - Single".into();
        assert!(!owned(&blue, &lib) && !album_owned(&blue, &lib));
        blue.other_names.artists = vec!["Tatsuya Kitani".into()];
        blue.other_names.titles = vec!["Where Our Blue Is".into()];
        blue.other_names.albums = vec!["Where Our Blue Is - Single".into()];
        assert!(album_owned(&blue, &lib));
        assert!(still_new(vec![kick, blue, pick("Uru", "アンビバレント")], &lib).len() == 1);
    }

    #[test]
    fn notes_only_names_that_differ() {
        let mut names = vec![];
        add_name(&mut names, "米津玄師", &Value::from("Kenshi Yonezu"));
        add_name(&mut names, "米津玄師", &Value::from("kenshi yonezu"));
        add_name(&mut names, "米津玄師", &Value::from("米津玄師"));
        add_name(&mut names, "米津玄師", &Value::Null);
        assert_eq!(names, ["Kenshi Yonezu"]);
    }

    #[test]
    fn placeholder_album_names_are_dropped() {
        assert_eq!(album_name(Some(" Unknown ")), "");
        assert_eq!(album_name(None), "");
        assert_eq!(album_name(Some("From Zero")), "From Zero");
    }

    #[test]
    fn notices_a_song_credited_to_the_wrong_artist() {
        let lib = vec![track("Anytime Anywhere", "milet")];
        assert!(title_clash(&pick("Aimer", "Anytime Anywhere"), &lib));
        assert!(!title_clash(&pick("milet", "Anytime Anywhere"), &lib));
        assert!(!title_clash(&pick("Aimer", "Brave Shine"), &lib));
    }

    /// Live: `cargo test --lib catalog -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn drops_misattributed_and_keeps_real_suggestions() {
        let lib = vec![track("Anytime Anywhere", "milet")];
        let out = check(vec![pick("Aimer", "Anytime Anywhere"), pick("YOASOBI", "Idol"), pick("Linkin Park", "The Emptiness Machine")], &lib);
        let titles: Vec<_> = out.iter().map(|d| format!("{} - {}", d.artist, d.title)).collect();
        println!("{titles:?}");
        assert!(!titles.iter().any(|t| t.contains("Anytime")));
        assert!(titles.iter().any(|t| t.contains("Idol")) && titles.iter().any(|t| t.contains("Emptiness")));
    }

    /// Live: the library spells these artists and titles in Latin letters, the suggestions in Japanese.
    #[test]
    #[ignore]
    fn drops_songs_owned_under_their_other_store_names() {
        let mut kitani = track("Have a nice end!", "Tatsuya Kitani");
        kitani.album = "Where Our Blue Is".into();
        let lib = vec![track("KICK BACK", "Kenshi Yonezu"), kitani];
        let out = check(vec![pick("米津玄師", "KICK BACK"), pick("キタニタツヤ", "青のすみか"), pick("Uru", "アンビバレント")], &lib);
        let titles: Vec<_> = out.iter().map(|d| format!("{} - {}", d.artist, d.title)).collect();
        println!("{titles:?}");
        assert_eq!(titles, ["Uru - アンビバレント"]);
    }

    /// Live: the other way round, the suggestion in Latin letters and the library in Japanese.
    #[test]
    #[ignore]
    fn drops_latin_suggestions_owned_in_japanese() {
        let mut kizuna = track("絆ノ奇跡", "MAN WITH A MISSION×milet");
        kizuna.album = "絆ノ奇跡 / コイコガレ".into();
        let lib = vec![kizuna, track("一途 (ALBUM ver.)", "King Gnu")];
        let mut saved = pick("MAN WITH A MISSION × milet", "Kizuna no Kiseki");
        saved.checked = true;
        assert!(wants_other_names(&saved, &lib));
        let out = check(vec![saved, pick("King Gnu", "Ichizu"), pick("Paramore", "The News")], &lib);
        let titles: Vec<_> = out.iter().map(|d| format!("{} - {}", d.artist, d.title)).collect();
        println!("{titles:?}");
        assert_eq!(titles, ["Paramore - The News"]);
    }
}
