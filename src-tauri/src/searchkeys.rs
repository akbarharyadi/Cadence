//! Extra words that find songs, albums and artists in the window's search (Search and the command
//! palette): the artist's other spellings and aliases ("yonezu" finds 米津玄師, "bangtan" finds
//! 방탄소년단), sort names, and names and titles read in Latin letters — kana as romaji with long
//! vowels either way ("tokyo" finds とうきょう), Hangul romanized, hanzi as toneless pinyin only
//! where the music is Chinese, kanji read with the Japanese dictionary only where it's Japanese.
//!
//! Worked out here once per library (or artists) change and kept; the window appends them to its
//! own folded index, so typing never waits for the app. Kanji readings need the large Japanese
//! dictionary: they're worked out once in the background, kept in search-readings.json (a cache,
//! not backed up), and the dictionary is let go afterwards.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;

use crate::ctx::Ctx;
use crate::identity::{Identity, Lang, Who, collapse, hangul_romaji, split_names};
use crate::library::{Album, Track};
use crate::romanize::{is_han, is_hangul, is_kana, kana_romaji, pinyin_plain};

/// The words, folded as the window folds what's typed (`searchIndex.ts`: NFKD, marks dropped,
/// lower case). Each value holds the words one per line.
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchKeys {
    pub version: u64,
    /// Artist credit, or artist name, as written → words.
    pub credits: HashMap<String, String>,
    /// Track id → words of its title.
    pub tracks: HashMap<String, String>,
    /// Album id → words of its title.
    pub albums: HashMap<String, String>,
    /// Kanji readings are still being worked out ("search-keys-changed" says when they're in).
    pub pending: bool,
}

impl SearchKeys {
    /// Bytes of text (keys and words), for the size the window holds.
    pub fn bytes(&self) -> usize {
        [&self.credits, &self.tracks, &self.albums].iter().flat_map(|m| m.iter()).map(|(k, v)| k.len() + v.len()).sum()
    }
}

/// What's typed is folded so (the window's `fold`).
pub fn sfold(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    use unicode_normalization::char::is_combining_mark;
    s.nfkd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Context {
    Japanese,
    Chinese,
    Other,
}

/// The words a string can also be found by: its readings in Latin letters (where the script
/// allows a safe one, or `context` says which language it is).
fn readings(s: &str, context: Context, kanji: &HashMap<String, String>, missing: &mut Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let has = |f: fn(char) -> bool| s.chars().any(f);
    if has(is_hangul) {
        let r = sfold(&hangul_romaji(s));
        out.push(r.replace(' ', ""));
        out.push(r);
    }
    let japanese = has(is_kana) || (has(is_han) && context == Context::Japanese);
    if japanese {
        let romaji = if has(is_han) {
            match kanji.get(s) {
                Some(r) => r.clone(),
                None => {
                    missing.push(s.to_string());
                    // Until the dictionary has read it: the kana parts.
                    kana_romaji(s)
                }
            }
        } else {
            kana_romaji(s)
        };
        let r = sfold(&romaji).replace('\'', "");
        let c = collapse(&r);
        out.push(c.replace(' ', ""));
        out.push(c);
        out.push(r);
    } else if has(is_han) && context == Context::Chinese
        && let Some(p) = pinyin_plain(s)
    {
        let p = sfold(&p);
        out.push(p.replace(' ', ""));
        out.push(p);
    }
    out
}

/// Adds the words not already part of `visible` (what the window searches anyway) or listed.
fn push_words(into: &mut Vec<String>, visible: &str, words: impl IntoIterator<Item = String>) {
    for w in words {
        let w = w.trim().to_string();
        if w.chars().filter(|c| c.is_alphanumeric()).count() < 2 || visible.contains(&w) || into.contains(&w) {
            continue;
        }
        into.push(w);
    }
}

/// Builds the keys. `kanji`: readings already worked out; the strings that still need one are
/// returned too.
pub fn build(tracks: &[Track], albums: &[Album], id: &Identity, kanji: &HashMap<String, String>, version: u64) -> (SearchKeys, Vec<String>) {
    let mut missing = Vec::new();
    let lead = |credit: &str| match id.credit(credit).into_iter().next() {
        Some(Who::Known(i)) => Some(i),
        _ => None,
    };
    // Which artists and albums have songs in kana (their Han-only titles are Japanese).
    let mut artist_kana: HashSet<u32> = HashSet::new();
    let mut album_kana: HashSet<&str> = HashSet::new();
    let mut lead_of: HashMap<&str, Option<u32>> = HashMap::new();
    for t in tracks {
        let kana = t.title.chars().chain(t.album.chars()).any(is_kana);
        let l = *lead_of.entry(t.artist.as_str()).or_insert_with(|| lead(&t.artist));
        if kana {
            album_kana.insert(t.album_id.as_str());
            if let Some(i) = l {
                artist_kana.insert(i);
            }
        }
    }
    let artist_context = |i: Option<u32>| -> Context {
        let Some(i) = i else { return Context::Other };
        let a = id.artist(i);
        match a.lang {
            Some(Lang::Japanese) => Context::Japanese,
            Some(Lang::Chinese) => Context::Chinese,
            Some(Lang::Korean) => Context::Other,
            None if artist_kana.contains(&i) => Context::Japanese,
            // Han-only names and no kana anywhere in their songs: Chinese.
            None if a.names.iter().any(|(n, _)| crate::identity::cjk(n) && n.chars().filter(|c| c.is_alphanumeric()).all(is_han)) => Context::Chinese,
            None => Context::Other,
        }
    };
    let context_of = |artist: Option<u32>, album_id: &str| -> Context {
        if album_kana.contains(album_id) || artist.is_some_and(|i| artist_kana.contains(&i)) {
            Context::Japanese
        } else {
            artist_context(artist)
        }
    };

    // Each artist's words once.
    let mut artist_words: HashMap<u32, Vec<String>> = HashMap::new();
    let mut words_of_artist = |i: u32, missing: &mut Vec<String>| -> Vec<String> {
        if let Some(w) = artist_words.get(&i) {
            return w.clone();
        }
        let a = id.artist(i);
        let context = artist_context(Some(i));
        let mut names: Vec<String> = a.names.iter().map(|(n, _)| n.clone()).chain(a.aliases.iter().cloned()).chain(a.hints.iter().cloned()).collect();
        if let Some(m) = &a.mb {
            names.push(m.sort_name.clone());
        }
        let mut w = Vec::new();
        for n in &names {
            push_words(&mut w, "", [sfold(n)]);
            push_words(&mut w, "", readings(n, context, kanji, missing));
        }
        artist_words.insert(i, w.clone());
        w
    };

    let mut credits: HashMap<String, String> = HashMap::new();
    let mut all_credits: Vec<String> = Vec::new();
    {
        let mut seen = HashSet::new();
        let mut add = |c: &str| {
            if !c.trim().is_empty() && seen.insert(c.to_string()) {
                all_credits.push(c.to_string());
            }
        };
        for t in tracks {
            add(&t.artist);
            add(&t.album_artist);
        }
        for a in albums {
            add(&a.artist);
        }
        for a in id.artists() {
            for (n, _) in &a.names {
                add(n);
            }
        }
        for names in crate::api::artist_index(albums.iter().map(|a| a.artist.as_str())).into_values() {
            for n in names {
                add(&n);
            }
        }
    }
    for c in &all_credits {
        let visible = sfold(c);
        let mut w = Vec::new();
        for who in id.credit(c) {
            match who {
                Who::Known(i) => push_words(&mut w, &visible, words_of_artist(i, &mut missing)),
                Who::Loose(_) => {}
            }
        }
        for part in split_names(c) {
            if id.of_name(&part).is_none() {
                push_words(&mut w, &visible, readings(&part, Context::Other, kanji, &mut missing));
            }
        }
        if !w.is_empty() {
            credits.insert(c.clone(), w.join("\n"));
        }
    }

    let mut out_tracks = HashMap::new();
    for t in tracks {
        if !crate::identity::cjk(&t.title) {
            continue;
        }
        let context = context_of(lead_of.get(t.artist.as_str()).copied().flatten(), &t.album_id);
        let mut w = Vec::new();
        push_words(&mut w, &sfold(&t.title), readings(&t.title, context, kanji, &mut missing));
        if !w.is_empty() {
            out_tracks.insert(t.id.clone(), w.join("\n"));
        }
    }
    let mut out_albums = HashMap::new();
    for a in albums {
        if !crate::identity::cjk(&a.title) {
            continue;
        }
        let context = context_of(lead(&a.artist), &a.id);
        let mut w = Vec::new();
        push_words(&mut w, &sfold(&a.title), readings(&a.title, context, kanji, &mut missing));
        if !w.is_empty() {
            out_albums.insert(a.id.clone(), w.join("\n"));
        }
    }
    missing.sort();
    missing.dedup();
    let keys = SearchKeys { version, credits, tracks: out_tracks, albums: out_albums, pending: !missing.is_empty() };
    (keys, missing)
}

// ---------- in the app ----------

/// Bumped when kanji readings come in.
static READINGS_GEN: AtomicU64 = AtomicU64::new(0);
static READING: AtomicBool = AtomicBool::new(false);
static KANJI: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static KEPT: Mutex<Option<((u64, u64), SearchKeys)>> = Mutex::new(None);

fn readings_file(dir: &Path) -> std::path::PathBuf {
    dir.join("search-readings.json")
}

fn kanji(dir: &Path) -> HashMap<String, String> {
    KANJI.lock().get_or_insert_with(|| crate::state::read_json(&readings_file(dir))).clone()
}

/// The keys for the library as it is (worked out when it or the artists changed since last time).
pub fn current(ctx: &Ctx) -> SearchKeys {
    let state = &ctx.state;
    let id = crate::identity::current();
    let stamp = (crate::identity::wanted().max(id.version()), READINGS_GEN.load(Ordering::SeqCst));
    if let Some((s, k)) = KEPT.lock().as_ref()
        && *s == stamp
    {
        return k.clone();
    }
    let started = std::time::Instant::now();
    let known = kanji(&state.data_dir);
    let (keys, missing) = {
        let lib = state.library.lock();
        build(&lib.snapshot.tracks, &lib.snapshot.albums, &id, &known, stamp.0 * 1000 + stamp.1)
    };
    log::info!("search keys: {} KB in {:?} ({} readings to work out)", keys.bytes() / 1024, started.elapsed(), missing.len());
    *KEPT.lock() = Some((stamp, keys.clone()));
    if !missing.is_empty() {
        read_kanji(ctx.clone(), missing);
    }
    keys
}

/// Works out the kanji readings in the background (one low-priority thread), keeps them, and
/// tells the window.
fn read_kanji(ctx: Ctx, texts: Vec<String>) {
    if READING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("cadence-readings".into()).spawn(move || {
        crate::jobs::background_priority();
        let started = std::time::Instant::now();
        let read = crate::romanize::readings_for_search(&texts);
        let dir = &ctx.state.data_dir;
        let all = {
            let mut k = KANJI.lock();
            let map = k.get_or_insert_with(HashMap::new);
            map.extend(texts.into_iter().zip(read));
            map.clone()
        };
        crate::state::write_json_with(&readings_file(dir), &all, false);
        log::info!("{} kanji readings for search in {:?}", all.len(), started.elapsed());
        READINGS_GEN.fetch_add(1, Ordering::SeqCst);
        READING.store(false, Ordering::SeqCst);
        ctx.emit("search-keys-changed", ());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Input, fold};
    use crate::musicbrainz::tests_fixtures;

    fn song(id: &str, title: &str, artist: &str, album: &str) -> Track {
        Track { id: id.into(), title: title.into(), artist: artist.into(), album_artist: artist.into(), album: album.into(), album_id: format!("{artist}|{album}"), ..Default::default() }
    }
    fn album(tracks: &[Track], title: &str, artist: &str) -> Album {
        Album { id: format!("{artist}|{title}"), title: title.into(), artist: artist.into(), track_ids: tracks.iter().filter(|t| t.album == title && t.album_artist == artist).map(|t| t.id.clone()).collect(), ..Default::default() }
    }
    fn finds(keys: &SearchKeys, map: &str, key: &str, q: &str) -> bool {
        let m = match map {
            "credits" => &keys.credits,
            "tracks" => &keys.tracks,
            _ => &keys.albums,
        };
        m.get(key).is_some_and(|w| w.contains(&sfold(q)))
    }

    fn library() -> (Vec<Track>, Vec<Album>, Identity) {
        let tracks = vec![
            song("1", "Lemon", "米津玄師", "Lemon"),
            song("2", "Kick Back", "Kenshi Yonezu", "Kick Back"),
            song("3", "Dynamite", "방탄소년단", "BE"),
            song("4", "とうきょう", "りりあ。", "東京の歌"),
            song("5", "東京", "周杰伦", "范特西"),
            song("6", "東京", "Someone", "Somewhere"),
            song("7", "東京", "米津玄師", "Lemon"),
        ];
        let albums = vec![
            album(&tracks, "Lemon", "米津玄師"),
            album(&tracks, "Kick Back", "Kenshi Yonezu"),
            album(&tracks, "BE", "방탄소년단"),
            album(&tracks, "東京の歌", "りりあ。"),
            album(&tracks, "范特西", "周杰伦"),
            album(&tracks, "Somewhere", "Someone"),
        ];
        let mut input = Input { names: ["米津玄師", "Kenshi Yonezu", "방탄소년단", "りりあ。", "周杰伦", "Someone"].iter().map(|n| (n.to_string(), 1)).collect(), ..Default::default() };
        input.mb.insert(fold("米津玄師"), crate::musicbrainz::pick("米津玄師", tests_fixtures::YONEZU).unwrap());
        input.mb.insert(fold("방탄소년단"), crate::musicbrainz::pick("방탄소년단", tests_fixtures::BTS).unwrap());
        let id = Identity::build(&input, 1);
        (tracks, albums, id)
    }

    #[test]
    fn artists_are_found_by_their_other_names() {
        let (tracks, albums, id) = library();
        let (keys, _) = build(&tracks, &albums, &id, &HashMap::new(), 1);
        assert!(finds(&keys, "credits", "米津玄師", "yonezu"), "{:?}", keys.credits.get("米津玄師"));
        assert!(finds(&keys, "credits", "Kenshi Yonezu", "米津"), "the other spelling");
        assert!(finds(&keys, "credits", "米津玄師", "よねづ"));
        assert!(finds(&keys, "credits", "방탄소년단", "bts"));
        assert!(finds(&keys, "credits", "방탄소년단", "bangtan"), "romanized and the search hint");
        assert!(!finds(&keys, "credits", "Someone", "yonezu"));
    }

    #[test]
    fn titles_are_found_by_their_readings_only_where_justified() {
        let (tracks, albums, id) = library();
        let (keys, missing) = build(&tracks, &albums, &id, &HashMap::new(), 1);
        // Kana: long vowels either way.
        assert!(finds(&keys, "tracks", "4", "tokyo"));
        assert!(finds(&keys, "tracks", "4", "toukyou"));
        // Kanji by a Japanese artist: read by the dictionary (here: waiting for it), never as pinyin.
        assert!(missing.contains(&"東京".to_string()));
        assert!(!finds(&keys, "tracks", "7", "dong"));
        // The same kanji by a Chinese artist: pinyin.
        assert!(finds(&keys, "tracks", "5", "dongjing"), "{:?}", keys.tracks.get("5"));
        assert!(finds(&keys, "albums", "周杰伦|范特西", "fan te xi"));
        // By someone the library says nothing about: no guess.
        assert!(!keys.tracks.contains_key("6"));
        // With the dictionary's reading in: tokyo.
        let kanji: HashMap<String, String> = missing.iter().map(|m| (m.clone(), if m == "東京" { "toukyou".into() } else { String::new() })).collect();
        let (keys, missing) = build(&tracks, &albums, &id, &kanji, 2);
        assert!(missing.is_empty() && !keys.pending);
        assert!(finds(&keys, "tracks", "7", "tokyo"));
        assert!(!finds(&keys, "tracks", "6", "tokyo"));
    }

    #[test]
    fn folded_like_the_window() {
        assert_eq!(sfold("Pokémon"), "pokemon");
        assert_eq!(sfold("ＢＴＳ"), "bts");
    }

    /// `cargo test --lib searchkeys::tests::cost -- --ignored --nocapture`: time and size for a
    /// library like the owner's (7,000 songs) and a big one (30,000), a third of the titles in
    /// Japanese, a tenth in Korean, a tenth in Chinese.
    #[test]
    #[ignore]
    fn cost() {
        for n in [7_000usize, 30_000] {
            let artists = n / 6;
            let mut tracks = Vec::with_capacity(n);
            for i in 0..n {
                let a = i % artists;
                let (artist, title) = match i % 10 {
                    0..=2 => (format!("アーティスト{a}"), format!("ひかりのうた{i}")),
                    3 => (format!("가수{a}"), format!("사랑해{i}")),
                    4 => (format!("歌手{}", char::from_u32(0x4e00 + (a % 5000) as u32).unwrap()), format!("东京爱情{i}")),
                    _ => (format!("Artist {a}"), format!("Song number {i}")),
                };
                tracks.push(song(&i.to_string(), &title, &artist, &format!("Album {}", i / 12)));
            }
            let mut by_album: HashMap<String, Vec<String>> = HashMap::new();
            for t in &tracks {
                by_album.entry(t.album_id.clone()).or_default().push(t.id.clone());
            }
            let albums: Vec<Album> = by_album
                .into_iter()
                .map(|(id, ids)| {
                    let t = tracks.iter().find(|t| t.album_id == id).unwrap();
                    Album { id, title: t.album.clone(), artist: t.artist.clone(), track_ids: ids, ..Default::default() }
                })
                .collect();
            let input = Input { names: tracks.iter().map(|t| (t.artist.clone(), 1)).collect(), ..Default::default() };
            let started = std::time::Instant::now();
            let id = Identity::build(&input, 1);
            let built = started.elapsed();
            let (keys, missing) = build(&tracks, &albums, &id, &HashMap::new(), 1);
            let json = serde_json::to_vec(&keys).unwrap().len();
            println!(
                "{n} songs: identity {built:?}, keys {:?}; {} credits, {} titles, {} albums; {} KB of text, {} KB as JSON; {} kanji to read",
                started.elapsed() - built,
                keys.credits.len(),
                keys.tracks.len(),
                keys.albums.len(),
                keys.bytes() / 1024,
                json / 1024,
                missing.len()
            );
        }
    }
}
