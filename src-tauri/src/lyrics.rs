//! Lyrics: embedded in the file, a `.lrc` file next to it, or LRCLIB (lrclib.net) as a fallback.
//! The listener can also search LRCLIB, pick a result, or paste lyrics; that choice is kept.
//!
//! Time-synced lyrics (LRC) are preferred; plain text is used when that's all there is.
//! Online results, including "not found", are cached so LRCLIB is asked at most once per song.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lofty::prelude::*;
use serde::{Deserialize, Serialize};

/// Don't ask LRCLIB again for a song it didn't have within this many days.
const RETRY_MISSING_DAYS: u64 = 14;
/// Bumped when the lookup gets better, so earlier "not found" answers are checked again.
const LOOKUP_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Seconds from the start of the song.
    pub time: f64,
    pub text: String,
    /// Word timings from "enhanced" LRC (`<mm:ss.xx>word`), for karaoke-style highlighting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<Word>,
    /// Who sings the line: indexes into `Lyrics::singers` (several when they sing it together);
    /// empty when not known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub singers: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub time: f64,
    pub text: String,
    /// When the word has been sung, if known (words placed on the singing have it); otherwise it
    /// lasts until the next word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    /// "embedded", "file", "lrclib" or "" when none were found.
    pub source: String,
    pub synced: Vec<Line>,
    pub plain: String,
    /// Instrumental (LRCLIB marks these), so there's nothing to show.
    pub instrumental: bool,
    /// Why there's nothing: "off" (online lookup disabled), "missing" (LRCLIB doesn't have it),
    /// "unavailable" (couldn't reach LRCLIB, will retry later) or "" when lyrics were found.
    pub lookup: String,
    /// Timing placed on the singing (see `voice`): "on" when these lines use it, "off" when it
    /// exists but the listener turned it off for this song, "" when the song hasn't had it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub voice: String,
    /// The singers, when the song has several and their lines are known (see `Line::singers`).
    /// A name is "" when the voices were told apart but not named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub singers: Vec<String>,
    /// Where the singers come from: "lyrics" (named in them), "ai", "voice" (told apart by
    /// their voices), or "" when there are none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub singers_from: String,
}

impl Lyrics {
    fn from_text(text: &str, source: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut synced = parse_lrc(text);
        let singers = mark_singers(&mut synced);
        let plain = if synced.is_empty() {
            text.to_string()
        } else {
            synced
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        Some(Self {
            source: source.into(),
            synced,
            plain,
            instrumental: false,
            lookup: String::new(),
            voice: String::new(),
            singers_from: if singers.is_empty() { String::new() } else { "lyrics".into() },
            singers,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.synced.is_empty() && self.plain.trim().is_empty()
    }
}

/// Parses LRC: `[mm:ss.xx] text`, several timestamps per line, and an `[offset:±ms]` tag.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut offset = 0.0;
    let mut lines = Vec::new();
    // An untimed line that only names a singer ("[Cassie]", "Yamato:"): for the next timed line.
    let mut header: Option<String> = None;
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            // After the times, a tag that isn't one names the singer ("[00:12.00][Cassie] …"):
            // it stays in the text, for `mark_singers`.
            if !times.is_empty() && parse_timestamp(tag).is_none() {
                break;
            }
            if times.is_empty() && !is_meta_tag(tag) && parse_timestamp(tag).is_none() {
                if singer_prefix(raw.trim()).is_some_and(|(_, r)| r.is_empty()) {
                    header = Some(raw.trim().to_string());
                }
                break;
            }
            rest = stripped[end + 1..].trim_start();
            if let Some(ms) = tag.strip_prefix("offset:") {
                // A positive offset shows lyrics earlier.
                offset = ms.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
            } else if let Some(t) = parse_timestamp(tag) {
                times.push(t);
            }
        }
        if times.is_empty() {
            if singer_prefix(rest).is_some_and(|(_, r)| r.is_empty()) {
                header = Some(rest.to_string());
            }
            continue;
        }
        let (mut text, words) = parse_words(rest);
        if let Some(h) = header.take() {
            if singer_prefix(&text).is_none() {
                text = format!("{h} {text}");
            }
        }
        for t in times {
            lines.push(Line {
                time: t,
                text: text.clone(),
                words: words.clone(),
                singers: Vec::new(),
            });
        }
    }
    for l in &mut lines {
        l.time = (l.time - offset).max(0.0);
        for w in &mut l.words {
            w.time = (w.time - offset).max(0.0);
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

/// LRC's own tags: `[ti:Title]`, `[ar:Artist]`, `[length:3:20]`…
fn is_meta_tag(tag: &str) -> bool {
    let key = tag.split(':').next().unwrap_or("").trim().to_lowercase();
    tag.contains(':') && ["ti", "ar", "al", "au", "by", "re", "ve", "tool", "length", "offset", "la", "lr", "id", "#"].contains(&key.as_str())
}

// ---------- singers named in the lyrics ----------

/// Names that mean everyone sings the line.
const EVERYONE: [&str; 9] = ["all", "both", "together", "everyone", "tutti", "group", "合唱", "全員", "다같이"];

/// Song parts, not singers: "[Chorus]", "Verse 2:".
const SECTIONS: [&str; 16] = [
    "chorus", "verse", "bridge", "intro", "outro", "hook", "refrain", "interlude", "instrumental", "repeat", "pre", "post", "rap",
    "spoken", "solo", "break",
];

/// A singer's name at the start of a line: "Cassie: …", "[Cassie] …", "(Cassie) …", "【男】…",
/// or a line that is only the name ("[Cassie]", "Cassie:"). Returns the name and the rest.
fn singer_prefix(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    let first = t.chars().next()?;
    let (name, rest) = if matches!(first, '[' | '(' | '（' | '【') {
        let close = match first {
            '[' => ']',
            '(' => ')',
            '（' => '）',
            _ => '】',
        };
        let end = t.find(close)?;
        let name = t[first.len_utf8()..end].trim().trim_end_matches([':', '：']);
        let rest = t[end + close.len_utf8()..].trim_start_matches([':', '：', ' ']);
        // "(Eat up) Fill your stomach": backing vocals. Parentheses name a singer only alone.
        if matches!(first, '(' | '（') && !rest.trim().is_empty() {
            return None;
        }
        (name, rest)
    } else {
        let (name, rest) = t.split_once([':', '：'])?;
        (name, rest)
    };
    let name = name.trim();
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // Singing, not a name: "(Oh oh)", "(La la la)".
    const SUNG: [&str; 14] = ["oh", "ah", "la", "yeah", "hey", "woo", "na", "ooh", "uh", "ha", "mm", "whoa", "da", "yo"];
    if name.is_empty()
        || name.chars().count() > 30
        || name.split_whitespace().count() > 4
        || name.contains(['.', '!', '?', '"', '“', '…'])
        || words.is_empty()
        || words.iter().all(|w| SUNG.contains(w) || w.chars().all(|c| c.is_ascii_digit()))
        || words.iter().any(|w| SECTIONS.contains(w) || w.starts_with("chorus") || w.starts_with("verse"))
    {
        return None;
    }
    Some((name.to_string(), rest.trim().to_string()))
}

/// Several singers in one name: "Cassie & Yamato", "A, B and C", "A x B".
fn split_names(name: &str) -> Vec<String> {
    let mut out = vec![name.to_string()];
    for sep in [" & ", ", ", " and ", " x ", " X ", "&", "/", "、", "・", ","] {
        out = out.iter().flat_map(|n| n.split(sep).map(|p| p.trim().to_string()).collect::<Vec<_>>()).filter(|p| !p.is_empty()).collect();
    }
    out
}

/// Finds singers named in the lyrics (see `singer_prefix`), takes the names out of the lines and
/// marks who sings each: a name holds for the lines after it, until the next one. Lines that are
/// only a name are removed. Needs two or more singers; otherwise nothing changes.
pub fn mark_singers(lines: &mut Vec<Line>) -> Vec<String> {
    let mut found: Vec<Option<(String, String)>> = lines.iter().map(|l| singer_prefix(&l.text)).collect();
    // "Remember this: …" is a line, not a singer: a name before a colon has to come back.
    let bare: Vec<bool> = lines.iter().map(|l| !l.text.trim_start().starts_with(['[', '(', '（', '【'])).collect();
    let uses = |name: &str, found: &[Option<(String, String)>]| found.iter().flatten().filter(|(n, _)| n == name).count();
    let lonely: Vec<bool> = found
        .iter()
        .zip(&bare)
        .map(|(f, &bare)| {
            f.as_ref().is_some_and(|(n, rest)| {
                bare && !rest.is_empty() && uses(n, &found) < 2 && !EVERYONE.contains(&n.to_lowercase().as_str())
                    && !["M", "F", "D", "男", "女"].contains(&n.as_str())
            })
        })
        .collect();
    for (f, lonely) in found.iter_mut().zip(lonely) {
        if lonely {
            *f = None;
        }
    }
    // Karaoke's M / F / D (male, female, duet); D is both.
    let duet = |n: &str| matches!(n, "D" | "d" | "Duet" | "duet" | "M&F" | "F&M");
    let everyone = |n: &str| EVERYONE.contains(&n.to_lowercase().as_str()) || duet(n);
    let mut singers: Vec<String> = Vec::new();
    for (name, _) in found.iter().flatten() {
        if everyone(name) {
            continue;
        }
        for n in split_names(name) {
            if !singers.iter().any(|s| s.eq_ignore_ascii_case(&n)) {
                singers.push(n);
            }
        }
    }
    let marked = found.iter().flatten().count();
    let with_text = lines.iter().filter(|l| !l.text.trim().is_empty()).count().max(1);
    // Two or more names, on enough lines to be a pattern rather than a stray "Note: …".
    if singers.len() < 2 || singers.len() > 12 || marked < 2 || marked * 20 < with_text {
        return Vec::new();
    }
    let index = |n: &str| singers.iter().position(|s| s.eq_ignore_ascii_case(n)).map(|i| i as u8);
    let all: Vec<u8> = (0..singers.len() as u8).collect();
    let mut current: Vec<u8> = Vec::new();
    let mut keep = Vec::with_capacity(lines.len());
    for (line, f) in lines.iter_mut().zip(found) {
        let mut only_name = false;
        if let Some((name, rest)) = f {
            current = if everyone(&name) { all.clone() } else { split_names(&name).iter().filter_map(|n| index(n)).collect() };
            only_name = rest.is_empty();
            // Take the name out of the line, and out of its first timed word.
            let trimmed = line.text.trim();
            let prefix = trimmed[..trimmed.len() - rest.len()].trim().to_string();
            line.text = rest;
            if let Some(w) = line.words.first_mut() {
                if let Some(r) = w.text.trim_start().strip_prefix(&prefix) {
                    w.text = r.trim_start_matches([':', '：', ' ']).to_string();
                }
            }
            line.words.retain(|w| !w.text.trim().is_empty());
        }
        line.singers = if line.text.trim().is_empty() { Vec::new() } else { current.clone() };
        keep.push(!only_name);
    }
    let mut k = keep.into_iter();
    lines.retain(|_| k.next().unwrap_or(true));
    singers
        .into_iter()
        .map(|s| match s.as_str() {
            "M" | "m" | "男" => "Male".into(),
            "F" | "f" | "女" => "Female".into(),
            _ => s,
        })
        .collect()
}

/// Splits `<00:01.00>Hello <00:01.50>world` into plain text and timed words.
fn parse_words(line: &str) -> (String, Vec<Word>) {
    if !line.contains('<') {
        return (line.to_string(), Vec::new());
    }
    let mut words = Vec::new();
    let mut rest = line;
    let mut lead = String::new();
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else { break };
        let Some(t) = parse_timestamp(&rest[start + 1..start + end]) else { break };
        if words.is_empty() {
            lead.push_str(&rest[..start]);
        } else if let Some(last) = words.last_mut() {
            let w: &mut Word = last;
            w.text.push_str(&rest[..start]);
        }
        words.push(Word {
            time: t,
            text: String::new(),
            end: None,
        });
        rest = &rest[start + end + 1..];
    }
    if let Some(last) = words.last_mut() {
        last.text.push_str(rest);
    }
    // A trailing timestamp only marks where the last word ends.
    words.retain(|w| !w.text.trim().is_empty());
    let text: String = lead + &words.iter().map(|w| w.text.as_str()).collect::<String>();
    if words.is_empty() {
        return (line.to_string(), Vec::new());
    }
    (text.trim().to_string(), words)
}

fn parse_timestamp(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let s: f64 = s.trim().replace(':', ".").parse().ok()?;
    (m >= 0.0 && (0.0..60.0).contains(&s)).then_some(m * 60.0 + s)
}

/// Lyrics stored in the file or in a sidecar `.lrc` (no network).
pub fn local(path: &Path) -> Option<Lyrics> {
    let lrc = path.with_extension("lrc");
    if let Ok(text) = std::fs::read_to_string(&lrc) {
        if let Some(l) = Lyrics::from_text(&text, "file") {
            return Some(l);
        }
    }
    let tagged = lofty::read_from_path(path).ok()?;
    for tag in tagged.tags() {
        for key in [ItemKey::Lyrics, ItemKey::UnsyncLyrics] {
            if let Some(text) = tag.get_string(key) {
                if let Some(l) = Lyrics::from_text(text, "embedded") {
                    return Some(l);
                }
            }
        }
    }
    None
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibResponse {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
    #[serde(default)]
    instrumental: bool,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    lyrics: Option<Lyrics>,
    checked: u64,
    /// Chosen by the listener (search result, AI or pasted): used even with online lookup off.
    #[serde(default)]
    picked: bool,
    #[serde(default)]
    v: u32,
    /// The listener marked this song's lyrics as wrong: show none until they pick some.
    #[serde(default)]
    hidden: bool,
}

fn http() -> Result<reqwest::blocking::Client, String> {
    crate::net::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(concat!(
            "Cadence/",
            env!("CARGO_PKG_VERSION"),
            " (open-source music player)"
        ))
        .build()
        .map_err(|e| e.to_string())
}

/// GET with up to three polite retries while LRCLIB says it's busy (503 / 429).
fn get(client: &reqwest::blocking::Client, url: &str) -> Result<reqwest::blocking::Response, String> {
    let mut wait = 1200;
    for attempt in 0..4 {
        match client.get(url).send() {
            Ok(r) if matches!(r.status().as_u16(), 503 | 429) && attempt < 3 => {
                std::thread::sleep(Duration::from_millis(wait));
                wait *= 2;
            }
            Ok(r) => return Ok(r),
            Err(e) => {
                log::info!("lyrics request failed: {e}");
                return Err("Couldn’t reach LRCLIB. Check your internet connection.".into());
            }
        }
    }
    Err("LRCLIB is busy right now. Try again in a minute.".into())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Lyrics the listener chose for this song, if any.
pub fn picked(cache_dir: &Path, track_id: &str) -> Option<Lyrics> {
    let text = std::fs::read_to_string(cache_dir.join(format!("{track_id}.json"))).ok()?;
    let c: Cached = serde_json::from_str(&text).ok()?;
    if c.picked { c.lyrics } else { None }
}

/// Lyrics found online earlier and saved; no network needed, so used even with lookup off.
/// Saved lyrics in the wrong language for the title are ignored (they get looked up again).
pub fn saved(cache_dir: &Path, track_id: &str, title: &str) -> Option<Lyrics> {
    let text = std::fs::read_to_string(cache_dir.join(format!("{track_id}.json"))).ok()?;
    let c = serde_json::from_str::<Cached>(&text).ok()?;
    c.lyrics.filter(|l| c.picked || fits(title, &l.plain))
}

/// Keeps the listener's choice for this song.
pub fn pick(cache_dir: &Path, track_id: &str, lyrics: &Lyrics) {
    let _ = std::fs::create_dir_all(cache_dir);
    let _ = std::fs::write(
        cache_dir.join(format!("{track_id}.json")),
        serde_json::to_vec(&Cached {
            lyrics: Some(lyrics.clone()),
            checked: now(),
            picked: true,
            v: LOOKUP_VERSION,
            hidden: false,
        })
        .unwrap_or_default(),
    );
}

/// Marks a song's lyrics as wrong: none are shown (not even the file's) until the listener picks some.
pub fn hide(cache_dir: &Path, track_id: &str) {
    let _ = std::fs::create_dir_all(cache_dir);
    let _ = std::fs::write(
        cache_dir.join(format!("{track_id}.json")),
        serde_json::to_vec(&Cached {
            lyrics: None,
            checked: now(),
            picked: true,
            v: LOOKUP_VERSION,
            hidden: true,
        })
        .unwrap_or_default(),
    );
}

pub fn is_hidden(cache_dir: &Path, track_id: &str) -> bool {
    std::fs::read_to_string(cache_dir.join(format!("{track_id}.json")))
        .ok()
        .and_then(|t| serde_json::from_str::<Cached>(&t).ok())
        .is_some_and(|c| c.hidden)
}

/// Pasted lyrics (LRC or plain text).
pub fn from_pasted(text: &str) -> Option<Lyrics> {
    Lyrics::from_text(text, "pasted")
}

/// Turns AI-found text (LRC or plain) into lyrics.
pub fn from_ai(text: &str) -> Option<Lyrics> {
    Lyrics::from_text(text, "ai")
}

/// One LRCLIB search result.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub instrumental: bool,
    pub synced: bool,
    /// The first lines, to recognise the right song.
    pub preview: String,
}

/// LRCLIB's search JSON; any field can be null.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHit {
    id: u64,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: Option<bool>,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

impl From<RawHit> for Hit {
    fn from(r: RawHit) -> Self {
        Self {
            id: r.id,
            title: r.track_name.unwrap_or_default(),
            artist: r.artist_name.unwrap_or_default(),
            album: r.album_name.unwrap_or_default(),
            duration: r.duration.unwrap_or(0.0),
            instrumental: r.instrumental.unwrap_or(false),
            synced: r.synced_lyrics.is_some_and(|s| !s.trim().is_empty()),
            preview: r
                .plain_lyrics
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(2)
                .collect::<Vec<_>>()
                .join(" / "),
        }
    }
}

/// Searches LRCLIB for any text (title, artist, a line of the lyrics).
pub fn search(query: &str) -> Result<Vec<Hit>, String> {
    let url = format!("https://lrclib.net/api/search?q={}", urlencoding::encode(query.trim()));
    let r = get(&http()?, &url)?;
    if !r.status().is_success() {
        return Err(format!("LRCLIB answered {}", r.status()));
    }
    let raw: Vec<RawHit> = r.json().map_err(|e| format!("Unexpected answer from LRCLIB: {e}"))?;
    Ok(raw.into_iter().take(30).map(Hit::from).collect())
}

/// Downloads one LRCLIB entry.
pub fn fetch(id: u64) -> Result<Lyrics, String> {
    let r: LrclibResponse = get(&http()?, &format!("https://lrclib.net/api/get/{id}"))?
        .json()
        .map_err(|e| e.to_string())?;
    if r.instrumental {
        return Ok(Lyrics {
            source: "lrclib".into(),
            instrumental: true,
            ..Default::default()
        });
    }
    r.synced_lyrics
        .as_deref()
        .and_then(|t| Lyrics::from_text(t, "lrclib"))
        .or_else(|| r.plain_lyrics.as_deref().and_then(|t| Lyrics::from_text(t, "lrclib")))
        .ok_or_else(|| "That result has no lyrics.".into())
}

/// The writing system of a song's language, as far as lyrics are concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Han,
    Kana,
    Hangul,
    Latin,
}

/// The script a title asks for: language versions are often tagged "[CN]", "[JP]", "[KR]", "[EN]".
pub fn wanted_script(title: &str) -> Option<Script> {
    let t = title.to_uppercase();
    let tagged = |codes: &[&str]| {
        codes
            .iter()
            .any(|c| t.contains(&format!("[{c}]")) || t.contains(&format!("({c})")))
    };
    if tagged(&["CN", "ZH", "CHN", "CHINESE"]) {
        Some(Script::Han)
    } else if tagged(&["JP", "JA", "JPN", "JAPANESE"]) {
        Some(Script::Kana)
    } else if tagged(&["KR", "KO", "KOR", "KOREAN"]) {
        Some(Script::Hangul)
    } else if tagged(&["EN", "ENG", "ENGLISH"]) {
        Some(Script::Latin)
    } else {
        None
    }
}

/// The main script of some text. Japanese mixes kanji with kana, so any real amount of kana means Japanese.
pub fn script_of(text: &str) -> Option<Script> {
    let (mut han, mut kana, mut hangul, mut latin) = (0usize, 0usize, 0usize, 0usize);
    for c in text.chars() {
        match c as u32 {
            0x3040..=0x30FF => kana += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF => han += 1,
            _ if c.is_ascii_alphabetic() => latin += 1,
            _ => {}
        }
    }
    let total = han + kana + hangul + latin;
    if total == 0 {
        None
    } else if kana * 10 >= total {
        Some(Script::Kana)
    } else if hangul * 5 >= total {
        Some(Script::Hangul)
    } else if han * 5 >= total {
        Some(Script::Han)
    } else {
        Some(Script::Latin)
    }
}

/// Whether lyrics suit a title's language (always true when the title doesn't say).
pub fn fits(title: &str, text: &str) -> bool {
    match wanted_script(title) {
        None => true,
        Some(w) => script_of(text).is_none_or(|s| s == w),
    }
}

/// Ways to search for a title: as it is, and its Latin and non-Latin halves on their own
/// ("A Small Miracle 小小奇迹" is listed as just "A Small Miracle" in many places).
pub fn title_queries(title: &str) -> Vec<String> {
    let clean: String = title
        .split(['(', '[', '（', '【'])
        .next()
        .unwrap_or(title)
        .trim()
        .to_string();
    let latin: String = clean.chars().filter(|c| c.is_ascii() ).collect::<String>().trim().to_string();
    let other: String = clean.chars().filter(|c| !c.is_ascii()).collect::<String>().trim().to_string();
    let mut out = vec![clean.clone()];
    for q in [latin, other] {
        if q.chars().filter(|c| c.is_alphanumeric()).count() >= 2 && !out.contains(&q) {
            out.push(q);
        }
    }
    out
}

/// The song a search result has to match: its title, credits and length.
pub struct Wanted<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f64,
}

/// Names that mean the same artist online (a game's Chinese name and its English one, say).
const ALIASES: &[(&str, &str)] = &[("鸣潮", "wuthering waves"), ("鳴潮", "wuthering waves")];

fn names(credit: &str) -> Vec<String> {
    credit
        .split([',', '&', '、', '/', ';', '和'])
        .flat_map(|p| p.split(" feat").next())
        .map(|p| p.trim().to_lowercase())
        .filter(|p| p.chars().count() >= 2)
        .map(|p| {
            ALIASES
                .iter()
                .find(|(a, _)| *a == p)
                .map(|(_, b)| b.to_string())
                .unwrap_or(p)
        })
        .collect()
}

/// The first part of an album title ("Fleet Snowfluff - EP" → "fleet snowfluff").
fn album_core(album: &str) -> String {
    let first = album.split(" - ").next().unwrap_or(album);
    first.split(['(', '[']).next().unwrap_or(first).trim().to_lowercase()
}

/// Whether a search result is credited like our song: a shared artist or the same album.
/// Common titles ("Paper Plane") have many songs of the same length, so length alone isn't enough.
pub fn credits_match(w: &Wanted, h: &Hit) -> bool {
    let ours = names(w.artist);
    let theirs = names(&h.artist);
    let artist = ours.iter().any(|a| theirs.iter().any(|b| a == b || a.contains(b.as_str()) || b.contains(a.as_str())));
    let (a, b) = (album_core(w.album), album_core(&h.album));
    let album = a.chars().count() >= 4 && b.chars().count() >= 4 && (a.contains(&b) || b.contains(&a));
    artist || album
}

/// Finds a song on LRCLIB by searching its title and picking the result of the same length and credits.
/// Err when LRCLIB couldn't be asked properly (so "not found" isn't remembered).
pub fn search_best(w: &Wanted) -> Result<Option<Lyrics>, ()> {
    let title = w.title;
    let mut failed = false;
    for q in title_queries(title) {
        let Ok(hits) = search(&q) else {
            failed = true;
            continue;
        };
        if let Some(h) = best_hit(&hits, w) {
            match fetch(h.id) {
                Ok(l) => return Ok(Some(l)),
                Err(_) => failed = true,
            }
        }
    }
    if failed { Err(()) } else { Ok(None) }
}

/// The best search result for a song: close in length, same credits, in the title's language,
/// synced preferred.
pub fn best_hit<'a>(hits: &'a [Hit], w: &Wanted) -> Option<&'a Hit> {
    let duration = w.duration;
    hits.iter()
        .filter(|h| !h.instrumental && (duration <= 0.0 || (h.duration - duration).abs() <= 4.0))
        .filter(|h| fits(w.title, &h.preview) && credits_match(w, h))
        .max_by_key(|h| (h.synced, -((h.duration - duration).abs() * 10.0) as i64))
}

/// Asks LRCLIB (with a local cache). Err("missing") when it has nothing for this song,
/// Err("unavailable") when it couldn't be reached.
pub fn online(
    cache_dir: &Path,
    track_id: &str,
    artist: &str,
    title: &str,
    album: &str,
    duration: f64,
) -> Result<Lyrics, &'static str> {
    let cache_file: PathBuf = cache_dir.join(format!("{track_id}.json"));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(text) = std::fs::read_to_string(&cache_file) {
        if let Ok(c) = serde_json::from_str::<Cached>(&text) {
            let fresh = c.v >= LOOKUP_VERSION && now.saturating_sub(c.checked) < RETRY_MISSING_DAYS * 86_400;
            let suitable = c.lyrics.as_ref().is_some_and(|l| c.picked || fits(title, &l.plain));
            if suitable || (c.lyrics.is_none() && fresh) {
                return c.lyrics.ok_or("missing");
            }
        }
    }
    let url = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}&album_name={}&duration={}",
        urlencoding::encode(artist),
        urlencoding::encode(title),
        urlencoding::encode(album),
        duration.round() as u64
    );
    let client = crate::net::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!(
            "Cadence/",
            env!("CARGO_PKG_VERSION"),
            " (open-source music player)"
        ))
        .build()
        .map_err(|_| "unavailable")?;
    // Offline or LRCLIB down: don't cache, try again next time.
    let response = get(&client, &url).map_err(|_| "unavailable")?;
    let lyrics = if response.status().is_success() {
        let direct = response.json::<LrclibResponse>().ok().and_then(|r| {
            if r.instrumental {
                return Some(Lyrics {
                    source: "lrclib".into(),
                    instrumental: true,
                    ..Default::default()
                });
            }
            r.synced_lyrics
                .as_deref()
                .and_then(|t| Lyrics::from_text(t, "lrclib"))
                .or_else(|| {
                    r.plain_lyrics
                        .as_deref()
                        .and_then(|t| Lyrics::from_text(t, "lrclib"))
                })
        });
        match direct {
            Some(l) if l.instrumental || fits(title, &l.plain) => Some(l),
            // Another language version of the song: look for the right one.
            _ => search_best(&Wanted { title, artist, album, duration }).map_err(|_| "unavailable")?,
        }
    } else if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Credits often differ (e.g. a game's name instead of the singers): search by title.
        search_best(&Wanted { title, artist, album, duration }).map_err(|_| "unavailable")?
    } else {
        log::info!("lyrics lookup: LRCLIB answered {}", response.status());
        return Err("unavailable");
    };
    let _ = std::fs::create_dir_all(cache_dir);
    let _ = std::fs::write(
        &cache_file,
        serde_json::to_vec(&Cached {
            lyrics: lyrics.clone(),
            checked: now,
            picked: false,
            v: LOOKUP_VERSION,
            hidden: false,
        })
        .unwrap_or_default(),
    );
    lyrics.ok_or("missing")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(l: &Lyrics) -> Vec<(String, Vec<u8>)> {
        l.synced.iter().map(|x| (x.text.clone(), x.singers.clone())).collect()
    }

    #[test]
    fn finds_singers_named_in_the_lyrics() {
        // Names before a colon, held until the next one; "Both" is everyone.
        let l = Lyrics::from_text(
            "[00:01.00]Cassie: The magical potion\n[00:03.00]Of reanimation\n[00:05.00]Yamato: So I can see you\n[00:07.00]Both: Eat up\n[00:09.00]Cassie: Fill your stomach\n[00:11.00]Yamato: again",
            "file",
        )
        .unwrap();
        assert_eq!(l.singers, ["Cassie", "Yamato"]);
        assert_eq!(l.singers_from, "lyrics");
        assert_eq!(
            who(&l),
            [
                ("The magical potion".into(), vec![0]),
                ("Of reanimation".into(), vec![0]),
                ("So I can see you".into(), vec![1]),
                ("Eat up".into(), vec![0, 1]),
                ("Fill your stomach".into(), vec![0]),
                ("again".into(), vec![1]),
            ]
        );
        // Lines that only name the singer, timed or not, and "A & B" for both.
        let l = Lyrics::from_text(
            "[Ayaka]\n[00:01.00]First line\n[00:02.00]Second line\n[00:03.00][Ken]\n[00:04.00]Third line\n[00:05.00][Ayaka & Ken] Fourth",
            "file",
        )
        .unwrap();
        assert_eq!(l.singers, ["Ayaka", "Ken"]);
        assert_eq!(
            who(&l),
            [("First line".into(), vec![0]), ("Second line".into(), vec![0]), ("Third line".into(), vec![1]), ("Fourth".into(), vec![0, 1])]
        );
        // Karaoke's M / F / D.
        let l = Lyrics::from_text("[00:01.00]M: one\n[00:02.00]F: two\n[00:03.00]D: three\n[00:04.00]M: four", "file").unwrap();
        assert_eq!(l.singers, ["Male", "Female"]);
        assert_eq!(l.synced[2].singers, [0, 1]);
        // Word-timed lines lose the name from their first word too.
        let l = Lyrics::from_text("[00:01.00]<00:01.00>Mia: <00:01.50>hello\n[00:02.00]Leo: bye\n[00:03.00]Mia: again\n[00:04.00]Leo: end", "file").unwrap();
        assert_eq!(l.synced[0].text, "hello");
        assert_eq!(l.synced[0].words.len(), 1);
    }

    #[test]
    fn ordinary_lyrics_have_no_singers() {
        for text in [
            // Backing vocals in parentheses, song parts, a colon in a line.
            "[00:01.00](Eat up) Fill your stomach\n[00:02.00](Eat up) Eat it all\n[00:03.00](Oh oh) yeah",
            "[Verse 1]\n[00:01.00]One\n[Chorus]\n[00:02.00]Two\n[Pre-Chorus]\n[00:03.00]Three",
            "[00:01.00]Remember this: love is all\n[00:02.00]Listen: the night\n[00:03.00]Something else",
            "[ti:Song]\n[ar:Someone]\n[00:01.00]Hello\n[00:02.00]World",
        ] {
            let l = Lyrics::from_text(text, "file").unwrap();
            assert!(l.singers.is_empty(), "{text}: {:?}", l.singers);
            assert!(l.synced.iter().all(|x| x.singers.is_empty()));
        }
        // Their text is left as it was.
        let l = Lyrics::from_text("[00:01.00](Eat up) Fill your stomach", "file").unwrap();
        assert_eq!(l.synced[0].text, "(Eat up) Fill your stomach");
    }

    #[test]
    fn parses_lrc() {
        let l = parse_lrc(
            "[ti:Song]\n[00:01.50]Hello\n[00:03.00][00:10.00]Again\n[offset:500]\n[01:02.25] Bye",
        );
        assert_eq!(l.len(), 4);
        assert_eq!(
            l[0],
            Line {
                time: 1.0,
                text: "Hello".into(),
                words: Vec::new(),
                singers: Vec::new()
            }
        );
        assert_eq!(l[1].text, "Again");
        assert!((l[3].time - 61.75).abs() < 1e-9);
    }

    #[test]
    fn plain_text_is_not_synced() {
        let l = Lyrics::from_text("Just words\nno times", "embedded").unwrap();
        assert!(l.synced.is_empty());
        assert_eq!(l.plain, "Just words\nno times");
    }

    #[test]
    fn parses_word_timings() {
        let l = parse_lrc("[00:01.00]<00:01.00>Hello <00:01.50>world<00:02.20>");
        assert_eq!(l[0].text, "Hello world");
        assert_eq!(l[0].words.len(), 2);
        assert_eq!(l[0].words[1].time, 1.5);
        assert_eq!(l[0].words[0].text, "Hello ");
    }

    #[test]
    fn title_variants() {
        assert_eq!(
            title_queries("A Small Miracle 小小奇迹 (Instrumental)"),
            vec!["A Small Miracle 小小奇迹", "A Small Miracle", "小小奇迹"]
        );
        assert_eq!(title_queries("Hello"), vec!["Hello"]);
    }

    #[test]
    fn language_versions() {
        assert_eq!(wanted_script("Song (feat. A [CN])"), Some(Script::Han));
        assert_eq!(wanted_script("Song (feat. B [JP])"), Some(Script::Kana));
        assert_eq!(wanted_script("Song (feat. 치사 [KR])"), Some(Script::Hangul));
        assert_eq!(wanted_script("Song"), None);
        assert_eq!(script_of("I've been here before"), Some(Script::Latin));
        assert_eq!(script_of("夏の灯りが消えないように"), Some(Script::Kana));
        assert_eq!(script_of("不散的夏之灯"), Some(Script::Han));
        assert_eq!(script_of("여름의 등불"), Some(Script::Hangul));
        assert!(!fits("Song [CN]", "I've been here before"));
        assert!(fits("Song", "I've been here before"));
    }

    #[test]
    fn credits_must_match() {
        let hit = |artist: &str, album: &str| Hit {
            id: 1,
            title: "Paper Plane".into(),
            artist: artist.into(),
            album: album.into(),
            duration: 233.0,
            instrumental: false,
            synced: true,
            preview: String::new(),
        };
        let w = Wanted { title: "Paper Plane 纸飞机", artist: "鳴潮 & 飞行雪绒", album: "Fleet Snowfluff - EP", duration: 233.5 };
        assert!(!credits_match(&w, &hit("Some Pop Band", "Summer")), "same title and length isn't enough");
        assert!(credits_match(&w, &hit("Wuthering Waves", "Wuthering Waves")), "game alias");
        assert!(credits_match(&w, &hit("Someone", "Fleet Snowfluff")), "same album");
    }
}
