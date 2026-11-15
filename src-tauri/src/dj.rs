//! "Ask the DJ": the model plans, Cadence retrieves, the model only orders and names.
//!
//! 1. **Plan** (one small request, no library listing): the request, the date and time, and the
//!    words the retriever understands (the library's languages, genres and decades with counts,
//!    the sound words) go to the model, which answers a strict JSON plan (`Plan`, `plan_schema`).
//!    An answer that doesn't fit the schema is asked for once more; without the AI (off, failed,
//!    unreadable) a small phrase parser (`parse_local`) makes the plan instead.
//! 2. **Retrieve** (here, in a few milliseconds): hard filters for what the plan states firmly
//!    (language, years, artists left out or asked for only, instrumental or not, never or long
//!    unplayed), soft scores for the rest (genre, mood and energy from the sound analysis, sound
//!    near the artists named, the listener's taste, novelty, skips), then a pick spread over
//!    artists and albums. Too few songs: the weakest constraint is relaxed once and the answer
//!    says so; a language the library has too few songs in is said, not padded with others.
//! 3. **Order** (optional, one short request): the selected candidates as compact lines
//!    (number, title, artist, year, a few words); the model returns numbers, a title and a
//!    sentence. A number that wasn't offered is dropped (the model can't add a song), a short
//!    answer is filled from the scores, and the same-artist spacing of the shuffle is applied.
//!    Without it the local order is used.
//!
//! Per-song facts (`Facts`) are worked out once per library revision and kept in memory.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::identity::{Identity, fold};
use crate::marks::Mark;
use crate::songlang::{Guess, Lang};
use crate::sonic::Traits;

/// A song counts as being in a language from this sureness on.
pub const LANG_MIN: f32 = 0.35;
/// Fewer candidates than this after the hard filters: the weakest constraint is relaxed.
pub const MIN_POOL: usize = 25;
/// Songs offered to the model for ordering, at most.
pub const MAX_OFFERED: usize = 150;
const DAY: u64 = 86_400;

// ---------- facts ----------

/// One song's facts, the same until the library changes.
#[derive(Clone, Debug, Default)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub lang: Lang,
    pub lang_conf: f32,
    pub year: Option<u16>,
    /// Indexes into `Facts::genres` (a tag like "Pop; Rock" is two).
    pub genres: Vec<u16>,
    pub secs: f32,
    pub traits: Option<Traits>,
    pub instrumental: bool,
    /// The lead artist (`identity` key), as an index into `Facts::artists`.
    pub artist_key: u32,
    /// Everyone in the credit ("A feat. B": A and B).
    pub credit: Vec<u32>,
    pub album: u32,
    pub added: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Genre {
    /// As most songs write it.
    pub name: String,
    /// Folded (`fold`): "J-Pop" and "jpop" are one.
    pub key: String,
    pub songs: u32,
}

#[derive(Default, Debug)]
pub struct Facts {
    pub songs: Vec<Song>,
    pub genres: Vec<Genre>,
    /// Artist keys (`identity::primary_key`).
    pub artists: Vec<String>,
    pub artist_index: HashMap<String, u32>,
    /// The songs' languages with counts, most first (sure enough only).
    pub languages: Vec<(Lang, u32)>,
    pub decades: Vec<(u16, u32)>,
    /// Songs with sound analysis.
    pub analysed: u32,
}

/// One track as the facts are built from it.
pub struct TrackIn<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub album_id: &'a str,
    pub genre: &'a str,
    pub year: Option<u32>,
    pub secs: f64,
    pub added: u64,
}

/// The genres of a tag: "Pop; Rock", "Pop/Rock" and "Pop, Rock" are two each.
fn genre_parts(g: &str) -> impl Iterator<Item = &str> {
    g.split([';', '/', ',', '|']).map(str::trim).filter(|s| !s.is_empty())
}

/// Builds the facts. `traits`: sound analysis by id; `lyrics`: lyrics languages by id.
pub fn build_facts(tracks: &[TrackIn], traits: &HashMap<String, Traits>, lyrics: &HashMap<String, Guess>, id: &Identity) -> Facts {
    let mut f = Facts::default();
    let intern = |f: &mut Facts, key: String| -> u32 {
        if let Some(&i) = f.artist_index.get(&key) {
            return i;
        }
        let i = f.artists.len() as u32;
        f.artists.push(key.clone());
        f.artist_index.insert(key, i);
        i
    };
    // Genres by folded name, the most used spelling shown.
    let mut genre_of: HashMap<String, u16> = HashMap::new();
    let mut spellings: Vec<HashMap<String, u32>> = Vec::new();
    let mut albums: HashMap<&str, u32> = HashMap::new();
    // Per credit, worked out once (many songs share one).
    let mut credit_cache: HashMap<&str, (String, Vec<String>, crate::songlang::ArtistHint)> = HashMap::new();
    let mut songs_in: Vec<crate::songlang::Song> = Vec::with_capacity(tracks.len());
    for t in tracks {
        let (lead, all, hint) = credit_cache
            .entry(t.artist)
            .or_insert_with(|| {
                let lead = id.primary_key(t.artist);
                let mut all: Vec<String> = Vec::new();
                for w in id.credit(t.artist) {
                    let k = match w {
                        crate::identity::Who::Known(i) => id.artist(i).key.clone(),
                        crate::identity::Who::Loose(s) => s,
                    };
                    if !all.contains(&k) {
                        all.push(k);
                    }
                }
                let hint = match id.credit(t.artist).into_iter().next() {
                    Some(crate::identity::Who::Known(i)) => {
                        let a = id.artist(i);
                        crate::songlang::ArtistHint {
                            lang: a.lang.map(|l| match l {
                                crate::identity::Lang::Japanese => Lang::Ja,
                                crate::identity::Lang::Korean => Lang::Ko,
                                crate::identity::Lang::Chinese => Lang::Zh,
                            }),
                            country: a.mb.as_ref().and_then(|m| m.country.as_deref()).and_then(crate::songlang::lang_of_country),
                        }
                    }
                    _ => Default::default(),
                };
                (lead, all, hint)
            })
            .clone();
        let artist_key = intern(&mut f, lead);
        let credit = all.into_iter().map(|k| intern(&mut f, k)).collect();
        let mut genres = Vec::new();
        for part in genre_parts(t.genre) {
            let key = fold(part);
            if key.is_empty() {
                continue;
            }
            let g = *genre_of.entry(key.clone()).or_insert_with(|| {
                f.genres.push(Genre { name: part.to_string(), key, songs: 0 });
                spellings.push(HashMap::new());
                (f.genres.len() - 1) as u16
            });
            f.genres[g as usize].songs += 1;
            *spellings[g as usize].entry(part.to_string()).or_default() += 1;
            if !genres.contains(&g) {
                genres.push(g);
            }
        }
        let next = albums.len() as u32;
        let album = *albums.entry(t.album_id).or_insert(next);
        let tr = traits.get(t.id).copied();
        if tr.is_some() {
            f.analysed += 1;
        }
        f.songs.push(Song {
            id: t.id.to_string(),
            title: t.title.to_string(),
            artist: t.artist.to_string(),
            lang: Lang::Unknown,
            lang_conf: 0.0,
            year: t.year.filter(|y| (1900..=2100).contains(y)).map(|y| y as u16),
            genres,
            secs: t.secs as f32,
            traits: tr,
            instrumental: crate::state::is_instrumental(t.title),
            artist_key,
            credit,
            album,
            added: t.added,
        });
        songs_in.push(crate::songlang::Song { title: t.title, artist: t.artist, album: t.album, album_id: t.album_id, artist_key: "", hint, lyrics: lyrics.get(t.id).copied() });
    }
    for (g, sp) in f.genres.iter_mut().zip(spellings) {
        if let Some((name, _)) = sp.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))) {
            g.name = name;
        }
    }
    // Languages (the artist's majority needs the artist's key).
    let keys: Vec<&str> = f.songs.iter().map(|s| f.artists[s.artist_key as usize].as_str()).collect();
    for (s, k) in songs_in.iter_mut().zip(&keys) {
        s.artist_key = k;
    }
    let guesses = crate::songlang::tag(&songs_in);
    drop(songs_in);
    let mut langs: HashMap<Lang, u32> = HashMap::new();
    let mut decades: HashMap<u16, u32> = HashMap::new();
    for (s, g) in f.songs.iter_mut().zip(guesses) {
        s.lang = g.lang;
        s.lang_conf = g.conf;
        if g.lang != Lang::Unknown && g.conf >= LANG_MIN {
            *langs.entry(g.lang).or_default() += 1;
        }
        if let Some(y) = s.year {
            *decades.entry(y / 10 * 10).or_default() += 1;
        }
    }
    f.languages = langs.into_iter().collect();
    f.languages.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    f.decades = decades.into_iter().collect();
    f.decades.sort();
    f
}

impl Facts {
    /// The genres most songs have, most first.
    pub fn top_genres(&self, n: usize) -> Vec<&Genre> {
        let mut g: Vec<&Genre> = self.genres.iter().collect();
        g.sort_by(|a, b| b.songs.cmp(&a.songs).then_with(|| a.name.cmp(&b.name)));
        g.truncate(n);
        g
    }

    /// Whether a song is (surely enough) in a language.
    pub fn sung_in(&self, s: &Song, langs: &[Lang]) -> bool {
        langs.contains(&s.lang) && s.lang_conf >= LANG_MIN
    }
}

// ---------- the facts of the app's library, kept ----------

type Key = (std::path::PathBuf, u64, u64, usize, usize, u64);
static KEPT: Mutex<Option<(Key, Arc<Facts>)>> = Mutex::new(None);
/// One build at a time (the Home page's chips, a question and Your Stats may ask together).
static BUILDING: Mutex<()> = Mutex::new(());

fn key_now(state: &crate::state::AppState) -> Key {
    let id = crate::identity::current();
    let tracks = state.library.lock().snapshot.tracks.len();
    let sonic = state.sonic.lock().tracks.len();
    let lyrics = std::fs::metadata(state.data_dir.join("lyrics"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (state.data_dir.clone(), crate::identity::wanted(), id.version(), tracks, sonic, lyrics)
}

/// The library's facts, worked out again only when the library, the artists, the sound analysis
/// or the saved lyrics changed. Don't call it holding the library's or the sound's lock.
pub fn facts(state: &crate::state::AppState) -> Arc<Facts> {
    let key = key_now(state);
    if let Some((k, f)) = KEPT.lock().as_ref()
        && *k == key
    {
        return f.clone();
    }
    let _one = BUILDING.lock();
    if let Some((k, f)) = KEPT.lock().as_ref()
        && *k == key
    {
        return f.clone();
    }
    let started = std::time::Instant::now();
    let id = crate::identity::current();
    let (tracks, traits) = {
        let lib = state.library.lock();
        let sonic = state.sonic.lock();
        let items: Vec<_> = lib.snapshot.tracks.iter().filter_map(|t| sonic.features(t).map(|f| crate::sonic::Item { track: t, f })).collect();
        (lib.snapshot.tracks.clone(), crate::sonic::traits(&items))
    };
    let japanese: HashMap<&str, bool> = tracks
        .iter()
        .map(|t| (t.artist.as_str(), matches!(id.credit(&t.artist).first(), Some(crate::identity::Who::Known(i)) if id.artist(*i).lang == Some(crate::identity::Lang::Japanese))))
        .collect();
    let wanted: Vec<(&str, &str, bool)> = tracks.iter().map(|t| (t.id.as_str(), t.title.as_str(), japanese.get(t.artist.as_str()).copied().unwrap_or(false))).collect();
    let lyrics = crate::songlang::lyric_languages(&state.data_dir, &wanted);
    let input: Vec<TrackIn> = tracks
        .iter()
        .map(|t| TrackIn { id: &t.id, title: &t.title, artist: &t.artist, album: &t.album, album_id: &t.album_id, genre: &t.genre, year: t.year, secs: t.duration, added: t.added })
        .collect();
    let f = Arc::new(build_facts(&input, &traits, &lyrics, &id));
    log::info!(
        "DJ facts: {} songs ({} with lyrics read, {} analysed), languages {:?}, in {:?}",
        f.songs.len(),
        lyrics.len(),
        f.analysed,
        f.languages.iter().map(|(l, n)| format!("{} {n}", l.code())).collect::<Vec<_>>(),
        started.elapsed()
    );
    *KEPT.lock() = Some((key, f.clone()));
    f
}

// ---------- the plan ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Any,
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Familiarity {
    #[default]
    Any,
    Favourites,
    Mixed,
    Forgotten,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vocals {
    #[default]
    Any,
    Instrumental,
    Vocal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    Any,
    Steady,
    BuildUp,
    WindDown,
}

/// What the request asks for, in the retriever's terms.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub title: String,
    pub languages: Vec<Lang>,
    /// Decades ("2010" for the 2010s).
    pub decades: Vec<u16>,
    pub year_from: Option<u16>,
    pub year_to: Option<u16>,
    pub genres: Vec<String>,
    pub moods: Vec<String>,
    pub energy: Level,
    pub tempo: Level,
    /// Artists as the listener wrote them.
    pub artists: Vec<String>,
    /// Only songs by `artists` (not "like" them).
    pub only_artists: bool,
    pub exclude_artists: Vec<String>,
    pub familiarity: Familiarity,
    /// "Not played in N days" (forgotten).
    pub not_played_days: Option<u16>,
    pub vocals: Vocals,
    pub count: Option<u16>,
    pub minutes: Option<u16>,
    pub order: Order,
}

pub const MOODS: [&str; 7] = ["sad", "happy", "calm", "energetic", "dark", "bright", "romantic"];
const LEVELS: [&str; 4] = ["any", "low", "medium", "high"];
const FAMILIARITY: [&str; 5] = ["any", "favourites", "mixed", "forgotten", "new"];
const VOCALS: [&str; 3] = ["any", "instrumental", "vocal"];
const ORDERS: [&str; 4] = ["any", "steady", "build_up", "wind_down"];

/// The plan's JSON schema (structured output where the service supports it).
pub fn plan_schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let int_or_null = json!({ "anyOf": [{ "type": "integer" }, { "type": "null" }] });
    let codes: Vec<&str> = Lang::KNOWN.iter().map(|l| l.code()).collect();
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "languages": { "type": "array", "items": { "type": "string", "enum": codes } },
            "decades": { "type": "array", "items": { "type": "integer" } },
            "year_from": int_or_null,
            "year_to": int_or_null,
            "genres": strings,
            "moods": { "type": "array", "items": { "type": "string", "enum": MOODS } },
            "energy": { "type": "string", "enum": LEVELS },
            "tempo": { "type": "string", "enum": LEVELS },
            "artists": strings,
            "only_artists": { "type": "boolean" },
            "exclude_artists": strings,
            "familiarity": { "type": "string", "enum": FAMILIARITY },
            "not_played_days": int_or_null,
            "vocals": { "type": "string", "enum": VOCALS },
            "count": int_or_null,
            "minutes": int_or_null,
            "order": { "type": "string", "enum": ORDERS },
        },
        "required": ["title", "languages", "decades", "year_from", "year_to", "genres", "moods", "energy", "tempo", "artists", "only_artists", "exclude_artists", "familiarity", "not_played_days", "vocals", "count", "minutes", "order"],
        "additionalProperties": false,
    })
}

/// Reads a plan answer. Missing fields are "not said"; a field of the wrong kind or a value outside
/// the vocabulary is an error (asked again once).
pub fn parse_plan(v: &Value) -> Result<Plan, String> {
    let o = v.as_object().ok_or("the answer is not a JSON object")?;
    let mut p = Plan::default();
    let strings = |k: &str| -> Result<Vec<String>, String> {
        match o.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or(format!("{k} must be a list of strings")))
                .filter(|s| s.as_ref().map_or(true, |s| !s.is_empty()))
                .collect(),
            Some(_) => Err(format!("{k} must be a list")),
        }
    };
    let int = |k: &str| -> Result<Option<i64>, String> {
        match o.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(x) => x.as_i64().or_else(|| x.as_f64().map(|f| f.round() as i64)).map(Some).ok_or(format!("{k} must be a whole number or null")),
        }
    };
    let choice = |k: &str, allowed: &[&str]| -> Result<String, String> {
        match o.get(k) {
            None | Some(Value::Null) => Ok("any".into()),
            Some(Value::String(s)) => {
                let s = s.trim().to_lowercase().replace([' ', '-'], "_");
                let s = if s.is_empty() { "any".to_string() } else { s };
                if allowed.contains(&s.as_str()) { Ok(s) } else { Err(format!("{k} must be one of {}", allowed.join(", "))) }
            }
            Some(_) => Err(format!("{k} must be a string")),
        }
    };
    p.title = o.get("title").and_then(Value::as_str).unwrap_or("").trim().chars().take(60).collect();
    for l in strings("languages")? {
        let lang = Lang::parse(&l).ok_or(format!("unknown language “{l}”"))?;
        if !p.languages.contains(&lang) {
            p.languages.push(lang);
        }
    }
    match o.get("decades") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for d in a {
                let d = d.as_i64().ok_or("decades must be whole numbers")?;
                // "90" for the 1990s, "10" for the 2010s.
                let d = if (0..100).contains(&d) { if d <= 30 { 2000 + d } else { 1900 + d } } else { d };
                if !(1900..=2090).contains(&d) {
                    return Err(format!("{d} is not a decade"));
                }
                let d = (d / 10 * 10) as u16;
                if !p.decades.contains(&d) {
                    p.decades.push(d);
                }
            }
        }
        Some(_) => return Err("decades must be a list".into()),
    }
    let year = |y: Option<i64>| y.filter(|y| (1900..=2100).contains(y)).map(|y| y as u16);
    p.year_from = year(int("year_from")?);
    p.year_to = year(int("year_to")?);
    p.genres = strings("genres")?;
    for m in strings("moods")? {
        let m = m.to_lowercase();
        if !MOODS.contains(&m.as_str()) {
            return Err(format!("mood “{m}” is not one of {}", MOODS.join(", ")));
        }
        if !p.moods.contains(&m) {
            p.moods.push(m);
        }
    }
    let level = |s: String| match s.as_str() {
        "low" => Level::Low,
        "medium" => Level::Medium,
        "high" => Level::High,
        _ => Level::Any,
    };
    p.energy = level(choice("energy", &LEVELS)?);
    p.tempo = level(choice("tempo", &LEVELS)?);
    p.artists = strings("artists")?;
    p.only_artists = match o.get("only_artists") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("only_artists must be true or false".into()),
    } && !p.artists.is_empty();
    p.exclude_artists = strings("exclude_artists")?;
    p.familiarity = match choice("familiarity", &FAMILIARITY)?.as_str() {
        "favourites" => Familiarity::Favourites,
        "mixed" => Familiarity::Mixed,
        "forgotten" => Familiarity::Forgotten,
        "new" => Familiarity::New,
        _ => Familiarity::Any,
    };
    p.not_played_days = int("not_played_days")?.filter(|d| *d > 0).map(|d| d.min(3650) as u16);
    p.vocals = match choice("vocals", &VOCALS)?.as_str() {
        "instrumental" => Vocals::Instrumental,
        "vocal" => Vocals::Vocal,
        _ => Vocals::Any,
    };
    p.count = int("count")?.filter(|n| *n > 0).map(|n| n.clamp(5, 100) as u16);
    p.minutes = int("minutes")?.filter(|n| *n > 0).map(|n| n.clamp(10, 600) as u16);
    p.order = match choice("order", &ORDERS)?.as_str() {
        "steady" => Order::Steady,
        "build_up" => Order::BuildUp,
        "wind_down" => Order::WindDown,
        _ => Order::Any,
    };
    Ok(p)
}

// ---------- the local phrase parser (no AI) ----------

fn has_word(hay: &str, w: &str) -> bool {
    hay.match_indices(w).any(|(i, _)| {
        let before = hay[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = hay[i + w.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

/// A plan from the request's own words: language names, decades ("90s", "2010s"), genres the
/// library has, a few moods, "haven't heard", "instrumental", a number of songs or minutes.
pub fn parse_local(request: &str, facts: &Facts) -> Plan {
    let r = request.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| has_word(&r, w));
    let mut p = Plan { title: title_case(request), ..Default::default() };
    let langs: [(&[&str], Lang); 6] = [
        (&["japanese", "jpop", "j-pop", "j-rock", "jrock", "anime", "jepang"], Lang::Ja),
        (&["korean", "kpop", "k-pop", "korea"], Lang::Ko),
        (&["chinese", "mandarin", "cantonese", "c-pop", "cpop", "mandopop", "cantopop"], Lang::Zh),
        (&["indonesian", "indonesia", "indo", "bahasa"], Lang::Id),
        (&["english"], Lang::En),
        (&["malay", "malaysian", "melayu"], Lang::Ms),
    ];
    for (words, l) in langs {
        if any(words) {
            p.languages.push(l);
        }
    }
    // Decades: "1990s", "90s", "'90s", "90an"; a year alone: that year.
    let tokens: Vec<&str> = r.split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|t| !t.is_empty()).collect();
    for t in &tokens {
        let t = t.trim_start_matches('\'');
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = &t[digits.len()..];
        if rest == "s" || rest == "an" || rest == "'s" {
            if let Ok(n) = digits.parse::<u16>() {
                let d = match digits.len() {
                    2 => Some(if n <= 30 { 2000 + n } else { 1900 + n }),
                    4 if (1900..=2090).contains(&n) => Some(n),
                    _ => None,
                };
                if let Some(d) = d.filter(|d| d % 10 == 0) && !p.decades.contains(&d) {
                    p.decades.push(d);
                }
            }
        } else if rest.is_empty() && digits.len() == 4 && let Ok(y) = digits.parse::<u16>() && (1950..=2100).contains(&y) {
            p.year_from = Some(p.year_from.map_or(y, |f| f.min(y)));
            p.year_to = Some(p.year_to.map_or(y, |f| f.max(y)));
        }
    }
    // Genres the library has, written as words of the request ("jazz", "city pop").
    let folded_words: Vec<String> = tokens.iter().map(|t| fold(t)).collect();
    let mut by_length: Vec<&Genre> = facts.genres.iter().collect();
    by_length.sort_by(|a, b| b.key.len().cmp(&a.key.len()).then_with(|| b.songs.cmp(&a.songs)));
    let mut matched_keys: Vec<String> = Vec::new();
    for g in by_length {
        // "J-Pop" asked: not "Pop" as well.
        if g.key.chars().count() < 3 || g.songs < 3 || matched_keys.iter().any(|k| k.contains(&g.key)) {
            continue;
        }
        let words: Vec<String> = genre_parts(&g.name).flat_map(|x| x.split_whitespace()).map(fold).filter(|w| !w.is_empty()).collect();
        let joined = folded_words.join("");
        let as_words = !words.is_empty() && words.iter().all(|w| folded_words.contains(w));
        if (as_words || (g.key.len() >= 4 && joined.contains(&g.key))) && !p.genres.iter().any(|x| fold(x) == g.key) {
            p.genres.push(g.name.clone());
            matched_keys.push(g.key.clone());
        }
    }
    let moods: [(&[&str], &str); 7] = [
        (&["sad", "melancholy", "melancholic", "heartbreak", "galau", "sedih", "lonely"], "sad"),
        (&["happy", "cheerful", "joyful", "sunny", "senang", "ceria"], "happy"),
        (&["calm", "chill", "relax", "relaxing", "quiet", "mellow", "soft", "peaceful", "rainy", "santai", "tenang", "sleep", "sleepy"], "calm"),
        (&["energetic", "upbeat", "hype", "workout", "gym", "party", "pump", "semangat", "running"], "energetic"),
        (&["dark", "moody", "gloomy"], "dark"),
        (&["bright"], "bright"),
        (&["romantic", "love songs", "romance"], "romantic"),
    ];
    for (words, m) in moods {
        if any(words) {
            p.moods.push(m.to_string());
        }
    }
    if any(&["fast"]) {
        p.tempo = Level::High;
    } else if any(&["slow"]) {
        p.tempo = Level::Low;
    }
    if r.contains("haven't heard") || r.contains("havent heard") || r.contains("haven't played") || r.contains("not heard") || r.contains("in a while") || any(&["forgotten", "rediscover", "lama"]) {
        p.familiarity = Familiarity::Forgotten;
    } else if r.contains("never heard") || r.contains("never played") || r.contains("new to me") || any(&["unplayed", "unheard"]) {
        p.familiarity = Familiarity::New;
    } else if any(&["favourite", "favourites", "favorite", "favorites", "loved", "best"]) || r.contains("most played") {
        p.familiarity = Familiarity::Favourites;
    }
    // "in 3 months", "for 2 weeks".
    for (i, t) in tokens.iter().enumerate() {
        if let (Ok(n), Some(unit)) = (t.parse::<u16>(), tokens.get(i + 1)) {
            let days = match unit.trim_end_matches('s') {
                "day" => Some(n),
                "week" => Some(n * 7),
                "month" => Some(n * 30),
                "year" => Some(n * 365),
                _ => None,
            };
            if let Some(d) = days && p.familiarity == Familiarity::Forgotten {
                p.not_played_days = Some(d.max(1));
            }
            match unit.trim_end_matches('s') {
                "song" | "track" | "lagu" => p.count = Some(n.clamp(5, 100)),
                "minute" | "min" | "menit" => p.minutes = Some(n.clamp(10, 600)),
                "hour" | "jam" => p.minutes = Some((n * 60).clamp(10, 600)),
                _ => {}
            }
        }
    }
    if r.contains("an hour") && p.minutes.is_none() {
        p.minutes = Some(60);
    }
    if any(&["instrumental", "instrumentals"]) || r.contains("no vocals") || r.contains("without vocals") {
        p.vocals = Vocals::Instrumental;
    } else if r.contains("with vocals") {
        p.vocals = Vocals::Vocal;
    }
    if r.contains("build up") || r.contains("warm up") || r.contains("builds up") {
        p.order = Order::BuildUp;
    } else if r.contains("wind down") || r.contains("fall asleep") || r.contains("winding down") {
        p.order = Order::WindDown;
    }
    // "by X", "like X", "without X": the words after, up to a comma or "from"/"in".
    let phrase = |after: &str| -> Option<String> {
        let i = r.find(after)? + after.len();
        let rest = &request[i.min(request.len())..];
        let end = [",", " from ", " in ", " and ", " for ", " that ", " with "].iter().filter_map(|s| rest.to_lowercase().find(s)).min().unwrap_or(rest.len());
        let s = rest[..end].trim().trim_matches(|c: char| !c.is_alphanumeric() && !crate::identity::cjk(&c.to_string()));
        (!s.is_empty()).then(|| s.to_string())
    };
    for (after, only) in [(" by ", true), ("songs like ", false), (" like ", false)] {
        if let Some(name) = phrase(after) {
            p.artists.push(name);
            p.only_artists = only;
            break;
        }
    }
    for after in [" without ", " except ", " no ", " not "] {
        if let Some(name) = phrase(after)
            && !["vocals", "vocal"].contains(&name.to_lowercase().as_str())
        {
            p.exclude_artists.push(name);
        }
    }
    p
}

fn title_case(request: &str) -> String {
    let t: String = request.trim().chars().take(40).collect();
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

// ---------- artists named in a plan ----------

/// The artist keys a list of names means: the identity's own names and aliases (any spelling or
/// script: "Kenshi Yonezu" is 米津玄師), a name given in part ("yonezu"), and the search's extra
/// words (`credits`: credit as written → words, one per line). Names that match no one are
/// returned apart.
pub fn resolve_artists(names: &[String], id: &Identity, credits: &[(String, String)]) -> (Vec<String>, Vec<String>) {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    let words_of = |s: &str| -> Vec<String> { crate::searchkeys::sfold(s).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect() };
    for n in names {
        let f = fold(n);
        if f.is_empty() {
            continue;
        }
        let mut keys: Vec<String> = Vec::new();
        if let Some(i) = id.of_name(n) {
            keys.push(id.artist(i).key.clone());
        } else {
            let q = words_of(n);
            let partial_ok = q.iter().all(|w| w.chars().count() >= 3);
            for a in id.artists() {
                let hit = a.names.iter().map(|(s, _)| s).chain(&a.aliases).any(|s| {
                    let fs = fold(s);
                    fs == f || (partial_ok && !q.is_empty() && { let w = words_of(s); q.iter().all(|x| w.contains(x)) }) || (crate::identity::cjk(n) && f.chars().count() >= 2 && fs.contains(&f))
                });
                if hit && !keys.contains(&a.key) {
                    keys.push(a.key.clone());
                }
            }
            if keys.is_empty() {
                for (credit, words) in credits {
                    let ws: Vec<&str> = words.lines().collect();
                    let all_words: Vec<String> = ws.iter().flat_map(|w| words_of(w)).collect();
                    if ws.iter().any(|w| fold(w) == f) || (partial_ok && !q.is_empty() && q.iter().all(|x| all_words.contains(x))) {
                        let k = id.primary_key(credit);
                        if !keys.contains(&k) {
                            keys.push(k);
                        }
                    }
                }
            }
            if keys.is_empty() {
                // Not in the identity (a name only `fold` knows).
                keys.push(id.primary_key(n));
            }
        }
        if keys.is_empty() {
            missing.push(n.clone());
        }
        for k in keys {
            if !found.contains(&k) {
                found.push(k);
            }
        }
    }
    (found, missing)
}

// ---------- retrieval ----------

/// What the listener's history and marks say, by song id.
pub struct Taste<'a> {
    pub plays: &'a HashMap<String, u32>,
    pub skips: &'a HashMap<String, u32>,
    pub last: &'a HashMap<String, u64>,
    pub marks: &'a HashMap<String, Mark>,
    pub now: u64,
}

/// The plan as the retriever applies it (artists resolved to keys).
pub struct Query<'a> {
    pub plan: &'a Plan,
    /// Artists asked for, and left out (indexes into `Facts::artists`).
    pub include: HashSet<u32>,
    pub exclude: HashSet<u32>,
    /// Songs never played (skipped songs, and instrumentals when those are left out).
    pub never: &'a HashSet<String>,
    pub seed: u64,
}

/// A constraint relaxed because too few songs fit it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relaxed {
    Familiarity,
    Years,
    Vocals,
}

#[derive(Debug, Default)]
pub struct Retrieved {
    /// Indexes into `Facts::songs`, best first, spread over artists and albums (the pool offered
    /// to the model, and what the local pick comes from).
    pub pool: Vec<u32>,
    /// How many songs the playlist should have (or, with `minutes`, a guess for the pool's size).
    pub wanted: usize,
    pub relaxed: Option<Relaxed>,
    /// Songs that passed the hard filters.
    pub matched: usize,
    /// Songs in the asked-for languages at all (when languages were asked for).
    pub in_language: Option<usize>,
    /// Songs by the artists asked for at all (when only their songs were asked for).
    pub by_artists: Option<usize>,
    pub per_artist: usize,
    pub per_album: usize,
}

fn passes(facts: &Facts, s: &Song, q: &Query, t: &Taste, relaxed: Option<Relaxed>) -> bool {
    let p = q.plan;
    if q.never.contains(&s.id) || s.credit.iter().any(|a| q.exclude.contains(a)) {
        return false;
    }
    if !p.languages.is_empty() && !facts.sung_in(s, &p.languages) {
        return false;
    }
    let years = !p.decades.is_empty() || p.year_from.is_some() || p.year_to.is_some();
    if years {
        let widen = if relaxed == Some(Relaxed::Years) { 5 } else { 0 };
        let Some(y) = s.year else { return false };
        let in_decade = p.decades.iter().any(|&d| y + widen >= d && y <= d + 9 + widen);
        let ranged = (p.year_from.is_some() || p.year_to.is_some())
            && p.year_from.is_none_or(|f| y + widen >= f)
            && p.year_to.is_none_or(|to| y <= to + widen);
        if !in_decade && !ranged {
            return false;
        }
    }
    if relaxed != Some(Relaxed::Vocals) {
        match p.vocals {
            Vocals::Instrumental if !s.instrumental => return false,
            Vocals::Vocal if s.instrumental => return false,
            _ => {}
        }
    }
    if p.only_artists && !s.credit.iter().any(|a| q.include.contains(a)) {
        return false;
    }
    if relaxed != Some(Relaxed::Familiarity) {
        let plays = t.plays.get(&s.id).copied().unwrap_or(0);
        let last = t.last.get(&s.id).copied().unwrap_or(0);
        match p.familiarity {
            Familiarity::Forgotten => {
                let days = p.not_played_days.unwrap_or(90) as u64;
                if plays == 0 || t.now.saturating_sub(last) < days * DAY {
                    return false;
                }
            }
            Familiarity::New if plays > 0 => return false,
            _ => {}
        }
    }
    true
}

/// How a song's sound fits the plan's moods, energy and tempo (0 when nothing about sound was
/// asked for).
fn sound_target(p: &Plan) -> [f32; 4] {
    // tempo, loud, bright, minor
    let mut d = [0.0f32; 4];
    for m in &p.moods {
        let add: [f32; 4] = match m.as_str() {
            "sad" => [-0.5, -0.3, -0.2, 1.0],
            "happy" => [0.3, 0.0, 0.5, -1.0],
            "calm" => [-0.7, -1.0, 0.0, 0.0],
            "energetic" => [0.8, 1.0, 0.2, 0.0],
            "dark" => [0.0, 0.0, -1.0, 0.5],
            "bright" => [0.0, 0.0, 1.0, -0.3],
            "romantic" => [-0.3, -0.5, 0.0, -0.2],
            _ => [0.0; 4],
        };
        for k in 0..4 {
            d[k] += add[k];
        }
    }
    match p.energy {
        Level::Low => {
            d[0] -= 0.5;
            d[1] -= 1.0;
        }
        Level::High => {
            d[0] += 0.5;
            d[1] += 1.0;
        }
        _ => {}
    }
    match p.tempo {
        Level::Low => d[0] -= 1.0,
        Level::High => d[0] += 1.0,
        _ => {}
    }
    d
}

fn as_vec(t: &Traits) -> [f32; 4] {
    [t.tempo, t.loud, t.bright, t.minor]
}

/// Finds the songs for a plan (see the module).
pub fn retrieve(facts: &Facts, q: &Query, t: &Taste) -> Retrieved {
    let p = q.plan;
    let wanted = match (p.count, p.minutes) {
        (Some(n), _) => n as usize,
        (None, Some(m)) => ((m as f32 * 60.0 / 230.0).ceil() as usize).clamp(5, 150),
        _ => 25,
    };
    let survivors = |relaxed: Option<Relaxed>| -> Vec<u32> {
        facts.songs.iter().enumerate().filter(|(_, s)| passes(facts, s, q, t, relaxed)).map(|(i, _)| i as u32).collect()
    };
    let mut relaxed = None;
    let mut list = survivors(None);
    let need = MIN_POOL.min(wanted.max(8));
    if list.len() < need {
        // The weakest constraint first; the language, the artists asked for only and those left
        // out are never relaxed (too few of those is said, not padded).
        let order = [
            (p.familiarity == Familiarity::Forgotten || p.familiarity == Familiarity::New, Relaxed::Familiarity),
            (!p.decades.is_empty() || p.year_from.is_some() || p.year_to.is_some(), Relaxed::Years),
            (p.vocals != Vocals::Any, Relaxed::Vocals),
        ];
        if let Some((_, r)) = order.iter().find(|(on, _)| *on) {
            let again = survivors(Some(*r));
            if again.len() > list.len() {
                relaxed = Some(*r);
                list = again;
            }
        }
    }
    let allowed = |s: &Song| !q.never.contains(&s.id) && !s.credit.iter().any(|a| q.exclude.contains(a));
    let in_language = (!p.languages.is_empty()).then(|| facts.songs.iter().filter(|s| facts.sung_in(s, &p.languages) && allowed(s)).count());
    let by_artists = p.only_artists.then(|| facts.songs.iter().filter(|s| s.credit.iter().any(|a| q.include.contains(a)) && allowed(s)).count());

    // Scores.
    let target = sound_target(p);
    let wants_sound = target.iter().any(|x| *x != 0.0);
    let genre_keys: Vec<String> = p.genres.iter().map(|g| fold(g)).filter(|g| !g.is_empty()).collect();
    // The sound of the artists named (not "only"): songs near it score higher.
    let seed_sound: Option<[f32; 4]> = (!q.include.is_empty()).then(|| {
        let v: Vec<[f32; 4]> = facts.songs.iter().filter(|s| s.credit.iter().any(|a| q.include.contains(a))).filter_map(|s| s.traits.as_ref().map(as_vec)).collect();
        (!v.is_empty()).then(|| {
            let mut m = [0.0f32; 4];
            for x in &v {
                for k in 0..4 {
                    m[k] += x[k] / v.len() as f32;
                }
            }
            m
        })
    }).flatten();
    let mut rng = crate::ai::Rng(q.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut scored: Vec<(f32, u32)> = list
        .iter()
        .map(|&i| {
            let s = &facts.songs[i as usize];
            let mut score = 0.0f32;
            if !genre_keys.is_empty() {
                let best = s
                    .genres
                    .iter()
                    .map(|&g| {
                        let k = &facts.genres[g as usize].key;
                        if genre_keys.iter().any(|w| w == k) {
                            3.0
                        } else if genre_keys.iter().any(|w| (w.len() >= 3 && k.contains(w.as_str())) || (k.len() >= 3 && w.contains(k.as_str()))) {
                            1.5
                        } else {
                            0.0
                        }
                    })
                    .fold(0.0f32, f32::max);
                score += best;
            }
            match (&s.traits, wants_sound) {
                (Some(tr), true) => {
                    let v = as_vec(tr);
                    score += (0..4).map(|k| target[k] * v[k].clamp(-2.0, 2.0)).sum::<f32>() * 0.8;
                    if p.energy == Level::Medium {
                        score -= 0.3 * v[1].abs();
                    }
                }
                (None, true) => score -= 0.5,
                _ => {}
            }
            if s.credit.iter().any(|a| q.include.contains(a)) {
                score += 5.0;
            } else if let (Some(seed), Some(tr)) = (seed_sound, &s.traits) {
                let v = as_vec(tr);
                let d2: f32 = (0..4).map(|k| (v[k] - seed[k]).powi(2)).sum();
                score += 2.0 * (-d2 / 2.0).exp();
            }
            // Taste: plays (fading with time since the last), hearts and stars.
            let plays = t.plays.get(&s.id).copied().unwrap_or(0) as f32;
            let skips = t.skips.get(&s.id).copied().unwrap_or(0) as f32;
            let last = t.last.get(&s.id).copied().unwrap_or(0);
            let age_days = if last == 0 { 10_000.0 } else { t.now.saturating_sub(last) as f32 / DAY as f32 };
            let fade = 0.5f32.powf(age_days / 120.0);
            let m = t.marks.get(&s.id).copied().unwrap_or_default();
            let mut taste = (1.0 + plays).ln() * (0.4 + 0.6 * fade);
            if p.familiarity == Familiarity::Forgotten {
                taste = (1.0 + plays).ln();
            }
            taste += if m.loved { 1.2 } else { 0.0 }
                + match m.rating {
                    4 | 5 => 0.8,
                    3 => 0.2,
                    1 | 2 => -1.5,
                    _ => 0.0,
                };
            score += taste
                * match p.familiarity {
                    Familiarity::Favourites => 2.0,
                    Familiarity::New => 0.2,
                    _ => 1.0,
                };
            if p.familiarity == Familiarity::New && plays == 0.0 {
                score += 1.0;
                // Added in the last two months: newer to the listener still.
                if s.added > 0 && t.now.saturating_sub(s.added) < 60 * DAY {
                    score += 0.5;
                }
            }
            if p.familiarity == Familiarity::Favourites && m.loved {
                score += 1.5;
            }
            // Heard in the last day or two: a little further down (not the same songs again).
            if p.familiarity != Familiarity::Favourites {
                score -= 1.5 * crate::shuffle::recency(last, t.now);
            }
            if skips >= 2.0 && skips > plays {
                score -= 1.5 + 0.3 * skips.min(10.0);
            }
            score += rng.unit() as f32 * 1.2;
            (score, i)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    // The pool: best first, one song once (the same title by the same artist on two albums),
    // at most twice the playlist's per-artist and per-album caps.
    let n_include = q.include.len().max(1);
    let per_artist = if p.only_artists { wanted.div_ceil(n_include) + 1 } else { wanted.div_ceil(10).max(2) };
    let per_album = per_artist;
    let offered = (wanted * 3).clamp(60, MAX_OFFERED);
    let mut pool = Vec::new();
    let mut seen: HashSet<(String, u32)> = HashSet::new();
    let (mut by_artist, mut by_album): (HashMap<u32, usize>, HashMap<u32, usize>) = (HashMap::new(), HashMap::new());
    for &(_, i) in &scored {
        if pool.len() >= offered {
            break;
        }
        let s = &facts.songs[i as usize];
        if !seen.insert((s.title.to_lowercase(), s.artist_key)) {
            continue;
        }
        let a = by_artist.entry(s.artist_key).or_default();
        let b = by_album.entry(s.album).or_default();
        if *a >= per_artist * 2 || *b >= per_album * 2 {
            continue;
        }
        *a += 1;
        *b += 1;
        pool.push(i);
    }
    Retrieved { matched: list.len(), pool, wanted, relaxed, in_language, by_artists, per_artist, per_album }
}

/// The playlist from `order` (the model's numbers into `r.pool`, or None for the local order):
/// songs over the caps dropped, filled up from the scores, cut to the wanted length (or minutes),
/// then spaced so one artist doesn't play twice in a row.
pub fn finish(facts: &Facts, r: &Retrieved, plan: &Plan, order: Option<&[u32]>) -> Vec<u32> {
    let (mut by_artist, mut by_album): (HashMap<u32, usize>, HashMap<u32, usize>) = (HashMap::new(), HashMap::new());
    let mut out: Vec<u32> = Vec::new();
    let mut secs = 0.0f32;
    let full = |out: &Vec<u32>, secs: f32| match plan.minutes {
        Some(m) if plan.count.is_none() => secs >= m as f32 * 60.0 || out.len() >= 200,
        _ => out.len() >= r.wanted,
    };
    let mut take = |i: u32, out: &mut Vec<u32>, secs: &mut f32, loose: usize| {
        let s = &facts.songs[i as usize];
        if out.contains(&i) {
            return;
        }
        let a = by_artist.entry(s.artist_key).or_default();
        let b = by_album.entry(s.album).or_default();
        if *a >= r.per_artist * loose || *b >= r.per_album * loose {
            return;
        }
        *a += 1;
        *b += 1;
        out.push(i);
        *secs += s.secs;
    };
    let local = local_order(facts, r, plan);
    for &i in order.unwrap_or(&local) {
        if full(&out, secs) {
            break;
        }
        take(i, &mut out, &mut secs, 1);
    }
    // Too few: filled from the scores, best first; a library with few artists for the request
    // then gets up to twice the caps rather than a short list.
    for loose in [1, 2] {
        for &i in &r.pool {
            if full(&out, secs) {
                break;
            }
            take(i, &mut out, &mut secs, loose);
        }
    }
    if order.is_some() || plan.order == Order::Any || plan.order == Order::Steady {
        let artists: Vec<&str> = out.iter().map(|&i| facts.artists[facts.songs[i as usize].artist_key as usize].as_str()).collect();
        let albums: Vec<String> = out.iter().map(|&i| facts.songs[i as usize].album.to_string()).collect();
        let album_refs: Vec<&str> = albums.iter().map(String::as_str).collect();
        let spaced = crate::shuffle::spaced(&artists, &album_refs);
        out = spaced.into_iter().map(|k| out[k]).collect();
    }
    out
}

/// The order without the AI: build up / wind down by energy, otherwise a flow from the best song
/// (each next one the nearest in sound), songs without sound analysis after.
fn local_order(facts: &Facts, r: &Retrieved, plan: &Plan) -> Vec<u32> {
    // The best of the pool first (what the playlist will hold), then put in order.
    let mut chosen: Vec<u32> = r.pool.clone();
    let energy = |i: u32| facts.songs[i as usize].traits.map_or(0.0, |t| t.loud + t.tempo * 0.6);
    match plan.order {
        Order::BuildUp => {
            chosen.truncate(r.wanted.max(8) + r.wanted / 2);
            chosen.sort_by(|a, b| energy(*a).total_cmp(&energy(*b)));
        }
        Order::WindDown => {
            chosen.truncate(r.wanted.max(8) + r.wanted / 2);
            chosen.sort_by(|a, b| energy(*b).total_cmp(&energy(*a)));
        }
        _ => {
            chosen.truncate(r.wanted.max(8) + r.wanted / 2);
            let (with, without): (Vec<u32>, Vec<u32>) = chosen.iter().partition(|&&i| facts.songs[i as usize].traits.is_some());
            let mut rest = with;
            let mut flow = Vec::with_capacity(rest.len());
            if !rest.is_empty() {
                let mut cur = rest.remove(0);
                flow.push(cur);
                while !rest.is_empty() {
                    let cv = as_vec(facts.songs[cur as usize].traits.as_ref().expect("analysed"));
                    let (k, _) = rest
                        .iter()
                        .enumerate()
                        .map(|(k, &i)| {
                            let v = as_vec(facts.songs[i as usize].traits.as_ref().expect("analysed"));
                            (k, (0..4).map(|d| (v[d] - cv[d]).powi(2)).sum::<f32>())
                        })
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .expect("not empty");
                    cur = rest.remove(k);
                    flow.push(cur);
                }
            }
            flow.extend(without);
            chosen = flow;
        }
    }
    chosen
}

// ---------- words for the answer ----------

fn days_words(d: u16) -> String {
    match d {
        d if d >= 60 => format!("{} months", (d as f32 / 30.0).round() as u32),
        d if d >= 14 => format!("{} weeks", (d as f32 / 7.0).round() as u32),
        1 => "a day".into(),
        d => format!("{d} days"),
    }
}

fn years_words(p: &Plan) -> Option<String> {
    let mut parts: Vec<String> = p.decades.iter().map(|d| format!("{d}s")).collect();
    match (p.year_from, p.year_to) {
        (Some(a), Some(b)) if a == b => parts.push(a.to_string()),
        (Some(a), Some(b)) => parts.push(format!("{a}–{b}")),
        (Some(a), None) => parts.push(format!("since {a}")),
        (None, Some(b)) => parts.push(format!("up to {b}")),
        _ => {}
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// What was understood, in a short line: "Indonesian · calm · not played in 3 months · 25 songs".
pub fn understood(p: &Plan, songs: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !p.languages.is_empty() {
        parts.push(p.languages.iter().map(|l| l.name()).collect::<Vec<_>>().join(" / "));
    }
    if let Some(y) = years_words(p) {
        parts.push(y);
    }
    parts.extend(p.genres.iter().cloned());
    parts.extend(p.moods.iter().cloned());
    match p.energy {
        Level::Low => parts.push("low energy".into()),
        Level::Medium => parts.push("medium energy".into()),
        Level::High => parts.push("high energy".into()),
        Level::Any => {}
    }
    match p.tempo {
        Level::Low => parts.push("slow".into()),
        Level::Medium => parts.push("mid-tempo".into()),
        Level::High => parts.push("fast".into()),
        Level::Any => {}
    }
    if !p.artists.is_empty() {
        parts.push(format!("{} {}", if p.only_artists { "by" } else { "like" }, p.artists.join(", ")));
    }
    if !p.exclude_artists.is_empty() {
        parts.push(format!("no {}", p.exclude_artists.join(", ")));
    }
    match p.familiarity {
        Familiarity::Favourites => parts.push("your favourites".into()),
        Familiarity::Mixed => parts.push("old and new".into()),
        Familiarity::Forgotten => parts.push(format!("not played in {}", days_words(p.not_played_days.unwrap_or(90)))),
        Familiarity::New => parts.push("never played".into()),
        Familiarity::Any => {}
    }
    match p.vocals {
        Vocals::Instrumental => parts.push("instrumental".into()),
        Vocals::Vocal => parts.push("with vocals".into()),
        Vocals::Any => {}
    }
    match p.order {
        Order::BuildUp => parts.push("building up".into()),
        Order::WindDown => parts.push("winding down".into()),
        _ => {}
    }
    match (p.minutes, p.count) {
        (Some(m), None) => parts.push(format!("about {m} minutes")),
        _ => parts.push(format!("{songs} {}", if songs == 1 { "song" } else { "songs" })),
    }
    parts.join(" · ")
}

/// One plain sentence when a constraint was relaxed or couldn't be met (empty otherwise).
pub fn notice(p: &Plan, r: &Retrieved, songs: usize) -> String {
    if let Some(n) = r.in_language
        && n < r.wanted
    {
        let names = p.languages.iter().map(|l| l.name()).collect::<Vec<_>>().join(" or ");
        return match n {
            0 => format!("Your library has no {names} songs that Cadence can tell."),
            1 => format!("Your library has only one {names} song Cadence can tell, so the list is short."),
            n => format!("Your library has only {n} {names} songs Cadence can tell, so the list is shorter."),
        };
    }
    if let Some(n) = r.by_artists
        && n < r.wanted
    {
        let names = p.artists.join(", ");
        return match n {
            0 => format!("Your library has no songs by {names}."),
            1 => format!("Your library has only one song by {names}."),
            n => format!("Your library has only {n} songs by {names}, so the list is shorter."),
        };
    }
    let relaxed = match r.relaxed {
        Some(Relaxed::Familiarity) if p.familiarity == Familiarity::New => "Few songs you've never played fit, so some you have played are in too.".to_string(),
        Some(Relaxed::Familiarity) => format!("Few songs you haven't played in {} fit, so some played more recently are in too.", days_words(p.not_played_days.unwrap_or(90))),
        Some(Relaxed::Years) => format!("Few songs from {} fit, so the years around them are in too.", years_words(p).unwrap_or_default()),
        Some(Relaxed::Vocals) if p.vocals == Vocals::Instrumental => "Few instrumental songs fit, so some with vocals are in too.".to_string(),
        Some(Relaxed::Vocals) => "Few songs with vocals fit, so some instrumentals are in too.".to_string(),
        None => String::new(),
    };
    if !relaxed.is_empty() {
        return relaxed;
    }
    if songs < r.wanted && p.minutes.is_none() {
        return format!("Only {songs} {} fit everything you asked for.", if songs == 1 { "song" } else { "songs" });
    }
    String::new()
}

// ---------- the model ----------

/// One JSON request to a model.
pub struct Request<'a> {
    /// What it's for ("plan", "order"): the schema's name, and the log.
    pub name: &'a str,
    /// Instructions that never change (first, so a service's prompt cache can keep them).
    pub system: &'a str,
    /// Context that changes only with the library (the vocabulary); may be "".
    pub context: &'a str,
    pub prompt: &'a str,
    pub schema: &'a Value,
}

/// Answers one JSON request (the app's AI; canned answers in tests).
pub trait Llm {
    fn json(&self, r: &Request) -> Result<Value, String>;
}

/// The app's AI.
pub struct AppLlm(pub crate::ai::AiSettings);

impl Llm for AppLlm {
    fn json(&self, r: &Request) -> Result<Value, String> {
        crate::ai::dj_json(&self.0, r.name, r.system, r.context, r.prompt, r.schema)
    }
}

pub const PLAN_SYSTEM: &str = "You plan playlists for Cadence, a personal music player. You never see the listener's library: \
you turn their request into a search plan that Cadence runs over the library itself. Answer with JSON only, matching the schema.\n\
Rules:\n\
- Use only languages, genres and words from the VOCABULARY; leave a list empty, a number null and a choice \"any\" when the request doesn't say.\n\
- languages: only when the request names a language, country or scene (J-pop or anime songs: ja; K-pop: ko; Mandopop: zh; \
Indonesian or \"lagu Indonesia\": id; Malay: ms; English: en).\n\
- decades / year_from / year_to: only when years are given (\"2010s\": decades [2010]; \"late 90s\": year_from 1995, year_to 1999).\n\
- genres: names from the VOCABULARY's genres that fit (the closest ones; none if nothing fits).\n\
- moods, energy, tempo: what the request implies (a rainy night: calm, low energy; a workout: energetic, high energy, fast).\n\
- artists: names as the listener wrote them. only_artists: true when they want songs BY those artists, false for \"like\" or \"similar to\".\n\
- exclude_artists: artists they don't want.\n\
- familiarity: favourites (their most played or loved), forgotten (\"haven't heard in a while\"; not_played_days if a time is given, \
e.g. 3 months: 90), new (\"never played\", \"new to me\"), mixed, or any.\n\
- vocals: instrumental, vocal or any. count: number of songs if given. minutes: length if given (\"an hour\": 60).\n\
- order: build_up, wind_down (for sleeping), steady, or any.\n\
- title: a playlist title of at most 5 words, in the language of the request.";

/// The vocabulary the retriever understands, for the plan request (no song, title or artist).
pub fn vocabulary(f: &Facts) -> String {
    let langs: Vec<String> = f.languages.iter().map(|(l, n)| format!("{} ({}, {n})", l.code(), l.name())).collect();
    let genres: Vec<String> = f.top_genres(40).iter().map(|g| format!("{} ({})", g.name, g.songs)).collect();
    let decades: Vec<String> = f.decades.iter().filter(|(_, n)| *n >= 3).map(|(d, n)| format!("{d}s ({n})")).collect();
    format!(
        "VOCABULARY (the library: {total} songs; counts in brackets)\n\
languages: {langs}\n\
genres: {genres}\n\
decades: {decades}\n\
moods: {moods} (from the sound analysis of {analysed} songs: tempo, loudness, brightness, major or minor key)\n\
energy, tempo: low, medium, high, any\n\
facts per song: language, year, genre, length, sound, instrumental, plays, last played, loved, rating, date added",
        total = f.songs.len(),
        langs = if langs.is_empty() { "unknown".into() } else { langs.join(", ") },
        genres = if genres.is_empty() { "none tagged".into() } else { genres.join(", ") },
        decades = if decades.is_empty() { "none tagged".into() } else { decades.join(", ") },
        moods = MOODS.join(", "),
        analysed = f.analysed,
    )
}

/// The plan request's own part: the request and the date and time.
pub fn plan_prompt(request: &str, now_words: &str) -> String {
    format!("Now: {now_words}.\nThe listener asks: \"{}\"", request.replace('"', "'").chars().take(500).collect::<String>())
}

/// Asks for a plan; an answer that doesn't fit is asked for once more with what was wrong.
pub fn ask_plan(llm: &dyn Llm, f: &Facts, request: &str, now_words: &str) -> Result<Plan, String> {
    let schema = plan_schema();
    let vocab = vocabulary(f);
    let base = plan_prompt(request, now_words);
    let mut prompt = base.clone();
    let mut last = String::new();
    for attempt in 0..2 {
        let v = llm.json(&Request { name: "plan", system: PLAN_SYSTEM, context: &vocab, prompt: &prompt, schema: &schema })?;
        match parse_plan(&v) {
            Ok(p) => return Ok(p),
            Err(e) => {
                log::info!("DJ plan answer didn't fit (attempt {}): {e}", attempt + 1);
                last = e.clone();
                prompt = format!("{base}\nYour previous answer was not valid: {e}. Answer again with JSON matching the schema.");
            }
        }
    }
    Err(format!("The AI's plan wasn't usable ({last})."))
}

pub fn order_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "reason": { "type": "string" },
            "ids": { "type": "array", "items": { "type": "integer" } },
            "discover": { "type": "array", "items": {
                "type": "object",
                "properties": { "artist": { "type": "string" }, "title": { "type": "string" }, "why": { "type": "string" } },
                "required": ["artist", "title", "why"],
                "additionalProperties": false,
            } },
        },
        "required": ["title", "reason", "ids", "discover"],
        "additionalProperties": false,
    })
}

pub const ORDER_SYSTEM: &str = "You are the DJ inside Cadence, a personal music player. Cadence has already picked candidate songs \
from the listener's own library for their request. Choose from the candidates only, by number, and order the songs so they flow. \
Answer with JSON only, matching the schema. title: at most 5 words, in the language of the request. reason: one short sentence. \
discover: up to 4 real, existing songs NOT among the candidates that fit the request (artist and title exactly as released; \
why: at most 12 words); an empty list if you're not sure.";

/// A few words about a song's sound for the order request ("fast, soft, minor, ~128 BPM").
fn sound_words(t: &Traits) -> String {
    let mut w: Vec<String> = Vec::new();
    for (z, hi, lo) in [(t.tempo, "fast", "slow"), (t.loud, "loud", "soft"), (t.bright, "bright", "dark"), (t.minor, "minor", "major")] {
        if z.abs() >= 0.6 {
            w.push(if z > 0.0 { hi } else { lo }.to_string());
        }
    }
    w.push(format!("~{} BPM", t.bpm));
    w.join(", ")
}

/// The order request: the request, what was understood, and the pool as numbered lines.
pub fn order_prompt(f: &Facts, r: &Retrieved, request: &str, understood: &str, plan: &Plan) -> String {
    let clean = |s: &str| s.replace(['|', '\n'], " ");
    let lines: Vec<String> = r
        .pool
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let s = &f.songs[i as usize];
            let genre = s.genres.first().map(|&g| f.genres[g as usize].name.as_str()).unwrap_or("");
            format!(
                "{}|{}|{}|{}|{}|{}|{}",
                k + 1,
                clean(&s.title),
                clean(&s.artist),
                s.year.map(|y| y.to_string()).unwrap_or_default(),
                if s.lang_conf >= LANG_MIN && s.lang != Lang::Unknown { s.lang.code() } else { "" },
                clean(genre),
                s.traits.as_ref().map(sound_words).unwrap_or_default()
            )
        })
        .collect();
    let flow = match plan.order {
        Order::BuildUp => " building up in energy",
        Order::WindDown => " winding down to the quietest",
        _ => "",
    };
    let length = match (plan.minutes, plan.count) {
        (Some(m), None) => format!("songs for about {m} minutes"),
        _ => format!("{} songs", r.wanted),
    };
    format!(
        "The listener asked: \"{req}\"\nUnderstood: {understood}\nPick {length}{flow}, at most {cap} by one artist.\n\
CANDIDATES (number|title|artist|year|language|genre|sound):\n{list}",
        req = request.replace('"', "'").chars().take(500).collect::<String>(),
        cap = r.per_artist,
        list = lines.join("\n"),
    )
}

/// The model's order as indexes into `facts.songs`: numbers that weren't offered are dropped.
pub fn read_order(v: &Value, r: &Retrieved) -> Vec<u32> {
    let mut seen = HashSet::new();
    v["ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
        .filter_map(|n| r.pool.get((n as usize).checked_sub(1)?).copied())
        .filter(|i| seen.insert(*i))
        .collect()
}

// ---------- the whole question ----------

/// What a question needs besides the facts.
pub struct Ask<'a> {
    pub request: &'a str,
    pub taste: Taste<'a>,
    pub never: &'a HashSet<String>,
    pub identity: &'a Identity,
    /// The search's extra words per credit (for artist names given in part), if at hand.
    pub credits: &'a dyn Fn() -> Vec<(String, String)>,
    /// None: no AI (off or not set up).
    pub llm: Option<&'a dyn Llm>,
    /// Let the AI order and name the songs (the second request).
    pub ai_order: bool,
    pub now_words: &'a str,
    pub seed: u64,
}

/// Answers "Ask the DJ" (see the module). Errors only when nothing at all fits.
pub fn ask(f: &Facts, a: &Ask) -> Result<crate::ai::AiMix, String> {
    let started = std::time::Instant::now();
    let (plan, without_ai) = match a.llm {
        None => (parse_local(a.request, f), "Picked on this computer without the AI (it isn't set up).".to_string()),
        Some(llm) => match ask_plan(llm, f, a.request, a.now_words) {
            Ok(p) => (p, String::new()),
            Err(e) => {
                log::info!("DJ plan failed, using the local parser: {e}");
                (parse_local(a.request, f), "Picked on this computer without the AI: it didn't answer in time or as expected.".to_string())
            }
        },
    };
    let credits_once = std::cell::OnceCell::new();
    let credits = || credits_once.get_or_init(|| (a.credits)());
    let resolve = |names: &[String]| -> HashSet<u32> {
        if names.is_empty() {
            return HashSet::new();
        }
        let (keys, _) = resolve_artists(names, a.identity, &[]);
        let known: HashSet<u32> = keys.iter().filter_map(|k| f.artist_index.get(k).copied()).collect();
        if !known.is_empty() {
            return known;
        }
        let (keys, _) = resolve_artists(names, a.identity, credits());
        keys.iter().filter_map(|k| f.artist_index.get(k).copied()).collect()
    };
    let q = Query { plan: &plan, include: resolve(&plan.artists), exclude: resolve(&plan.exclude_artists), never: a.never, seed: a.seed };
    let r = retrieve(f, &q, &a.taste);
    log::info!("DJ retrieval: {} matched, {} offered, relaxed {:?}, in {:?}", r.matched, r.pool.len(), r.relaxed, started.elapsed());
    if r.pool.is_empty() {
        let n = notice(&plan, &r, 0);
        return Err(if n.is_empty() { "No songs in your library fit that. Try asking another way.".into() } else { n });
    }
    let mut title = plan.title.clone();
    let mut reason = String::new();
    let mut discover = Vec::new();
    let mut without_ai = without_ai;
    let mut ordered: Option<Vec<u32>> = None;
    if let (Some(llm), true, true) = (a.llm, a.ai_order, without_ai.is_empty()) {
        let line = understood(&plan, r.wanted);
        let prompt = order_prompt(f, &r, a.request, &line, &plan);
        match llm.json(&Request { name: "order", system: ORDER_SYSTEM, context: "", prompt: &prompt, schema: &order_schema() }) {
            Ok(v) => {
                let o = read_order(&v, &r);
                if !o.is_empty() {
                    ordered = Some(o);
                }
                let text = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
                if !text("title").is_empty() {
                    title = text("title").chars().take(60).collect();
                }
                reason = text("reason");
                discover = v["discover"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|d| crate::ai::Discovery { artist: d["artist"].as_str().unwrap_or("").trim().into(), title: d["title"].as_str().unwrap_or("").trim().into(), why: d["why"].as_str().unwrap_or("").trim().into(), ..Default::default() })
                    .filter(|d| !d.artist.is_empty() && !d.title.is_empty())
                    .take(4)
                    .collect();
            }
            Err(e) => {
                log::info!("DJ order failed, ordering locally: {e}");
                without_ai = "Ordered on this computer: the AI didn't answer the second request.".into();
            }
        }
    }
    let list = finish(f, &r, &plan, ordered.as_deref());
    if title.trim().is_empty() {
        title = title_case(a.request);
    }
    Ok(crate::ai::AiMix {
        title,
        reason,
        track_ids: list.iter().map(|&i| f.songs[i as usize].id.clone()).collect(),
        discover: crate::ai::distinct_picks(discover),
        understood: understood(&plan, list.len()),
        notice: notice(&plan, &r, list.len()),
        without_ai,
        ..Default::default()
    })
}

/// Exactly what the plan request sends (Settings' "See what's sent"), and what the second sends.
pub fn preview(f: &Facts, request: &str, now_words: &str) -> String {
    format!(
        "1. The plan (no songs from your library are sent):\n\n{PLAN_SYSTEM}\n\n{}\n\n{}\n\n\
2. Then, if the AI orders the songs: the request, what was understood, and up to {MAX_OFFERED} of the songs Cadence picked \
(number, title, artist, year, language, genre and a few words about the sound).\n\n{ORDER_SYSTEM}",
        vocabulary(f),
        plan_prompt(request, now_words)
    )
}

/// Suggestions for the DJ box from what the library has: its main languages, decades and genres.
pub fn suggestions(f: &Facts) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let langs: Vec<Lang> = f.languages.iter().filter(|(_, n)| *n >= 15).map(|(l, _)| *l).collect();
    // The decade with most songs, for the first language.
    let decade = f.decades.iter().filter(|(_, n)| *n >= 15).max_by_key(|(_, n)| *n).map(|(d, _)| *d);
    let genres: Vec<&Genre> = f.top_genres(3).into_iter().filter(|g| g.songs >= 10).collect();
    if let Some(l) = langs.first() {
        match decade {
            Some(d) => out.push(format!("Upbeat {} songs from the {d}s", l.name())),
            None => out.push(format!("Upbeat {} songs", l.name())),
        }
    }
    if let Some(l) = langs.get(1) {
        out.push(format!("Calm {} songs for a rainy night", l.name()));
    }
    if let Some(g) = genres.first() {
        out.push(format!("{} I haven't heard in a while", g.name));
    }
    if let Some(l) = langs.get(2) {
        out.push(format!("My favourite {} songs", l.name()));
    } else if let Some(g) = genres.get(1) {
        out.push(format!("Energetic {}", g.name));
    }
    if out.len() < 3 {
        for s in ["Calm songs for late-night coding", "Something upbeat to focus", "Songs I haven't heard in a while", "Quiet instrumentals"] {
            if out.len() >= 4 {
                break;
            }
            out.push(s.to_string());
        }
    }
    out.truncate(4);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::songlang::tests::{ENGLISH, INDONESIAN};
    use std::cell::RefCell;

    /// Answers from a list, one per request, and keeps the requests.
    struct Canned {
        answers: RefCell<Vec<Result<Value, String>>>,
        asked: RefCell<Vec<(String, String, String)>>,
    }
    impl Canned {
        fn new(answers: Vec<Result<Value, String>>) -> Self {
            Self { answers: RefCell::new(answers), asked: RefCell::new(Vec::new()) }
        }
    }
    impl Llm for Canned {
        fn json(&self, r: &Request) -> Result<Value, String> {
            self.asked.borrow_mut().push((r.name.to_string(), format!("{}\n{}", r.system, r.context), r.prompt.to_string()));
            let mut a = self.answers.borrow_mut();
            if a.is_empty() { Err("no more answers".into()) } else { a.remove(0) }
        }
    }

    struct Lib {
        tracks: Vec<crate::library::Track>,
        traits: HashMap<String, Traits>,
        lyrics: HashMap<String, Guess>,
    }

    /// An invented library: per language a few artists with albums.
    fn library() -> Lib {
        let mut tracks = Vec::new();
        let mut traits = HashMap::new();
        let mut lyrics = HashMap::new();
        let id_lyrics = crate::songlang::latin(&INDONESIAN.join("\n"));
        let en_lyrics = crate::songlang::latin(&ENGLISH.join("\n"));
        let mut n = 0;
        let mut add = |tracks: &mut Vec<crate::library::Track>, title: String, artist: &str, album: &str, genre: &str, year: u32| {
            n += 1;
            let id = format!("t{n}");
            tracks.push(crate::library::Track { id: id.clone(), title, artist: artist.into(), album: album.into(), album_artist: artist.into(), album_id: format!("{artist}|{album}"), genre: genre.into(), year: Some(year), duration: 240.0, ..Default::default() });
            id
        };
        // Japanese: kana titles, 2010s, J-Pop, five artists × 12 songs.
        for a in 0..5 {
            for k in 0..12 {
                let id = add(&mut tracks, format!("よるのうた{k}"), &format!("架空バンド{a}"), &format!("アルバム{a}"), "J-Pop", 2012 + (k % 8));
                traits.insert(id, Traits { tempo: (k as f32 - 6.0) / 3.0, loud: (k as f32 - 6.0) / 3.0, bright: 0.0, minor: 0.0, bpm: 120.0 });
            }
        }
        // Indonesian (Latin titles, lyrics): three artists × 6 songs = 18, 2000s, Pop.
        for a in 0..3 {
            for k in 0..6 {
                let id = add(&mut tracks, format!("Lagu Rekaan {a}-{k}"), &format!("Grup Rekaan {a}"), &format!("Album Rekaan {a}"), "Pop", 2005);
                lyrics.insert(id.clone(), id_lyrics);
                traits.insert(id, Traits { tempo: -1.0, loud: -1.2, bright: 0.0, minor: 1.0, bpm: 80.0 });
            }
        }
        // English: four artists × 10 songs, 1990s, Rock; one of them with 30 songs on many albums.
        for a in 0..4 {
            for k in 0..10 {
                let id = add(&mut tracks, format!("Made Up Song {a}-{k}"), &format!("Invented Band {a}"), &format!("Record {a}-{}", k % 3), "Rock", 1994);
                lyrics.insert(id.clone(), en_lyrics);
                traits.insert(id, Traits { tempo: 1.0, loud: 1.0, bright: 0.5, minor: -0.5, bpm: 140.0 });
            }
        }
        for k in 0..30 {
            let id = add(&mut tracks, format!("Big Artist Song {k}"), "Prolific Act", &format!("Disc {}", k % 6), "Rock", 1996);
            lyrics.insert(id, en_lyrics);
        }
        // A Japanese artist written two ways (米津玄師 / Kenshi Yonezu in the test identity).
        for k in 0..4 {
            add(&mut tracks, format!("かくうのきょく{k}"), "米津玄師", "架空盤", "J-Pop", 2018);
            add(&mut tracks, format!("Imaginary Tune {k}"), "Kenshi Yonezu", "Imaginary LP", "J-Pop", 2020);
        }
        Lib { tracks, traits, lyrics }
    }

    fn facts_of(lib: &Lib, id: &Identity) -> Facts {
        let input: Vec<TrackIn> = lib
            .tracks
            .iter()
            .map(|t| TrackIn { id: &t.id, title: &t.title, artist: &t.artist, album: &t.album, album_id: &t.album_id, genre: &t.genre, year: t.year, secs: t.duration, added: t.added })
            .collect();
        build_facts(&input, &lib.traits, &lib.lyrics, id)
    }

    struct World {
        plays: HashMap<String, u32>,
        skips: HashMap<String, u32>,
        last: HashMap<String, u64>,
        marks: HashMap<String, Mark>,
        never: HashSet<String>,
        now: u64,
    }
    impl World {
        fn new() -> Self {
            World { plays: HashMap::new(), skips: HashMap::new(), last: HashMap::new(), marks: HashMap::new(), never: HashSet::new(), now: 2_000_000_000 }
        }
        fn taste(&self) -> Taste<'_> {
            Taste { plays: &self.plays, skips: &self.skips, last: &self.last, marks: &self.marks, now: self.now }
        }
    }

    fn run(f: &Facts, w: &World, id: &Identity, request: &str, llm: Option<&dyn Llm>) -> Result<crate::ai::AiMix, String> {
        let none = || Vec::new();
        ask(f, &Ask { request, taste: w.taste(), never: &w.never, identity: id, credits: &none, llm, ai_order: true, now_words: "Sunday evening (20:00)", seed: 7 })
    }

    fn lang_of<'a>(f: &'a Facts, id: &str) -> &'a Song {
        f.songs.iter().find(|s| s.id == id).unwrap()
    }

    fn plan_json(over: Value) -> Value {
        let mut v = json!({
            "title": "Test", "languages": [], "decades": [], "year_from": null, "year_to": null, "genres": [], "moods": [],
            "energy": "any", "tempo": "any", "artists": [], "only_artists": false, "exclude_artists": [], "familiarity": "any",
            "not_played_days": null, "vocals": "any", "count": null, "minutes": null, "order": "any"
        });
        for (k, x) in over.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn plan_answers_are_checked_and_asked_again_once() {
        assert!(parse_plan(&plan_json(json!({ "languages": ["id"], "decades": [2010], "familiarity": "forgotten" }))).is_ok());
        assert!(parse_plan(&json!("not an object")).is_err());
        assert!(parse_plan(&plan_json(json!({ "languages": ["fr"] }))).is_err(), "a language outside the vocabulary");
        assert!(parse_plan(&plan_json(json!({ "energy": "extreme" }))).is_err());
        assert!(parse_plan(&plan_json(json!({ "count": "many" }))).is_err());
        // Lenient where it's safe: missing fields, "90" as a decade, "build up" spelled with a space.
        let p = parse_plan(&json!({ "title": "x", "decades": [90], "order": "build up" })).unwrap();
        assert_eq!((p.decades.clone(), p.order), (vec![1990], Order::BuildUp));

        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        let llm = Canned::new(vec![Ok(plan_json(json!({ "moods": ["weird"] }))), Ok(plan_json(json!({ "languages": ["ja"] })))]);
        let p = ask_plan(&llm, &f, "Japanese songs", "now").unwrap();
        assert_eq!(p.languages, [Lang::Ja]);
        let asked = llm.asked.borrow();
        assert_eq!(asked.len(), 2);
        assert!(asked[1].2.contains("not valid"), "the second request says what was wrong");
        // The plan request carries no song, title or artist of the library.
        for (_, sys, prompt) in asked.iter() {
            assert!(!sys.contains("よるのうた") && !sys.contains("Invented Band") && !prompt.contains("Grup Rekaan"));
        }
        // Two bad answers: given up (the caller then uses the local parser).
        let llm = Canned::new(vec![Ok(json!([])), Ok(json!(1))]);
        assert!(ask_plan(&llm, &f, "x", "now").is_err());
    }

    #[test]
    fn artists_resolve_across_scripts() {
        let id = Identity::from_pairs(&[("米津玄師", "Kenshi Yonezu")]);
        let (keys, missing) = resolve_artists(&["yonezu".into()], &id, &[]);
        assert!(missing.is_empty());
        assert_eq!(keys, vec![id.primary_key("米津玄師")]);
        assert_eq!(resolve_artists(&["KENSHI YONEZU".into()], &id, &[]).0, vec![id.primary_key("米津玄師")]);
        // Through the search's words (a reading the identity doesn't have).
        let credits = vec![("架空バンド0".to_string(), "kakuu bando\nkakuubando".to_string())];
        let id2 = Identity::build(&crate::identity::Input { names: vec![("架空バンド0".into(), 1)], ..Default::default() }, 1);
        assert_eq!(resolve_artists(&["kakuu bando".into()], &id2, &credits).0, vec![id2.primary_key("架空バンド0")]);

        // In a question: "songs by yonezu" gets both spellings' songs, only theirs.
        crate::identity::set_for_test(Some(id.clone()));
        let lib = library();
        let f = facts_of(&lib, &id);
        let w = World::new();
        let llm = Canned::new(vec![Ok(plan_json(json!({ "artists": ["yonezu"], "only_artists": true }))), Err("offline".into())]);
        let mix = run(&f, &w, &id, "songs by yonezu", Some(&llm)).unwrap();
        let artists: HashSet<&str> = mix.track_ids.iter().map(|t| lang_of(&f, t).artist.as_str()).collect();
        assert_eq!(artists, HashSet::from(["米津玄師", "Kenshi Yonezu"]));
        assert_eq!(mix.track_ids.len(), 8);
        assert!(mix.notice.contains("only 8 songs by yonezu"), "{}", mix.notice);
        crate::identity::set_for_test(None);
    }

    #[test]
    fn hard_filters_hold_and_a_scarce_language_is_said_not_padded() {
        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        let w = World::new();
        // Japanese: plenty; never another language.
        let llm = Canned::new(vec![Ok(plan_json(json!({ "languages": ["ja"], "count": 25 }))), Err("offline".into())]);
        let mix = run(&f, &w, &id, "Japanese songs", Some(&llm)).unwrap();
        assert_eq!(mix.track_ids.len(), 25);
        assert!(mix.track_ids.iter().all(|t| lang_of(&f, t).lang == Lang::Ja));
        assert!(mix.notice.is_empty());
        assert!(mix.understood.starts_with("Japanese"), "{}", mix.understood);
        // Indonesian: 18 songs only. All of them (capped per artist), nothing else, and it says so.
        let llm = Canned::new(vec![Ok(plan_json(json!({ "languages": ["id"], "moods": ["sad"], "count": 25 }))), Err("offline".into())]);
        let mix = run(&f, &w, &id, "sad Indonesian songs for a rainy night", Some(&llm)).unwrap();
        assert!(!mix.track_ids.is_empty());
        assert!(mix.track_ids.iter().all(|t| lang_of(&f, t).lang == Lang::Id), "no other language pads the list");
        assert!(mix.notice.contains("only 18 Indonesian songs"), "{}", mix.notice);
        // Malay: none at all → an error that says so.
        let llm = Canned::new(vec![Ok(plan_json(json!({ "languages": ["ms"] })))]);
        let Err(err) = run(&f, &w, &id, "Malay songs", Some(&llm)) else { panic!("Malay songs found") };
        assert!(err.contains("no Malay songs"), "{err}");
        // Decades and excluded artists.
        let llm = Canned::new(vec![Ok(plan_json(json!({ "decades": [1990], "exclude_artists": ["Prolific Act"] }))), Err("offline".into())]);
        let mix = run(&f, &w, &id, "90s rock but no Prolific Act", Some(&llm)).unwrap();
        assert!(mix.track_ids.iter().all(|t| { let s = lang_of(&f, t); s.year.is_some_and(|y| (1990..2000).contains(&y)) && s.artist != "Prolific Act" }));
        // Skipped songs never come back.
        let mut w2 = World::new();
        w2.never = f.songs.iter().filter(|s| s.lang == Lang::Ja).take(30).map(|s| s.id.clone()).collect();
        let llm = Canned::new(vec![Ok(plan_json(json!({ "languages": ["ja"] }))), Err("offline".into())]);
        let mix = run(&f, &w2, &id, "Japanese", Some(&llm)).unwrap();
        assert!(mix.track_ids.iter().all(|t| !w2.never.contains(t)));
    }

    #[test]
    fn relaxing_the_weakest_constraint_once() {
        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        // "Forgotten" Japanese songs: only 5 were played long ago → familiarity is relaxed first.
        let mut w = World::new();
        for s in f.songs.iter().filter(|s| s.lang == Lang::Ja).take(5) {
            w.plays.insert(s.id.clone(), 4);
            w.last.insert(s.id.clone(), w.now - 200 * DAY);
        }
        for s in f.songs.iter().filter(|s| s.lang == Lang::Ja).skip(5).take(20) {
            w.plays.insert(s.id.clone(), 2);
            w.last.insert(s.id.clone(), w.now - 3 * DAY);
        }
        let p = Plan { languages: vec![Lang::Ja], decades: vec![2010], familiarity: Familiarity::Forgotten, not_played_days: Some(90), ..Default::default() };
        let q = Query { plan: &p, include: HashSet::new(), exclude: HashSet::new(), never: &w.never, seed: 1 };
        let r = retrieve(&f, &q, &w.taste());
        assert_eq!(r.relaxed, Some(Relaxed::Familiarity), "the weakest goes first, the decade stays");
        assert!(r.pool.iter().all(|&i| f.songs[i as usize].year.is_some_and(|y| (2010..2020).contains(&y))));
        let list = finish(&f, &r, &p, None);
        assert!(notice(&p, &r, list.len()).contains("haven't played in 3 months"), "{}", notice(&p, &r, list.len()));
        // The forgotten ones still come first.
        let forgotten: HashSet<String> = f.songs.iter().filter(|s| s.lang == Lang::Ja).take(5).map(|s| s.id.clone()).collect();
        let top: Vec<&str> = r.pool.iter().take(8).map(|&i| f.songs[i as usize].id.as_str()).collect();
        assert!(top.iter().filter(|t| forgotten.contains(**t)).count() >= 3, "{top:?}");
        // Without a familiarity: the years are relaxed (2000s Indonesian asked as 2010s).
        let p = Plan { decades: vec![2000], genres: vec!["Pop".into()], vocals: Vocals::Any, ..Default::default() };
        let q = Query { plan: &p, include: HashSet::new(), exclude: HashSet::new(), never: &w.never, seed: 1 };
        let r = retrieve(&f, &q, &w.taste());
        assert_eq!(r.relaxed, Some(Relaxed::Years));
    }

    #[test]
    fn one_artist_cannot_fill_the_list() {
        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        let mut w = World::new();
        // The prolific artist is the favourite by far.
        for s in f.songs.iter().filter(|s| s.artist == "Prolific Act") {
            w.plays.insert(s.id.clone(), 50);
            w.last.insert(s.id.clone(), w.now - 10 * DAY);
        }
        let llm = Canned::new(vec![Ok(plan_json(json!({ "languages": ["en"], "familiarity": "favourites", "count": 10 }))), Err("offline".into())]);
        let mix = run(&f, &w, &id, "my favourite English songs", Some(&llm)).unwrap();
        let per = |name: &str| mix.track_ids.iter().filter(|t| lang_of(&f, t).artist == name).count();
        assert!(per("Prolific Act") <= 2, "{}", per("Prolific Act"));
        let mut albums: HashMap<u32, usize> = HashMap::new();
        for t in &mix.track_ids {
            *albums.entry(lang_of(&f, t).album).or_default() += 1;
        }
        assert!(albums.values().all(|&n| n <= 2));
        // Never the same artist twice in a row where it can be helped.
        for w2 in mix.track_ids.windows(2) {
            assert_ne!(lang_of(&f, &w2[0]).artist_key, lang_of(&f, &w2[1]).artist_key);
        }
    }

    #[test]
    fn forgotten_uses_history() {
        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        let mut w = World::new();
        let en: Vec<&Song> = f.songs.iter().filter(|s| s.lang == Lang::En).collect();
        for (k, s) in en.iter().enumerate() {
            w.plays.insert(s.id.clone(), 3);
            // Half heard a year ago, half yesterday.
            w.last.insert(s.id.clone(), if k % 2 == 0 { w.now - 365 * DAY } else { w.now - DAY });
        }
        let p = Plan { languages: vec![Lang::En], familiarity: Familiarity::Forgotten, not_played_days: Some(180), count: Some(20), ..Default::default() };
        let q = Query { plan: &p, include: HashSet::new(), exclude: HashSet::new(), never: &w.never, seed: 3 };
        let r = retrieve(&f, &q, &w.taste());
        assert_eq!(r.relaxed, None);
        assert!(r.pool.iter().all(|&i| w.now - w.last[&f.songs[i as usize].id] >= 180 * DAY));
        // New to me: never played only.
        let p = Plan { familiarity: Familiarity::New, ..Default::default() };
        let q = Query { plan: &p, include: HashSet::new(), exclude: HashSet::new(), never: &w.never, seed: 3 };
        let r = retrieve(&f, &q, &w.taste());
        assert!(r.pool.iter().all(|&i| !w.plays.contains_key(&f.songs[i as usize].id)));
    }

    #[test]
    fn the_order_cannot_add_songs_and_is_filled_up() {
        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        let w = World::new();
        let order = json!({ "title": "Night Lights", "reason": "Soft songs.", "ids": [3, 1, 3, 999, 0, -2, "2"], "discover": [{ "artist": "Someone", "title": "Some Song", "why": "fits" }] });
        let llm = Canned::new(vec![Ok(plan_json(json!({ "languages": ["ja"], "count": 12 }))), Ok(order)]);
        let mix = run(&f, &w, &id, "Japanese songs", Some(&llm)).unwrap();
        assert_eq!(mix.title, "Night Lights");
        assert_eq!(mix.reason, "Soft songs.");
        assert_eq!(mix.track_ids.len(), 12, "filled up from the scores");
        assert!(mix.track_ids.iter().all(|t| lang_of(&f, t).lang == Lang::Ja));
        let set: HashSet<&String> = mix.track_ids.iter().collect();
        assert_eq!(set.len(), 12, "each song once");
        assert!(mix.without_ai.is_empty());
        assert_eq!(mix.discover.len(), 1);
        // The order request sends only the offered songs' lines.
        let asked = llm.asked.borrow();
        assert_eq!(asked[1].0, "order");
        assert!(!asked[1].2.contains("Grup Rekaan"), "an Indonesian song wasn't offered for a Japanese request");
        // read_order on its own: only offered numbers, once.
        let r = Retrieved { pool: vec![10, 20, 30], ..Default::default() };
        assert_eq!(read_order(&json!({ "ids": [2, 2, 4, 1, "3", -1] }), &r), vec![20, 10, 30]);
    }

    #[test]
    fn without_the_ai_the_local_parser_plans() {
        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        let mut w = World::new();
        for s in f.songs.iter().filter(|s| s.lang == Lang::Ja).take(30) {
            w.plays.insert(s.id.clone(), 2);
            w.last.insert(s.id.clone(), w.now - 120 * DAY);
        }
        let mix = run(&f, &w, &id, "upbeat Japanese songs from the 2010s I haven't heard in a while", None).unwrap();
        assert!(mix.without_ai.contains("without the AI"));
        assert!(!mix.track_ids.is_empty());
        assert!(mix.track_ids.iter().all(|t| { let s = lang_of(&f, t); s.lang == Lang::Ja && s.year.is_some_and(|y| (2010..2020).contains(&y)) }));
        assert!(mix.understood.contains("Japanese") && mix.understood.contains("2010s") && mix.understood.contains("not played in"), "{}", mix.understood);
        // A failing AI: the same, saying so.
        let llm = Canned::new(vec![Err("timeout".into())]);
        let mix = run(&f, &w, &id, "Japanese songs", Some(&llm)).unwrap();
        assert!(mix.without_ai.contains("without the AI"));
        assert!(mix.track_ids.iter().all(|t| lang_of(&f, t).lang == Lang::Ja));
    }

    #[test]
    fn the_phrase_parser() {
        let lib = library();
        let f = facts_of(&lib, &Identity::new());
        let p = parse_local("sad Indonesian songs for a rainy night", &f);
        assert_eq!(p.languages, [Lang::Id]);
        assert!(p.moods.contains(&"sad".to_string()) && p.moods.contains(&"calm".to_string()));
        let p = parse_local("90s rock, 30 songs", &f);
        assert_eq!((p.decades.clone(), p.genres.clone(), p.count), (vec![1990], vec!["Rock".to_string()], Some(30)));
        let p = parse_local("J-Pop from the 2010s I haven't heard in 3 months", &f);
        assert_eq!(p.languages, [Lang::Ja]);
        assert_eq!(p.decades, [2010]);
        assert_eq!(p.genres, ["J-Pop"]);
        assert_eq!((p.familiarity, p.not_played_days), (Familiarity::Forgotten, Some(90)));
        let p = parse_local("quiet instrumentals for an hour", &f);
        assert_eq!((p.vocals, p.minutes), (Vocals::Instrumental, Some(60)));
        let p = parse_local("energetic songs I've never played, building up", &f);
        assert_eq!(p.familiarity, Familiarity::New);
        assert!(p.moods.contains(&"energetic".to_string()));
        let p = parse_local("songs by Kenshi Yonezu", &f);
        assert_eq!((p.artists.clone(), p.only_artists), (vec!["Kenshi Yonezu".to_string()], true));
        let p = parse_local("something like Invented Band 2", &f);
        assert_eq!((p.artists.clone(), p.only_artists), (vec!["Invented Band 2".to_string()], false));
        let p = parse_local("english rock without Prolific Act", &f);
        assert_eq!(p.exclude_artists, ["Prolific Act"]);
        let p = parse_local("lagu galau tahun 2000an", &f);
        assert_eq!((p.decades.clone(), p.moods.clone()), (vec![2000], vec!["sad".to_string()]));
    }

    #[test]
    fn suggestions_come_from_the_library() {
        let lib = library();
        let f = facts_of(&lib, &Identity::new());
        let s = suggestions(&f);
        assert!(!s.is_empty() && s.len() <= 4);
        // The library's biggest language and decade, and its genres; nothing it doesn't have.
        assert!(s[0].contains("English") || s[0].contains("Japanese"), "{s:?}");
        assert!(s.iter().any(|x| x.contains("Rock") || x.contains("J-Pop")), "{s:?}");
        assert!(!s.iter().any(|x| x.contains("Korean") || x.contains("Malay")), "{s:?}");
        // An empty library still gets something to tap.
        assert!(!suggestions(&Facts::default()).is_empty());
    }

    /// Prints the requests' sizes: `cargo test --lib dj::tests::request_sizes -- --nocapture`.
    #[test]
    fn request_sizes() {
        let lib = library();
        let id = Identity::new();
        let f = facts_of(&lib, &id);
        let w = World::new();
        let plan = Plan { languages: vec![Lang::Ja], count: Some(25), ..Default::default() };
        let q = Query { plan: &plan, include: HashSet::new(), exclude: HashSet::new(), never: &w.never, seed: 1 };
        let r = retrieve(&f, &q, &w.taste());
        let p = format!("{PLAN_SYSTEM}\n{}\n{}\n{}", vocabulary(&f), plan_prompt("sad Indonesian songs for a rainy night", "Sunday evening (20:00)"), plan_schema());
        let o = format!("{ORDER_SYSTEM}\n{}\n{}", order_prompt(&f, &r, "x", &understood(&plan, 25), &plan), order_schema());
        println!("plan request: {} chars; order request ({} songs): {} chars", p.chars().count(), r.pool.len(), o.chars().count());
        assert!(p.chars().count() < 6000);
    }

    #[test]
    fn retrieval_on_30000_songs_is_quick() {
        let id = Identity::new();
        let langs = [INDONESIAN[0], ENGLISH[0]];
        let mut tracks = Vec::new();
        let mut traits = HashMap::new();
        let mut lyrics = HashMap::new();
        for i in 0..30_000u32 {
            let t = crate::library::Track {
                id: format!("{i:08x}"),
                title: if i % 3 == 0 { format!("うた{i}") } else { format!("Invented {i}") },
                artist: format!("Artist {}", i % 1500),
                album: format!("Album {}", i % 3000),
                album_id: format!("al{}", i % 3000),
                genre: ["Pop", "Rock", "Jazz", "J-Pop", "Ballad"][(i % 5) as usize].into(),
                year: Some(1970 + i % 55),
                duration: 200.0 + (i % 100) as f64,
                ..Default::default()
            };
            if i % 3 != 0 {
                lyrics.insert(t.id.clone(), crate::songlang::latin(&langs[(i % 2) as usize].repeat(3)));
            }
            traits.insert(t.id.clone(), Traits { tempo: ((i % 17) as f32 - 8.0) / 4.0, loud: ((i % 13) as f32 - 6.0) / 3.0, bright: 0.0, minor: ((i % 7) as f32 - 3.0) / 2.0, bpm: 120.0 });
            tracks.push(t);
        }
        let mut w = World::new();
        for t in tracks.iter().step_by(4) {
            w.plays.insert(t.id.clone(), 3);
            w.last.insert(t.id.clone(), w.now - 100 * DAY);
        }
        let started = std::time::Instant::now();
        let input: Vec<TrackIn> = tracks.iter().map(|t| TrackIn { id: &t.id, title: &t.title, artist: &t.artist, album: &t.album, album_id: &t.album_id, genre: &t.genre, year: t.year, secs: t.duration, added: 0 }).collect();
        let f = build_facts(&input, &traits, &lyrics, &id);
        let built = started.elapsed();
        let plan = Plan { languages: vec![Lang::Id], moods: vec!["calm".into()], genres: vec!["Ballad".into()], familiarity: Familiarity::Forgotten, count: Some(25), ..Default::default() };
        let q = Query { plan: &plan, include: HashSet::new(), exclude: HashSet::new(), never: &w.never, seed: 9 };
        let started = std::time::Instant::now();
        let r = retrieve(&f, &q, &w.taste());
        let list = finish(&f, &r, &plan, None);
        let took = started.elapsed();
        println!("30,000 songs: facts in {built:?}, retrieval and pick in {took:?} ({} matched, {} offered)", r.matched, r.pool.len());
        assert_eq!(list.len(), 25);
        assert!(list.iter().all(|&i| f.songs[i as usize].lang == Lang::Id));
        // Unoptimised test builds are several times slower than the app; still well under this.
        assert!(took < std::time::Duration::from_millis(400), "{took:?}");
    }
}
