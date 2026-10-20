//! The AI DJ: a chat model (Z.ai GLM by default, any OpenAI-compatible service works) picks songs
//! from your library ("Ask the DJ" and daily mixes), suggests new music and finds lyrics.
//!
//! The model never sees the whole library: Cadence sends a short list (at most `SHORTLIST` songs,
//! each with a few words about how it sounds from the local sound analysis) and a short summary
//! of your listening, then checks every id it gets back. With web search on, GLM can look things
//! up (new releases, lyrics); services that don't support it are asked again without.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::history::HistoryStats;
use crate::library::Track;

pub const KEY_SECRET: &str = "ai-key";
const SHORTLIST: usize = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// An online OpenAI-compatible service (needs an API key).
    Cloud,
    /// Also what older settings with a provider that no longer exists (Ollama) become.
    #[default]
    #[serde(other)]
    Off,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    pub provider: Provider,
    pub cloud_url: String,
    pub cloud_model: String,
    /// Let the model search the web (new music, lyrics).
    pub web_search: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            provider: Provider::Off,
            web_search: true,
            // Z.ai's GLM Coding Plan (as the old Cadence used); any OpenAI-compatible service works.
            cloud_url: "https://api.z.ai/api/coding/paas/v4".into(),
            cloud_model: "glm-5.3-flash".into(),
        }
    }
}

struct Endpoint {
    url: String,
    model: String,
    key: Option<String>,
}

fn endpoint(s: &AiSettings) -> Result<Endpoint, String> {
    match s.provider {
        Provider::Off => Err("The AI DJ is off. Turn it on in Settings.".into()),
        Provider::Cloud => {
            let key = crate::secrets::get(KEY_SECRET)
                .ok_or("Add your API key in Settings to use the online AI DJ.")?;
            Ok(Endpoint {
                url: s.cloud_url.trim_end_matches('/').to_string(),
                model: s.cloud_model.clone(),
                key: Some(key),
            })
        }
    }
}

fn client(timeout: u64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

pub const SYSTEM: &str = "You are the DJ inside Cadence, a personal music player. You pick songs only from \
the list you are given and answer with strict JSON only: no markdown, no commentary.";

/// Sends one chat request and returns the JSON object in the answer. `search`: allow web search.
fn chat_json(s: &AiSettings, prompt: &str, search: bool) -> Result<Value, String> {
    let ep = endpoint(s)?;
    // Web search makes answers much slower (several searches before the reply).
    let http = client(if search && s.web_search { 300 } else { 120 });
    let mut body = json!({
        "model": ep.model,
        "temperature": 0.7,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": prompt },
        ],
        "response_format": { "type": "json_object" },
    });
    if search && s.web_search {
        // Z.ai's built-in web search tool.
        body["tools"] = json!([{
            "type": "web_search",
            "web_search": { "enable": true, "search_engine": "search-prime", "search_result": true, "count": 6 }
        }]);
    }
    let send = |body: &Value| {
        let mut req = http.post(format!("{}/chat/completions", ep.url)).json(body);
        if let Some(k) = &ep.key {
            req = req.bearer_auth(k);
        }
        req.send()
    };
    let mut r = send(&body).map_err(|e| unreachable_message(s, &e))?;
    // Some services (or plans) don't support web search or JSON mode; ask again without them.
    if r.status() == reqwest::StatusCode::BAD_REQUEST {
        if let Some(o) = body.as_object_mut() {
            if o.remove("tools").is_some() {
                log::info!("AI service refused web search; asking without it");
            } else {
                o.remove("response_format");
            }
        }
        r = send(&body).map_err(|e| unreachable_message(s, &e))?;
        if r.status() == reqwest::StatusCode::BAD_REQUEST {
            body.as_object_mut().map(|o| o.remove("response_format"));
            r = send(&body).map_err(|e| unreachable_message(s, &e))?;
        }
    }
    let status = r.status();
    let v: Value = r.json().map_err(|e| format!("Unexpected answer from the AI service: {e}"))?;
    if !status.is_success() {
        let msg = v["error"]["message"]
            .as_str()
            .or(v["error"].as_str())
            .unwrap_or("no details");
        return Err(match status.as_u16() {
            401 | 403 => "The AI service didn’t accept the API key. Check it in Settings.".into(),
            404 => format!("The AI service doesn’t know the model “{}”. Check it in Settings.", ep.model),
            429 => "The AI service is busy or your free limit is used up. Try again later.".into(),
            _ => format!("The AI service answered {status}: {msg}"),
        });
    }
    let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    extract_json(text).ok_or_else(|| "The AI answer wasn’t in the expected format. Try again.".into())
}

fn unreachable_message(_s: &AiSettings, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "The AI took too long to answer. Try again.".into()
    } else {
        format!("Can’t reach the AI service: {e}")
    }
}

/// Finds the JSON object in a model's answer, skipping any "thinking" and code fences.
fn extract_json(text: &str) -> Option<Value> {
    let text = match text.rfind("</think>") {
        Some(i) => &text[i + 8..],
        None => text,
    };
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// One song as the model sees it.
pub struct Candidate {
    pub track: Track,
    /// A few words about its sound ("fast, bright, minor"), if analysed.
    pub sound: Option<String>,
}

/// A summary of the listener's habits, in plain words.
pub fn taste<'a>(track: impl Fn(&str) -> Option<&'a Track>, h: &HistoryStats) -> String {
    let mut artists: HashMap<&'a str, (u32, u32)> = HashMap::new();
    for (id, n) in &h.plays {
        if let Some(t) = track(id) {
            artists.entry(t.artist.as_str()).or_default().0 += n;
        }
    }
    for (id, n) in &h.skips {
        if let Some(t) = track(id) {
            artists.entry(t.artist.as_str()).or_default().1 += n;
        }
    }
    let mut liked: Vec<_> = artists.iter().filter(|(_, (p, _))| *p > 0).collect();
    liked.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    let skipped: Vec<_> = artists
        .iter()
        .filter(|(_, (p, s))| *s >= 3 && s > p)
        .map(|(a, _)| *a)
        .take(5)
        .collect();
    let mut out = String::new();
    if liked.is_empty() {
        out.push_str("No listening history yet.\n");
    } else {
        let names: Vec<_> = liked.iter().take(8).map(|(a, (p, _))| format!("{a} ({p} plays)")).collect();
        out.push_str(&format!("Most played artists: {}.\n", names.join(", ")));
    }
    if !skipped.is_empty() {
        out.push_str(&format!("Often skipped: {}.\n", skipped.join(", ")));
    }
    out
}

/// Chooses up to `SHORTLIST` songs to show the model: songs matching the request's words first,
/// then favourites, then the rest spread across albums (so one big album can't fill the list).
pub fn shortlist(all: Vec<Candidate>, request: &str, h: &HistoryStats) -> Vec<Candidate> {
    if all.len() <= SHORTLIST {
        return all;
    }
    let words: Vec<String> = request
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(String::from)
        .collect();
    let score = |c: &Candidate| {
        let t = &c.track;
        let hay = format!("{} {} {} {} {}", t.title, t.artist, t.album, t.genre, c.sound.as_deref().unwrap_or(""))
            .to_lowercase();
        let hits = words.iter().filter(|w| hay.contains(w.as_str())).count() as f64;
        let plays = *h.plays.get(&t.id).unwrap_or(&0) as f64;
        let skips = *h.skips.get(&t.id).unwrap_or(&0) as f64;
        hits * 10.0 + (plays - 0.5 * skips).clamp(-3.0, 10.0) * 0.3
    };
    let mut scored: Vec<(f64, Candidate)> = all.into_iter().map(|c| (score(&c), c)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (top, rest): (Vec<_>, Vec<_>) = scored.into_iter().enumerate().partition(|(i, (s, _))| *i < SHORTLIST / 2 && *s > 0.0);
    let mut out: Vec<Candidate> = top.into_iter().map(|(_, (_, c))| c).collect();
    // Round-robin over albums for the rest.
    let mut by_album: Vec<(String, std::collections::VecDeque<Candidate>)> = Vec::new();
    for (_, (_, c)) in rest {
        match by_album.iter_mut().find(|(a, _)| *a == c.track.album_id) {
            Some((_, v)) => v.push_back(c),
            None => by_album.push((c.track.album_id.clone(), [c].into())),
        }
    }
    // Vary which albums come first from day to day.
    if !by_album.is_empty() {
        let k = today() as usize % by_album.len();
        by_album.rotate_left(k);
    }
    while out.len() < SHORTLIST {
        let mut any = false;
        for (_, v) in by_album.iter_mut() {
            if out.len() < SHORTLIST {
                if let Some(c) = v.pop_front() {
                    out.push(c);
                    any = true;
                }
            }
        }
        if !any {
            break;
        }
    }
    out
}

/// The song list as the model sees it: `id|title|artist|album|genre|year|plays|sound`.
fn catalog(c: &[Candidate], h: &HistoryStats) -> String {
    let clean = |s: &str| s.replace(['|', '\n'], " ");
    c.iter()
        .enumerate()
        .map(|(i, c)| {
            let t = &c.track;
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                i + 1,
                clean(&t.title),
                clean(&t.artist),
                clean(&t.album),
                clean(&t.genre),
                t.year.map(|y| y.to_string()).unwrap_or_default(),
                h.plays.get(&t.id).unwrap_or(&0),
                c.sound.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the model is told, so it can be shown in Settings before anything is sent.
pub fn ask_prompt(request: &str, taste: &str, c: &[Candidate], h: &HistoryStats) -> String {
    format!(
        "Now: {now}.\nLISTENER:\n{taste}\nSONGS (id|title|artist|album|genre|year|plays|sound):\n{list}\n\n\
The listener asks: \"{req}\"\n\
Pick 8–25 songs from SONGS that fit the request (use the listener's taste when the request is vague). \
Order them so they flow. title: at most 5 words. reason: one short sentence.\n\
Also suggest up to 6 real songs that are NOT in SONGS and fit the request and the listener's taste \
(search the web for recent releases if you can). why: at most 12 words.\n\
Return JSON only: {{\"title\":\"\",\"reason\":\"\",\"ids\":[1,2,3],\"discover\":[{{\"artist\":\"\",\"title\":\"\",\"why\":\"\"}}]}}",
        now = now_words(),
        list = catalog(c, h),
        req = request.replace('"', "'"),
    )
}

fn mixes_prompt(taste: &str, c: &[Candidate], h: &HistoryStats) -> String {
    format!(
        "Now: {now}.\nLISTENER:\n{taste}\nSONGS (id|title|artist|album|genre|year|plays|sound):\n{list}\n\n\
Make 3 mixes for today from SONGS, each 6–20 songs with a clearly different mood or moment. \
Mix favourites with songs played rarely or never. Order each so it flows. \
title: at most 4 words. reason: one short sentence.\n\
Return JSON only: {{\"mixes\":[{{\"title\":\"\",\"reason\":\"\",\"ids\":[1,2,3]}}]}}",
        now = now_words(),
        list = catalog(c, h),
    )
}

fn now_words() -> String {
    // Local time of day matters more than the date ("late night", "morning").
    let now = local_hour();
    let part = match now {
        5..=11 => "morning",
        12..=16 => "afternoon",
        17..=21 => "evening",
        _ => "late night",
    };
    format!("{part} ({now}:00)")
}

/// The local hour, from Windows' local time.
fn local_hour() -> u32 {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            dow: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            ms: u16,
        }
        unsafe extern "system" {
            fn GetLocalTime(t: *mut SystemTime);
        }
        let mut t = SystemTime::default();
        unsafe { GetLocalTime(&mut t) };
        t.hour as u32
    }
    #[cfg(not(windows))]
    {
        ((crate::history::now() / 3600) % 24) as u32
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMix {
    pub title: String,
    pub reason: String,
    pub track_ids: Vec<String>,
    /// Songs not in the library the DJ thinks fit.
    #[serde(default)]
    pub discover: Vec<Discovery>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Discovery {
    pub artist: String,
    pub title: String,
    pub why: String,
    /// Album it's on, from the catalogue.
    pub album: String,
    /// Album cover, or the artist's picture when `image_is_artist` (empty if unknown).
    pub image: String,
    pub image_is_artist: bool,
    /// Already looked up in the catalogue.
    pub checked: bool,
}

fn to_mix(v: &Value, c: &[Candidate]) -> AiMix {
    let mut seen = HashSet::new();
    let track_ids = v["ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
        .filter_map(|n| c.get((n as usize).checked_sub(1)?))
        .map(|c| c.track.id.clone())
        .filter(|id| seen.insert(id.clone()))
        .collect();
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let discover = v["discover"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| Discovery {
            artist: text(d, "artist"),
            title: text(d, "title"),
            why: text(d, "why"),
            ..Default::default()
        })
        .filter(|d| !d.artist.is_empty() && !d.title.is_empty())
        .take(6)
        .collect();
    AiMix {
        title: text(v, "title"),
        reason: text(v, "reason"),
        track_ids,
        discover,
    }
}

pub fn ask(s: &AiSettings, request: &str, taste: &str, c: &[Candidate], h: &HistoryStats) -> Result<AiMix, String> {
    let v = chat_json(s, &ask_prompt(request, taste, c, h), true)?;
    let mut mix = to_mix(&v, c);
    if mix.track_ids.is_empty() {
        return Err("The DJ didn’t find songs for that. Try asking another way.".into());
    }
    if mix.title.is_empty() {
        mix.title = request.chars().take(40).collect();
    }
    Ok(mix)
}

pub fn daily_mixes(s: &AiSettings, taste: &str, c: &[Candidate], h: &HistoryStats) -> Result<Vec<AiMix>, String> {
    let v = chat_json(s, &mixes_prompt(taste, c, h), false)?;
    let mixes: Vec<AiMix> = v["mixes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| to_mix(m, c))
        .filter(|m| m.track_ids.len() >= 3 && !m.title.is_empty())
        .collect();
    if mixes.is_empty() {
        return Err("The DJ couldn’t make mixes this time. Try again.".into());
    }
    Ok(mixes)
}

/// A quick check that the key, server and model work.
pub fn test(s: &AiSettings) -> Result<(), String> {
    let v = chat_json(s, "Reply with this JSON only: {\"ok\":true}", false)?;
    if v["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err("The AI answered, but not as expected. Check the model name.".into())
    }
}

/// What the AI found for a song's lyrics.
pub struct FoundLyrics {
    /// Other ways to search for the song (other languages or spellings of the title and artist).
    pub queries: Vec<String>,
    /// Time-synced LRC if it found one.
    pub lrc: String,
    /// Plain lyrics, line by line.
    pub plain: String,
    pub instrumental: bool,
}

/// Asks the AI (with web search) to find a song's lyrics.
pub fn find_lyrics(s: &AiSettings, t: &Track) -> Result<FoundLyrics, String> {
    let prompt = format!(
        "Find the lyrics of this song. Search the web.\n\
Title: {title}\nArtist: {artist}\nAlbum: {album}\nLength: {len}\n\n\
Return JSON only: {{\"queries\":[\"\"],\"lrc\":\"\",\"lyrics\":\"\",\"instrumental\":false}}\n\
queries: up to 4 short searches (title and main artist) that a lyrics database would match, \
including the title in other languages or spellings used for this song.\n\
lrc: time-synced lyrics in LRC format ([mm:ss.xx] line) only if you found real timings, else \"\".\n\
lyrics: the complete original lyrics, one line per line, exactly as published; \"\" if you can't find them. \
Never invent or translate lyrics.\ninstrumental: true if the song has no vocals.",
        title = t.title,
        artist = t.artist,
        album = t.album,
        len = format!("{}:{:02}", (t.duration / 60.0) as u32, (t.duration % 60.0) as u32),
    );
    let v = chat_json(s, &prompt, true)?;
    let text = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    Ok(FoundLyrics {
        queries: v["queries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|q| q.as_str().map(|q| q.trim().to_string()))
            .filter(|q| !q.is_empty())
            .take(4)
            .collect(),
        lrc: text("lrc"),
        plain: text("lyrics"),
        instrumental: v["instrumental"].as_bool() == Some(true),
    })
}

/// Songs the listener doesn't have yet, picked from their listening (taste, most played songs)
/// with web search for recent releases.
pub fn discover(s: &AiSettings, taste: &str, c: &[Candidate], owned_artists: &[String]) -> Result<Vec<Discovery>, String> {
    let favourites: Vec<String> = c
        .iter()
        .take(25)
        .map(|c| format!("{} - {}", c.track.artist, c.track.title))
        .collect();
    let artists: Vec<&str> = owned_artists.iter().take(80).map(String::as_str).collect();
    let prompt = format!(
        "Now: {now}.\nLISTENER:\n{taste}\nSome songs they have and play:\n{favs}\n\
Artists already in their library: {artists}\n\n\
Suggest 12 real songs NOT in their library that this listener would love. Search the web for recent \
releases (this year and last) and include some. Mix: about a third by artists they already like, the \
rest by artists they don't have yet. Only real, existing songs. why: at most 12 words, refer to their taste.\n\
Return JSON only: {{\"picks\":[{{\"artist\":\"\",\"title\":\"\",\"why\":\"\"}}]}}",
        now = now_words(),
        favs = favourites.join("\n"),
        artists = artists.join(", "),
    );
    let v = chat_json(s, &prompt, true)?;
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let picks: Vec<Discovery> = v["picks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| Discovery {
            artist: text(d, "artist"),
            title: text(d, "title"),
            why: text(d, "why"),
            ..Default::default()
        })
        .filter(|d| !d.artist.is_empty() && !d.title.is_empty())
        .take(15)
        .collect();
    if picks.is_empty() {
        return Err("The DJ couldn’t find new music this time. Try again.".into());
    }
    Ok(picks)
}

#[derive(Default, Serialize, Deserialize)]
pub struct DiscoverCache {
    pub day: u64,
    pub picks: Vec<Discovery>,
    /// Why today's attempt failed; it isn't retried automatically until tomorrow.
    #[serde(default)]
    pub error: String,
}

/// Today's mixes, kept so they're only made once a day.
#[derive(Default, Serialize, Deserialize)]
pub struct DailyCache {
    pub day: u64,
    pub mixes: Vec<AiMix>,
    /// Why today's attempt failed; it isn't retried automatically until tomorrow.
    #[serde(default)]
    pub error: String,
}

pub fn today() -> u64 {
    // Days in local time would be nicer; UTC days change at 07:00 in Jakarta, close enough to morning.
    crate::history::now() / 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_json_after_thinking() {
        let v = extract_json("<think>maybe {a}</think>\n```json\n{\"ids\":[1,2]}\n```").unwrap();
        assert_eq!(v["ids"][1], 2);
        assert!(extract_json("no json here").is_none());
    }
}
