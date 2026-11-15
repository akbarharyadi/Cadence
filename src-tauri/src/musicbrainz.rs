//! Learning the other names of the library's artists from MusicBrainz (米津玄師 is also
//! Kenshi Yonezu, よねづけんし and 켄시 요네즈), for `identity`.
//!
//! One background job, one request at a time, at most one every 1.1 s (MusicBrainz allows one a
//! second per address), backing off when it's busy (503, honouring Retry-After). Only artist
//! names are sent, with a User-Agent naming Cadence, its version and the project's public page.
//! Each name is asked about once; what was found (or that nothing safe was) is kept in
//! musicbrainz-artists.json, a cache that can always be built again (it isn't in backups). Names
//! without a safe match are asked again after a month.
//!
//! A match is only taken when it's safe: a high search score, one of the artist's names or
//! aliases written like the library's name, and no other MusicBrainz artist matching as well.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ctx::Ctx;
use crate::identity::{fold, unsort};

/// Between two requests (MusicBrainz allows one a second; a little under it).
pub const GAP_MS: u64 = 1_100;
/// Asked again after this long when no safe match was found.
pub const RETRY_UNMATCHED: u64 = 30 * 86_400;
/// Asked again after this long when MusicBrainz refused the question (not when it was busy).
pub const RETRY_FAILED: u64 = 7 * 86_400;
/// A search score at least this high (out of 100) can be a match.
const MIN_SCORE: u64 = 90;
/// Busy answers in a row before the job gives up for now.
const MAX_BUSY: u32 = 5;
/// The cache is saved (and the artists rebuilt) every this many names.
const SAVE_EVERY: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MbAlias {
    pub name: String,
    pub sort_name: Option<String>,
    pub locale: Option<String>,
    pub primary: bool,
    /// "Artist name", "Legal name", "Search hint"…
    pub kind: Option<String>,
}

/// A MusicBrainz artist, as kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MbArtist {
    pub mbid: String,
    /// As MusicBrainz writes it (usually in the artist's own script: 米津玄師).
    pub name: String,
    /// "Yonezu, Kenshi".
    pub sort_name: String,
    pub country: Option<String>,
    pub aliases: Vec<MbAlias>,
}

impl MbArtist {
    /// Every name the artist goes by (search hints only with `hints`).
    pub fn names(&self, hints: bool) -> Vec<String> {
        let mut out = vec![self.name.clone(), self.sort_name.clone()];
        out.extend(unsort(&self.sort_name));
        for a in &self.aliases {
            if !hints && a.kind.as_deref() == Some("Search hint") {
                continue;
            }
            out.push(a.name.clone());
            if let Some(s) = &a.sort_name {
                out.extend(unsort(s));
            }
        }
        out.retain(|n| !n.trim().is_empty());
        out
    }

    /// The search hints (misspellings and the like): for search only, never for joining names.
    pub fn hints(&self) -> Vec<String> {
        self.aliases.iter().filter(|a| a.kind.as_deref() == Some("Search hint")).map(|a| a.name.clone()).collect()
    }

    /// The name in the artist's own script and the one in Latin letters, when MusicBrainz has them.
    pub fn native_and_latin(&self) -> (Option<String>, Option<String>) {
        let cjk = crate::identity::cjk;
        let native = if cjk(&self.name) { Some(self.name.clone()) } else { self.aliases.iter().find(|a| cjk(&a.name)).map(|a| a.name.clone()) };
        let latin = if !cjk(&self.name) {
            Some(self.name.clone())
        } else {
            self.aliases
                .iter()
                .filter(|a| !cjk(&a.name) && a.kind.as_deref() != Some("Search hint"))
                .max_by_key(|a| (a.locale.as_deref() == Some("en"), a.primary))
                .map(|a| a.name.clone())
                .or_else(|| unsort(&self.sort_name))
        };
        (native, latin)
    }
}

/// What was asked about a name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    /// Unix seconds.
    pub asked: u64,
    /// No safe match: asked again from then (unix seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found: Option<MbArtist>,
}

impl Entry {
    pub fn due(&self, now: u64) -> bool {
        self.found.is_none() && self.retry.is_none_or(|r| now >= r)
    }
}

/// musicbrainz-artists.json: by the library name's `fold`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Cache {
    pub names: HashMap<String, Entry>,
    /// The user paused the job: it doesn't start by itself until resumed.
    pub paused: bool,
}

fn cache_file(dir: &Path) -> PathBuf {
    dir.join("musicbrainz-artists.json")
}

/// The cache, read once (kept in memory while Cadence runs).
static CACHE: Mutex<Option<(PathBuf, Cache)>> = Mutex::new(None);

fn with_cache<R>(dir: &Path, f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut c = CACHE.lock();
    let file = cache_file(dir);
    if c.as_ref().is_none_or(|(p, _)| *p != file) {
        *c = Some((file.clone(), crate::state::read_json(&file)));
    }
    f(&mut c.as_mut().expect("cache").1)
}

fn save_cache(dir: &Path) {
    let copy = with_cache(dir, |c| c.clone());
    // A cache: no .bak of it.
    crate::state::write_json_with(&cache_file(dir), &copy, false);
}

/// Each library name MusicBrainz safely knows, by `fold`.
pub fn accepted(dir: &Path) -> HashMap<String, MbArtist> {
    with_cache(dir, |c| c.names.iter().filter_map(|(k, e)| e.found.clone().map(|f| (k.clone(), f))).collect())
}

// ---------- the question and the answer ----------

/// The characters Lucene treats specially, escaped.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The search for an artist by any of its names.
pub fn url(name: &str) -> String {
    let e = escape(name.trim());
    let query = format!("alias:\"{e}\" OR artist:\"{e}\" OR sortname:\"{e}\"");
    format!("https://musicbrainz.org/ws/2/artist/?query={}&fmt=json&limit=10", urlencoding::encode(&query))
}

fn score(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}

fn artist_of(a: &Value) -> Option<MbArtist> {
    let s = |v: &Value| v.as_str().map(str::to_string);
    Some(MbArtist {
        mbid: s(&a["id"])?,
        name: s(&a["name"])?,
        sort_name: s(&a["sort-name"]).unwrap_or_default(),
        country: s(&a["country"]),
        aliases: a["aliases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| {
                Some(MbAlias {
                    name: s(&x["name"])?,
                    sort_name: s(&x["sort-name"]),
                    locale: s(&x["locale"]),
                    primary: x["primary"].as_bool().unwrap_or(false),
                    kind: s(&x["type"]),
                })
            })
            .collect(),
    })
}

/// A name this short (in letters; a Chinese, Japanese or Korean name in characters, which say
/// more each) is shared by too many artists to be taken on the search alone.
fn too_short(folded: &str) -> bool {
    let n = folded.chars().count();
    if crate::identity::cjk(folded) { n <= 1 } else { n <= 3 }
}

/// The one MusicBrainz artist that is safely `name` (`pick_with` without anything local to go on).
#[cfg(test)]
pub fn pick(name: &str, body: &str) -> Option<MbArtist> {
    pick_with(name, body, &HashSet::new())
}

/// The one MusicBrainz artist that is safely `name`: one of its names (not a search hint) written
/// like `name`, a score of 90 or more, and clearly ahead of every other artist with such a name
/// (whatever their score: two artists called the same, at 100 and 85, are still two). A name that
/// is very short, or that several MusicBrainz artists have, is taken only when something local
/// agrees: another of the artist's names is also in the library (`library`: the library's names,
/// folded). None also when the answer can't be read.
pub fn pick_with(name: &str, body: &str, library: &HashSet<String>) -> Option<MbArtist> {
    let v: Value = serde_json::from_str(body).ok()?;
    let want = fold(name);
    if want.is_empty() {
        return None;
    }
    // Every artist written like the name, whatever its score; each artist once (its best score).
    let mut matching: Vec<(u64, MbArtist)> = v["artists"]
        .as_array()?
        .iter()
        .filter_map(|a| Some((score(&a["score"]), artist_of(a)?)))
        .filter(|(_, a)| a.names(false).iter().any(|n| fold(n) == want))
        .collect();
    matching.sort_by(|a, b| b.0.cmp(&a.0));
    let mut ids = HashSet::new();
    matching.retain(|(_, a)| ids.insert(a.mbid.clone()));
    let corroborated = |a: &MbArtist| a.names(false).iter().map(|n| fold(n)).any(|f| !f.is_empty() && f != want && library.contains(&f));
    let (top, one) = matching.first()?;
    if *top < MIN_SCORE {
        return None;
    }
    if let Some((next, _)) = matching.get(1)
        && *top < next + 10
    {
        // Another artist of that name nearly as good: can't tell which.
        return None;
    }
    let shared = matching.len() > 1;
    if (shared || too_short(&want)) && !(corroborated(one) && !matching.iter().skip(1).any(|(_, a)| corroborated(a))) {
        return None;
    }
    Some(one.clone())
}

// ---------- the job, with its network and clock as parameters (tests use fakes) ----------

/// How a request went.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Body(String),
    /// 503 / 429: wait (this many seconds, when MusicBrainz says) and ask again.
    Busy(Option<u64>),
    /// Any other status: this name is asked again in a week.
    Failed(u16),
    /// No answer at all (no internet): the job stops quietly.
    Offline,
}

pub trait Net {
    fn get(&mut self, url: &str) -> Reply;
}

pub trait Clock {
    /// Milliseconds since some start.
    fn now_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
    /// Unix seconds.
    fn unix(&self) -> u64;
}

/// One request every `GAP_MS` at most.
#[derive(Default)]
pub struct Pacer {
    last: Option<u64>,
}

impl Pacer {
    pub fn wait(&mut self, clock: &mut dyn Clock) {
        if let Some(last) = self.last {
            let next = last + GAP_MS;
            let now = clock.now_ms();
            if now < next {
                clock.sleep_ms(next - now);
            }
        }
        self.last = Some(clock.now_ms());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Done,
    Stopped,
    Offline,
    /// MusicBrainz stayed busy: tried again next time.
    Busy,
}

/// Asks about each of `names` in turn, filing the answers in `cache`. `go(i)` before each name
/// (false: stop); `saved(cache, done)` every `SAVE_EVERY` names and at the end. `library`: every
/// library artist name, folded (what corroborates a short or shared name, `pick_with`).
pub fn run(names: &[String], library: &HashSet<String>, net: &mut dyn Net, clock: &mut dyn Clock, cache: &mut Cache, mut go: impl FnMut(usize) -> bool, mut saved: impl FnMut(&Cache, usize)) -> Outcome {
    let mut pacer = Pacer::default();
    let mut busy = 0u32;
    let mut outcome = Outcome::Done;
    let mut done = 0;
    'names: for (i, name) in names.iter().enumerate() {
        if !go(i) {
            outcome = Outcome::Stopped;
            break;
        }
        let key = fold(name);
        loop {
            pacer.wait(clock);
            let entry = match net.get(&url(name)) {
                Reply::Body(body) => {
                    busy = 0;
                    let found = pick_with(name, &body, library);
                    let now = clock.unix();
                    Entry { asked: now, retry: found.is_none().then_some(now + RETRY_UNMATCHED), found }
                }
                Reply::Busy(after) => {
                    busy += 1;
                    if busy >= MAX_BUSY {
                        outcome = Outcome::Busy;
                        break 'names;
                    }
                    let secs = after.unwrap_or(1 << busy).clamp(1, 120);
                    clock.sleep_ms(secs * 1000);
                    continue;
                }
                Reply::Failed(code) => {
                    log::info!("MusicBrainz said {code} for an artist name; asked again later");
                    let now = clock.unix();
                    Entry { asked: now, retry: Some(now + RETRY_FAILED), found: None }
                }
                Reply::Offline => {
                    outcome = Outcome::Offline;
                    break 'names;
                }
            };
            cache.names.insert(key, entry);
            done = i + 1;
            if done % SAVE_EVERY == 0 {
                saved(cache, done);
            }
            break;
        }
    }
    saved(cache, done);
    outcome
}

/// The names still to ask about: library names (not compilations' "Various Artists") without an
/// answer, or whose retry date has come.
pub fn todo(names: &[(String, u32)], cache: &Cache, now: u64) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<(u32, String)> = names
        .iter()
        .filter(|(n, _)| !crate::api::no_one(n))
        .filter_map(|(n, songs)| {
            let k = fold(n);
            (!k.is_empty() && seen.insert(k.clone()) && cache.names.get(&k).is_none_or(|e| e.due(now))).then(|| (*songs, n.clone()))
        })
        .collect();
    // The artists with most songs first.
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    out.into_iter().map(|(_, n)| n).collect()
}

// ---------- the real network and clock ----------

struct Http(reqwest::blocking::Client);

impl Net for Http {
    fn get(&mut self, url: &str) -> Reply {
        match self.0.get(url).header("Accept", "application/json").send() {
            Ok(r) => {
                let status = r.status().as_u16();
                if status == 503 || status == 429 {
                    let after = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
                    return Reply::Busy(after);
                }
                if !r.status().is_success() {
                    return Reply::Failed(status);
                }
                // (Capped: an artist search answer is a few KB.)
                match crate::net::text(r) {
                    Ok(body) => Reply::Body(body),
                    Err(_) => Reply::Failed(status),
                }
            }
            Err(_) => Reply::Offline,
        }
    }
}

/// The app's User-Agent for MusicBrainz: name, version and the project's public page (as
/// MusicBrainz asks: "Application/version ( contact-url )"). Nothing about the user.
pub const USER_AGENT: &str = concat!("Cadence/", env!("CARGO_PKG_VERSION"), " ( ", env!("CARGO_PKG_REPOSITORY"), " )");

/// Real time, sleeping in short steps so Stop and Pause take effect at once.
struct Wall(std::time::Instant);

impl Clock for Wall {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
    fn sleep_ms(&mut self, ms: u64) {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < end && !STOP.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(250).min(end - std::time::Instant::now()));
        }
    }
    fn unix(&self) -> u64 {
        crate::history::now()
    }
}

// ---------- the job in the app ----------

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// Stopped by the user this session: not started again by itself until Cadence restarts.
static STOPPED: AtomicBool = AtomicBool::new(false);
static PROGRESS: Mutex<(usize, usize)> = Mutex::new((0, 0));

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub done: usize,
    pub total: usize,
    pub running: bool,
    pub paused: bool,
    /// The setting (Settings › Learn artist names online).
    pub enabled: bool,
    /// Library names MusicBrainz knows.
    pub known: usize,
    /// Library names not asked about yet (or due again).
    pub waiting: usize,
}

pub fn status(ctx: &Ctx) -> Status {
    let dir = &ctx.state.data_dir;
    let names = crate::identity::gather(&ctx.state).names;
    let now = crate::history::now();
    let (known, waiting, paused) = with_cache(dir, |c| {
        let known = names.iter().filter(|(n, _)| c.names.get(&fold(n)).is_some_and(|e| e.found.is_some())).count();
        (known, todo(&names, c, now).len(), c.paused)
    });
    let (done, total) = *PROGRESS.lock();
    Status { done, total, running: RUNNING.load(Ordering::SeqCst), paused, enabled: ctx.state.settings.lock().artist_names_online, known, waiting }
}

fn emit(ctx: &Ctx) {
    ctx.emit("artist-names-progress", status(ctx));
}

/// Something else heavy or online is running: the job waits for it (a scan, loudness
/// measuring, the catalogue checks of Needs Attention).
fn others_busy(ctx: &Ctx) -> bool {
    let lib = ctx.state.library.lock();
    lib.scanning || lib.analyzing || crate::libfix::catalogue_job_running()
}

/// After a scan and at startup: starts the job if it's on, not paused or stopped, and has names
/// to ask about.
pub fn auto(ctx: &Ctx) {
    if !ctx.state.settings.lock().artist_names_online || STOPPED.load(Ordering::SeqCst) || with_cache(&ctx.state.data_dir, |c| c.paused) {
        return;
    }
    start(ctx.clone());
}

/// Starts the job (one at a time). False if it's running, off, or has nothing to do.
pub fn start(ctx: Ctx) -> bool {
    if !ctx.state.settings.lock().artist_names_online {
        return false;
    }
    let names = crate::identity::gather(&ctx.state).names;
    let dir = ctx.state.data_dir.clone();
    let list = with_cache(&dir, |c| todo(&names, c, crate::history::now()));
    let library: HashSet<String> = names.iter().map(|(n, _)| fold(n)).filter(|f| !f.is_empty()).collect();
    if list.is_empty() || RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    STOP.store(false, Ordering::SeqCst);
    *PROGRESS.lock() = (0, list.len());
    let spawned = std::thread::Builder::new().name("cadence-musicbrainz".into()).spawn(move || {
        crate::jobs::background_priority();
        let client = crate::net::builder().timeout(Duration::from_secs(15)).user_agent(USER_AGENT).build();
        let outcome = match client {
            Ok(client) => {
                emit(&ctx);
                let mut net = Http(client);
                let mut clock = Wall(std::time::Instant::now());
                let mut cache = with_cache(&dir, |c| c.clone());
                let total = list.len();
                let outcome = run(
                    &list,
                    &library,
                    &mut net,
                    &mut clock,
                    &mut cache,
                    |i| {
                        // Waits while a scan or another job runs.
                        while !STOP.load(Ordering::SeqCst) && others_busy(&ctx) {
                            std::thread::sleep(Duration::from_secs(2));
                        }
                        if STOP.load(Ordering::SeqCst) || !ctx.state.settings.lock().artist_names_online {
                            return false;
                        }
                        *PROGRESS.lock() = (i, total);
                        if i % 10 == 0 {
                            emit(&ctx);
                        }
                        true
                    },
                    |c, done| {
                        let paused = with_cache(&dir, |m| m.paused);
                        with_cache(&dir, |m| {
                            *m = c.clone();
                            m.paused = paused;
                        });
                        save_cache(&dir);
                        if done > 0 {
                            crate::identity::invalidate();
                        }
                    },
                );
                outcome
            }
            Err(e) => {
                log::warn!("no HTTP client for MusicBrainz: {e}");
                Outcome::Offline
            }
        };
        log::info!("MusicBrainz artist names: {outcome:?}");
        RUNNING.store(false, Ordering::SeqCst);
        *PROGRESS.lock() = (0, 0);
        emit(&ctx);
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
    }
    true
}

/// Pause (kept across restarts: it doesn't start by itself) or resume.
pub fn set_paused(ctx: &Ctx, paused: bool) {
    let dir = &ctx.state.data_dir;
    with_cache(dir, |c| c.paused = paused);
    save_cache(dir);
    if paused {
        STOP.store(true, Ordering::SeqCst);
    } else {
        STOPPED.store(false, Ordering::SeqCst);
        start(ctx.clone());
    }
    emit(ctx);
}

/// Stops the job for this session.
pub fn stop(ctx: &Ctx) {
    STOPPED.store(true, Ordering::SeqCst);
    STOP.store(true, Ordering::SeqCst);
    emit(ctx);
}

/// The setting was turned off: a running job stops.
pub fn turned_off() {
    STOP.store(true, Ordering::SeqCst);
}

/// Recorded MusicBrainz answers (trimmed), for tests: nothing is sent to MusicBrainz from tests.
#[cfg(test)]
pub(crate) mod tests_fixtures {
    /// Trimmed from MusicBrainz's answer for `alias:yonezu` (2026-11).
    pub const YONEZU: &str = r#"{"created":"2026-11-10T00:00:00.000Z","count":2,"offset":0,"artists":[
      {"id":"6a1d0a19-4a4c-4d0d-9b40-yonezu","type":"Person","score":100,"name":"米津玄師","sort-name":"Yonezu, Kenshi","country":"JP",
       "aliases":[{"sort-name":"よねづけんし","name":"よねづけんし","locale":"ja","type":"Artist name","primary":null},
                  {"sort-name":"Yonezu, Kenshi","name":"Kenshi Yonezu","locale":"en","type":"Artist name","primary":true},
                  {"sort-name":"요네즈, 켄시","name":"켄시 요네즈","locale":"ko","type":"Artist name","primary":null},
                  {"sort-name":"Hachi","name":"ハチ","locale":null,"type":"Artist name","primary":null}]},
      {"id":"00000000-tribute","type":"Group","score":62,"name":"Kenshi Yonezu Tribute Band","sort-name":"Kenshi Yonezu Tribute Band"}]}"#;

    /// Trimmed from the answer for `bts` (the score as a string, as the docs show it).
    pub const BTS: &str = r#"{"count":2,"offset":0,"artists":[
      {"id":"0d79fe8e-ba27-4859-bb8c-bts","type":"Group","score":"100","name":"방탄소년단","sort-name":"Bangtan Boys","country":"KR",
       "aliases":[{"name":"BTS","sort-name":"BTS","locale":"en","type":"Artist name","primary":true},
                  {"name":"Bangtan Sonyeondan","sort-name":"Bangtan Sonyeondan","type":null},
                  {"name":"防弾少年団","sort-name":"防弾少年団","locale":"ja","type":"Artist name"},
                  {"name":"bangtan","sort-name":"bangtan","type":"Search hint"}]},
      {"id":"x-other","type":"Group","score":"55","name":"BTS Tribute","sort-name":"BTS Tribute"}]}"#;

    /// Two artists called "Aimer" with the same score: a common name, none is taken.
    pub const AMBIGUOUS: &str = r#"{"count":2,"offset":0,"artists":[
      {"id":"aimer-1","score":100,"name":"Aimer","sort-name":"Aimer","country":"JP"},
      {"id":"aimer-2","score":100,"name":"Aimer","sort-name":"Aimer","country":"FR"}]}"#;

}

#[cfg(test)]
mod tests {
    use super::tests_fixtures::*;
    use super::*;

    #[test]
    fn safe_matches_only() {
        let y = pick("米津玄師", YONEZU).expect("found");
        assert_eq!(y.name, "米津玄師");
        assert_eq!(y.sort_name, "Yonezu, Kenshi");
        assert!(y.aliases.iter().any(|a| a.name == "Kenshi Yonezu" && a.locale.as_deref() == Some("en") && a.primary));
        assert_eq!(pick("Kenshi Yonezu", YONEZU).map(|a| a.mbid), Some(y.mbid.clone()), "by an alias");
        assert_eq!(pick("KENSHI  YONEZU", YONEZU).map(|a| a.mbid), Some(y.mbid.clone()), "folded");
        // The search found it, but none of its names is the library's name: not taken.
        assert_eq!(pick("Yonezu", YONEZU), None);
        // A low score isn't enough even with the same name.
        assert_eq!(pick("Kenshi Yonezu Tribute Band", YONEZU), None);
        // Three letters: only with something local agreeing (the library has the Hangul name too).
        assert_eq!(pick("BTS", BTS), None);
        let library: HashSet<String> = ["BTS", "방탄소년단"].iter().map(|n| fold(n)).collect();
        let b = pick_with("BTS", BTS, &library).expect("BTS");
        assert_eq!(b.name, "방탄소년단");
        assert_eq!(b.native_and_latin(), (Some("방탄소년단".into()), Some("BTS".into())));
        // Not by a search hint.
        assert_eq!(pick("bangtan", BTS), None);
        assert_eq!(pick("Aimer", AMBIGUOUS), None);
        assert_eq!(pick("x", "not json"), None);
    }

    #[test]
    fn a_same_named_rival_below_90_still_counts() {
        // Two different artists called "Invented Name": the second scores under 90 but is still
        // a second artist of that name, so neither is taken from the search alone.
        let two = r#"{"artists":[
          {"id":"inv-1","score":100,"name":"Invented Name","sort-name":"Name, Invented","country":"US",
           "aliases":[{"name":"架空の名前","type":"Artist name"}]},
          {"id":"inv-2","score":85,"name":"Invented Name","sort-name":"Invented Name","country":"GB"}]}"#;
        assert_eq!(pick("Invented Name", two), None);
        // With the library also having the first one's other name, it is that one.
        let library: HashSet<String> = ["Invented Name", "架空の名前"].iter().map(|n| fold(n)).collect();
        assert_eq!(pick_with("Invented Name", two, &library).map(|a| a.mbid), Some("inv-1".into()));
        // Under 90 alone: nothing, however unambiguous.
        let low = r#"{"artists":[{"id":"inv-3","score":80,"name":"Another Invention","sort-name":"Another Invention"}]}"#;
        assert_eq!(pick("Another Invention", low), None);
        // Two at nearly the same score: nothing even when both are corroborated.
        let near = r#"{"artists":[
          {"id":"a","score":100,"name":"Twin Name","aliases":[{"name":"ふたご","type":"Artist name"}]},
          {"id":"b","score":95,"name":"Twin Name","aliases":[{"name":"ふたご","type":"Artist name"}]}]}"#;
        let library: HashSet<String> = ["Twin Name", "ふたご"].iter().map(|n| fold(n)).collect();
        assert_eq!(pick_with("Twin Name", near, &library), None);
        // A one-character CJK name: corroboration needed; a longer one isn't short.
        let one = r#"{"artists":[{"id":"c","score":100,"name":"嵐","aliases":[{"name":"Arashi","type":"Artist name"}]}]}"#;
        assert_eq!(pick("嵐", one), None);
        assert!(pick_with("嵐", one, &["嵐", "Arashi"].iter().map(|n| fold(n)).collect()).is_some());
    }

    #[test]
    fn the_question_is_escaped_and_names_the_app() {
        let u = url("AC/DC");
        assert!(u.starts_with("https://musicbrainz.org/ws/2/artist/?query="));
        assert!(u.contains(&*urlencoding::encode("AC\\/DC")), "{u}");
        assert!(u.ends_with("&fmt=json&limit=10"));
        assert_eq!(escape("a+b (c)"), "a\\+b \\(c\\)");
        assert!(USER_AGENT.starts_with("Cadence/") && USER_AGENT.contains("github.com"), "{USER_AGENT}");
    }

    struct FakeNet {
        replies: Vec<Reply>,
        asked: Vec<(String, u64)>,
        clock: std::rc::Rc<std::cell::Cell<u64>>,
    }
    impl Net for FakeNet {
        fn get(&mut self, url: &str) -> Reply {
            self.asked.push((url.to_string(), self.clock.get()));
            if self.replies.is_empty() { Reply::Offline } else { self.replies.remove(0) }
        }
    }
    struct FakeClock(std::rc::Rc<std::cell::Cell<u64>>, Vec<u64>);
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.get()
        }
        fn sleep_ms(&mut self, ms: u64) {
            self.1.push(ms);
            self.0.set(self.0.get() + ms);
        }
        fn unix(&self) -> u64 {
            1_800_000_000 + self.0.get() / 1000
        }
    }

    fn fakes(replies: Vec<Reply>) -> (FakeNet, FakeClock) {
        let t = std::rc::Rc::new(std::cell::Cell::new(0));
        (FakeNet { replies, asked: Vec::new(), clock: t.clone() }, FakeClock(t, Vec::new()))
    }

    #[test]
    fn paced_one_request_per_1_1_seconds() {
        let names: Vec<String> = ["米津玄師", "BTS", "Aimer"].iter().map(|s| s.to_string()).collect();
        let (mut net, mut clock) = fakes(vec![Reply::Body(YONEZU.into()), Reply::Body(BTS.into()), Reply::Body(AMBIGUOUS.into())]);
        let mut cache = Cache::default();
        let mut saves = 0;
        let library: HashSet<String> = ["米津玄師", "BTS", "방탄소년단", "Aimer"].iter().map(|n| fold(n)).collect();
        let out = run(&names, &library, &mut net, &mut clock, &mut cache, |_| true, |_, _| saves += 1);
        assert_eq!(out, Outcome::Done);
        let times: Vec<u64> = net.asked.iter().map(|(_, t)| *t).collect();
        assert!(times.windows(2).all(|w| w[1] - w[0] >= GAP_MS), "{times:?}");
        assert!(cache.names[&fold("米津玄師")].found.is_some());
        assert!(cache.names[&fold("bts")].found.is_some());
        // No safe match: kept, with a date to ask again.
        let aimer = &cache.names[&fold("Aimer")];
        assert!(aimer.found.is_none());
        assert_eq!(aimer.retry, Some(aimer.asked + RETRY_UNMATCHED));
        assert_eq!(saves, 1);
    }

    #[test]
    fn busy_backs_off_honouring_retry_after_then_gives_up() {
        let names = vec!["BTS".to_string()];
        let (mut net, mut clock) = fakes(vec![Reply::Busy(Some(7)), Reply::Busy(None), Reply::Body(BTS.into())]);
        let mut cache = Cache::default();
        assert_eq!(run(&names, &HashSet::new(), &mut net, &mut clock, &mut cache, |_| true, |_, _| {}), Outcome::Done);
        assert!(clock.1.contains(&7_000), "Retry-After honoured: {:?}", clock.1);
        assert!(clock.1.contains(&4_000), "then doubling: {:?}", clock.1);
        assert_eq!(net.asked.len(), 3, "the same name asked again");
        // Busy every time: stops (nothing filed for the name, so it's asked next time).
        let (mut net, mut clock) = fakes(vec![Reply::Busy(None); 10]);
        let mut cache = Cache::default();
        assert_eq!(run(&names, &HashSet::new(), &mut net, &mut clock, &mut cache, |_| true, |_, _| {}), Outcome::Busy);
        assert!(cache.names.is_empty());
    }

    #[test]
    fn offline_stops_quietly_and_resumes_where_it_left_off() {
        let names: Vec<String> = ["米津玄師", "BTS"].iter().map(|s| s.to_string()).collect();
        let (mut net, mut clock) = fakes(vec![Reply::Body(YONEZU.into()), Reply::Offline]);
        let mut cache = Cache::default();
        assert_eq!(run(&names, &HashSet::new(), &mut net, &mut clock, &mut cache, |_| true, |_, _| {}), Outcome::Offline);
        assert_eq!(cache.names.len(), 1);
        // Next time (after a restart, say): only what's left.
        let lib: Vec<(String, u32)> = names.iter().map(|n| (n.clone(), 1)).collect();
        let left = todo(&lib, &cache, clock.unix());
        assert_eq!(left, ["BTS"]);
        // Stopped by the user before the second name.
        let (mut net, mut clock) = fakes(vec![Reply::Body(BTS.into())]);
        assert_eq!(run(&left, &HashSet::new(), &mut net, &mut clock, &mut Cache::default(), |_| false, |_, _| {}), Outcome::Stopped);
        assert!(net.asked.is_empty());
    }

    #[test]
    fn unmatched_names_are_asked_again_after_their_retry_date() {
        let mut cache = Cache::default();
        let now = 1_800_000_000;
        cache.names.insert(fold("Aimer"), Entry { asked: now, retry: Some(now + RETRY_UNMATCHED), found: None });
        cache.names.insert(fold("米津玄師"), Entry { asked: now, retry: None, found: pick("米津玄師", YONEZU) });
        cache.names.insert(fold("Gone Wrong"), Entry { asked: now, retry: Some(now + RETRY_FAILED), found: None });
        let lib: Vec<(String, u32)> = ["Aimer", "米津玄師", "Gone Wrong", "Various Artists", "New One"].iter().map(|n| (n.to_string(), 1)).collect();
        assert_eq!(todo(&lib, &cache, now + 86_400), ["New One"]);
        assert_eq!(todo(&lib, &cache, now + RETRY_FAILED), ["Gone Wrong", "New One"]);
        assert_eq!(todo(&lib, &cache, now + RETRY_UNMATCHED), ["Aimer", "Gone Wrong", "New One"]);
        // The cache reads back as written.
        let json = serde_json::to_string(&cache).unwrap();
        let back: Cache = serde_json::from_str(&json).unwrap();
        assert_eq!(back.names, cache.names);
    }
}
