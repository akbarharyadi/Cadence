//! Your Stats over the whole listening history: the archive's year files and history.json.
//!
//! Each archive year is read once, line by line, into a summary (`Agg`: per song its plays, plays
//! heard to the end, skips, time, first and last play; plays per hour and weekday; time per day;
//! songs per day; the year's first song; albums heard through), kept in memory. The archive only
//! grows at its end, so when a year file grows only the new lines are read; one that shrank or was
//! rewritten (a restore) is read again. The live window (history.json, in memory anyway) is
//! summarised on every request: 20,000 plays, a few milliseconds. A play in both (archived just
//! before a crash) counts once. No raw plays are kept: only the summaries.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::Mutex;
use serde::Serialize;

use crate::api::{ListeningStats, StatItem};
use crate::history::PlayEvent;
use crate::state::AppState;

const DAY: i64 = 86_400;
/// How many of a year's last plays are remembered to spot the same play again (see `Agg::tail`).
const TAIL: usize = 4096;
/// "Forgotten favourites": not played for this long.
const FORGOTTEN_AFTER: u64 = 180 * 86_400;

// ---------- sections the page shows (besides the original ones) ----------

/// A song played on this date in an earlier year.
#[derive(Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DayItem {
    pub id: String,
    pub name: String,
    pub sub: String,
    pub year: i64,
    pub plays: u32,
}

/// A song played often (or loved, or rated 4–5 stars) and not for half a year.
#[derive(Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Forgotten {
    pub id: String,
    pub name: String,
    pub sub: String,
    /// Plays heard to the end, all time.
    pub plays: u32,
    /// Unix seconds of the last play.
    pub last: u64,
    pub loved: bool,
    pub rating: u8,
}

/// Runs of days with listening (local days).
#[derive(Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Streaks {
    /// Ending today or yesterday (0: none going on).
    pub current: u32,
    pub longest: u32,
    /// The longest run's first and last day, "YYYY-MM-DD".
    pub longest_from: String,
    pub longest_to: String,
    /// Days with any listening.
    pub days: u32,
}

/// Artists heard for the first time in a month ("YYYY-MM").
#[derive(Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MonthCount {
    pub month: String,
    pub count: u32,
}

/// The first song played this year.
#[derive(Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FirstSong {
    pub id: String,
    pub name: String,
    pub sub: String,
    pub at: u64,
}

/// Plays by the decade of the songs' year tag.
#[derive(Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Decade {
    pub decade: u32,
    pub plays: u32,
    pub minutes: u32,
}

/// Plays by the language the songs are sung in (`songlang`, as the DJ tells it).
#[derive(Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    /// "ja", "id"…
    pub code: String,
    /// "Japanese", "Indonesian"…
    pub name: String,
    pub plays: u32,
    pub minutes: u32,
}

// ---------- local time ----------

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's civil_from_days.
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// (year, month, day) → days since 1970-01-01.
#[cfg_attr(not(windows), allow(dead_code))]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds the local time is ahead of UTC now (the same for every play: plays from before a
/// daylight-saving change are off by an hour, which hours of the day and days can live with).
pub(crate) fn local_offset() -> i64 {
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
            fn GetSystemTime(t: *mut SystemTime);
        }
        let (mut local, mut utc) = (SystemTime::default(), SystemTime::default());
        unsafe {
            GetLocalTime(&mut local);
            GetSystemTime(&mut utc);
        }
        let secs = |t: &SystemTime| days_from_civil(t.year as i64, t.month as u32, t.day as u32) * DAY + t.hour as i64 * 3600 + t.minute as i64 * 60 + t.second as i64;
        // To the quarter hour (the two clocks were read a moment apart).
        ((secs(&local) - secs(&utc)) as f64 / 900.0).round() as i64 * 900
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() { 0 } else { tm.tm_gmtoff as i64 }
    }
    #[cfg(target_os = "macos")]
    unsafe {
        #[repr(C)]
        struct Tm {
            sec: i32,
            min: i32,
            hour: i32,
            mday: i32,
            mon: i32,
            year: i32,
            wday: i32,
            yday: i32,
            isdst: i32,
            gmtoff: i64,
            zone: *const i8,
        }
        unsafe extern "C" {
            fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
        }
        let now = crate::history::now() as i64;
        let mut tm: Tm = std::mem::zeroed();
        if localtime_r(&now, &mut tm).is_null() { 0 } else { tm.gmtoff }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "android", target_os = "macos")))]
    {
        0
    }
}

fn day_name(day: i64) -> String {
    let (y, m, d) = civil(day);
    format!("{y}-{m:02}-{d:02}")
}

// ---------- summaries ----------

/// One song's plays in a summary.
#[derive(Clone, Copy, Default, Debug)]
struct Song {
    plays: u32,
    finished: u32,
    skipped: u32,
    secs: f32,
    first: u32,
    last: u32,
}

/// Song and album ids, as small numbers (each id's text kept once).
#[derive(Default)]
struct Names {
    all: Vec<String>,
    by: HashMap<String, u32>,
}

impl Names {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.by.get(s) {
            return i;
        }
        let i = self.all.len() as u32;
        self.all.push(s.to_string());
        self.by.insert(s.to_string(), i);
        i
    }

    fn get(&self, s: &str) -> Option<u32> {
        self.by.get(s).copied()
    }

    fn name(&self, i: u32) -> &str {
        self.all.get(i as usize).map_or("", String::as_str)
    }
}

/// A time range's listening per bucket (the page's chart), for a summary of a range.
#[derive(Clone, Copy)]
struct Window {
    since: u64,
    bucket: u64,
    buckets: usize,
}

/// Heard to the end: 90% of the song, or within 10 s of its end, and not skipped.
fn finished(e: &PlayEvent) -> bool {
    !e.skipped && (e.duration <= 0.0 || e.listened >= e.duration * 0.9 || e.duration - e.listened <= 10.0)
}

/// What a set of plays adds up to.
#[derive(Default)]
struct Agg {
    songs: HashMap<u32, Song>,
    /// Plays per local hour of the day, and per weekday (Monday first).
    hours: [u32; 24],
    weekdays: [u32; 7],
    /// Seconds heard per local day (days since 1970).
    days: HashMap<i32, f32>,
    /// Songs played (not skipped) per local day, with how often.
    dates: HashMap<i32, Vec<(u32, u16)>>,
    /// Each local year's first play: (unix seconds, song).
    firsts: HashMap<i32, (u64, u32)>,
    /// Albums heard through: per album, the songs of the album heard to the end in each run of
    /// plays from it (`open`: the run going on at the end).
    runs: HashMap<u32, Vec<u16>>,
    open: Option<(u32, Vec<u32>)>,
    timeline: Vec<f64>,
    window: Option<Window>,
    max_at: u64,
    min_at: u64,
    /// The last plays read (song, start): the same play read again (archived twice after a
    /// crash, or both archived and still in history.json) is counted once.
    tail: VecDeque<(u32, u64)>,
    tail_set: HashSet<(u32, u64)>,
}

impl Agg {
    fn windowed(w: Window) -> Self {
        Agg { timeline: vec![0.0; w.buckets], window: Some(w), ..Default::default() }
    }

    fn seen(&self, key: &(u32, u64)) -> bool {
        self.tail_set.contains(key)
    }

    /// Adds a play (`seen`: every play read so far in this pass, for a whole read).
    fn add(&mut self, names: &mut Names, e: &PlayEvent, offset: i64, seen: Option<&mut HashSet<(u32, u64)>>) {
        if let Some(w) = self.window
            && e.at < w.since {
                return;
            }
        let t = names.id(&e.track_id);
        let key = (t, e.at);
        let again = match seen {
            Some(s) => !s.insert(key),
            None => self.tail_set.contains(&key),
        };
        if again {
            return;
        }
        self.tail.push_back(key);
        self.tail_set.insert(key);
        if self.tail.len() > TAIL
            && let Some(old) = self.tail.pop_front() {
                self.tail_set.remove(&old);
            }
        let done = finished(e);
        let heard = e.listened.max(0.0);
        let at = e.at.min(u32::MAX as u64) as u32;
        let s = self.songs.entry(t).or_insert(Song { first: at, last: at, ..Default::default() });
        s.plays += 1;
        s.finished += done as u32;
        s.skipped += e.skipped as u32;
        s.secs += heard as f32;
        s.first = s.first.min(at);
        s.last = s.last.max(at);
        let local = e.at as i64 + offset;
        let day = local.div_euclid(DAY);
        self.hours[(local.rem_euclid(DAY) / 3600) as usize] += 1;
        // 1970-01-01 was a Thursday.
        self.weekdays[(day + 3).rem_euclid(7) as usize] += 1;
        *self.days.entry(day as i32).or_insert(0.0) += heard as f32;
        if !e.skipped {
            let list = self.dates.entry(day as i32).or_default();
            match list.iter_mut().find(|(x, _)| *x == t) {
                Some((_, n)) => *n = n.saturating_add(1),
                None => list.push((t, 1)),
            }
        }
        let year = civil(day).0 as i32;
        let first = self.firsts.entry(year).or_insert((e.at, t));
        if e.at < first.0 {
            *first = (e.at, t);
        }
        let album = names.id(&e.album_id);
        match &mut self.open {
            Some((a, heard)) if *a == album => {
                if done && !heard.contains(&t) {
                    heard.push(t);
                }
            }
            _ => {
                self.close_run();
                self.open = Some((album, if done { vec![t] } else { Vec::new() }));
            }
        }
        if let Some(w) = self.window
            && let Some(last) = self.timeline.len().checked_sub(1) {
                self.timeline[(((e.at - w.since) / w.bucket.max(1)) as usize).min(last)] += heard;
            }
        self.max_at = self.max_at.max(e.at);
        self.min_at = if self.min_at == 0 { e.at } else { self.min_at.min(e.at) };
    }

    fn close_run(&mut self) {
        if let Some((a, heard)) = self.open.take()
            && heard.len() >= 2 {
                self.runs.entry(a).or_default().push(heard.len().min(u16::MAX as usize) as u16);
            }
    }

    /// Albums' runs, the one going on included.
    fn all_runs(&self) -> impl Iterator<Item = (u32, u16)> + '_ {
        let open = self.open.iter().filter(|(_, h)| h.len() >= 2).map(|(a, h)| (*a, h.len() as u16));
        self.runs.iter().flat_map(|(a, v)| v.iter().map(move |n| (*a, *n))).chain(open)
    }

    /// Roughly how much memory the summary holds (for the tests' report).
    #[cfg(test)]
    fn approx_bytes(&self) -> usize {
        self.songs.capacity() * (4 + std::mem::size_of::<Song>() + 8)
            + self.days.capacity() * 16
            + self.dates.values().map(|v| v.capacity() * 8 + 40).sum::<usize>()
            + self.firsts.capacity() * 24
            + self.runs.values().map(|v| v.capacity() * 2 + 40).sum::<usize>()
            + self.tail.capacity() * 16
            + self.tail_set.capacity() * 24
    }
}

/// One archive year, summarised.
struct Year {
    len: u64,
    modified: Option<SystemTime>,
    /// Where the lines read so far end.
    end: u64,
    agg: Agg,
}

/// A range's summary of the archive (only when history.json doesn't reach back that far).
struct WindowCache {
    key: (u64, u64, u64, Vec<(PathBuf, u64)>),
    agg: Agg,
}

#[derive(Default)]
struct Cache {
    offset: i64,
    names: Names,
    years: HashMap<PathBuf, Year>,
    windows: HashMap<String, WindowCache>,
}

/// The summaries, per data folder.
static CACHES: Mutex<Option<HashMap<PathBuf, Cache>>> = Mutex::new(None);

/// Forgets the summaries of a data folder (the archive was rewritten: a restore).
pub fn invalidate(data_dir: &Path) {
    if let Some(all) = CACHES.lock().as_mut() {
        all.remove(data_dir);
    }
}

/// Brings the year summaries up to date with the archive's files.
fn refresh(cache: &mut Cache, data_dir: &Path) {
    let files = crate::history::archive_files(data_dir);
    cache.years.retain(|p, _| files.contains(p));
    let offset = cache.offset;
    for f in files {
        let Ok(meta) = std::fs::metadata(&f) else { continue };
        let (len, modified) = (meta.len(), meta.modified().ok());
        let names = &mut cache.names;
        if let Some(y) = cache.years.get_mut(&f) {
            if y.len == len && y.modified == modified {
                continue;
            }
            if len > y.len {
                // Plays added at the end: only those are read.
                if let Ok(end) = crate::history::read_archive_from(&f, y.end, |e| y.agg.add(names, &e, offset, None)) {
                    y.end = end;
                    y.len = len;
                    y.modified = modified;
                    continue;
                }
            }
        }
        let mut agg = Agg::default();
        let mut seen = HashSet::new();
        let Ok(end) = crate::history::read_archive_from(&f, 0, |e| agg.add(names, &e, offset, Some(&mut seen))) else {
            cache.years.remove(&f);
            continue;
        };
        cache.years.insert(f, Year { len, modified, end, agg });
    }
}

/// The archive's plays from `w.since`, older than history.json's first (`live_first`: the
/// archive's later plays are history.json's too).
fn archive_window(cache: &mut Cache, data_dir: &Path, w: Window, live_first: u64) -> Agg {
    let mut agg = Agg::windowed(w);
    let mut seen = HashSet::new();
    let from_year = civil((w.since as i64 - DAY).div_euclid(DAY)).0;
    let offset = cache.offset;
    for f in crate::history::archive_files(data_dir) {
        let year: i64 = f.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse().ok()).unwrap_or(i64::MAX);
        if year < from_year {
            continue;
        }
        let names = &mut cache.names;
        let _ = crate::history::read_archive_from(&f, 0, |e| {
            if e.at < live_first {
                agg.add(names, &e, offset, Some(&mut seen));
            }
        });
    }
    agg.close_run();
    agg
}

/// Merged per-song plays of several summaries.
fn songs_of(aggs: &[&Agg]) -> HashMap<u32, Song> {
    let mut out: HashMap<u32, Song> = HashMap::new();
    for a in aggs {
        for (t, s) in &a.songs {
            let m = out.entry(*t).or_insert(Song { first: s.first, last: s.last, ..Default::default() });
            m.plays += s.plays;
            m.finished += s.finished;
            m.skipped += s.skipped;
            m.secs += s.secs;
            m.first = m.first.min(s.first);
            m.last = m.last.max(s.last);
        }
    }
    out
}

/// The listening stats for a range ("week", "month", "year", "all"), over the whole history.
pub fn listening_stats(state: &AppState, range: String) -> ListeningStats {
    let now = crate::history::now();
    let (days, bucket) = match range.as_str() {
        "week" => (7, 86_400),
        "month" => (30, 86_400),
        "year" => (365, 30 * 86_400),
        _ => (0, 0),
    };
    let since = if days == 0 { 0 } else { now.saturating_sub(days * 86_400) };
    let window = (bucket > 0).then(|| Window { since, bucket, buckets: (days * 86_400 / bucket) as usize });
    let offset = local_offset();
    // The songs' languages (worked out once per library, `dj::facts`; before any lock is taken).
    let facts = crate::dj::facts(state);
    let lang_of: HashMap<&str, crate::songlang::Lang> = facts
        .songs
        .iter()
        .filter(|s| s.lang != crate::songlang::Lang::Unknown && s.lang_conf >= crate::dj::LANG_MIN)
        .map(|s| (s.id.as_str(), s.lang))
        .collect();

    let mut caches = CACHES.lock();
    let cache = caches.get_or_insert_with(HashMap::new).entry(state.data_dir.clone()).or_default();
    if cache.offset != offset {
        *cache = Cache { offset, ..Default::default() };
    }
    refresh(cache, &state.data_dir);
    let archive_max = cache.years.values().map(|y| y.agg.max_at).max().unwrap_or(0);

    // history.json: summarised now (all of it, and the range).
    let (mut live_all, mut live_range, live_first) = {
        let h = state.history.lock();
        let mut all = Agg::default();
        let mut ranged = window.map(Agg::windowed);
        for e in &h.events {
            let Some(t) = (e.at <= archive_max).then(|| cache.names.get(&e.track_id)).flatten() else {
                all.add(&mut cache.names, e, offset, None);
                if let Some(r) = ranged.as_mut() {
                    r.add(&mut cache.names, e, offset, None);
                }
                continue;
            };
            // Archived as well (just before a crash): already counted there.
            if cache.years.values().any(|y| y.agg.seen(&(t, e.at))) {
                continue;
            }
            all.add(&mut cache.names, e, offset, None);
            if let Some(r) = ranged.as_mut() {
                r.add(&mut cache.names, e, offset, None);
            }
        }
        (all, ranged, h.events.first().map_or(u64::MAX, |e| e.at))
    };
    live_all.close_run();
    if let Some(r) = live_range.as_mut() {
        r.close_run();
    }

    // The archive's part of the range, when history.json doesn't reach back to its start.
    let window_key = window.map(|w| {
        let stamps: Vec<(PathBuf, u64)> = cache.years.iter().map(|(p, y)| (p.clone(), y.len)).collect();
        (w.since / 86_400, w.bucket, live_first, stamps)
    });
    let needs_archive = window.is_some_and(|w| live_first > w.since && archive_max >= w.since);
    if let (true, Some(w), Some(key)) = (needs_archive, window, window_key.clone()) {
        let fresh = cache.windows.get(&range).is_some_and(|c| c.key == key);
        if !fresh {
            let start = Window { since: (w.since / 86_400) * 86_400, ..w };
            let agg = archive_window(cache, &state.data_dir, start, live_first);
            cache.windows.insert(range.clone(), WindowCache { key, agg });
        }
    } else {
        cache.windows.remove(&range);
    }
    let cache = &*cache;
    let years: Vec<&Agg> = cache.years.values().map(|y| &y.agg).collect();
    let mut everything: Vec<&Agg> = years.clone();
    everything.push(&live_all);
    let ranged: Vec<&Agg> = match &live_range {
        None => everything.clone(),
        Some(r) => {
            let mut v = vec![r];
            if needs_archive && let Some(c) = cache.windows.get(&range) {
                v.push(&c.agg);
            }
            v
        }
    };

    let lib = state.library.lock();
    let names = &cache.names;
    let track = |t: u32| lib.by_id.get(names.name(t)).map(|&i| &lib.snapshot.tracks[i]);
    let mut out = ListeningStats { range: range.clone(), ..Default::default() };

    // ----- the original sections, now over the whole history -----
    let range_songs = songs_of(&ranged);
    let mut songs: HashMap<String, (StatItem, f64)> = HashMap::new();
    let mut artists: HashMap<String, (StatItem, f64)> = HashMap::new();
    let mut albums: HashMap<String, (StatItem, f64)> = HashMap::new();
    let album_of: HashMap<&str, &crate::library::Album> = lib.snapshot.albums.iter().map(|a| (a.id.as_str(), a)).collect();
    let credits = crate::api::artist_index(lib.snapshot.albums.iter().map(|a| a.artist.as_str()));
    let pages = crate::api::artist_pages(&credits);
    // One artist under several spellings (米津玄師, Kenshi Yonezu) is one page, as in the window:
    // the spelling on most songs.
    let shown = {
        let mut songs: HashMap<String, (String, u32)> = HashMap::new();
        for a in &lib.snapshot.albums {
            for n in credits.get(&a.artist).into_iter().flatten() {
                songs.entry(n.to_lowercase()).or_insert_with(|| (n.clone(), 0)).1 += a.track_ids.len() as u32;
            }
        }
        crate::identity::current().merge_pages(&songs.into_values().collect::<Vec<_>>())
    };
    // Many songs share an artist credit: worked out once per credit.
    let mut under: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut artists_of = |artist: &str, credit: &str| -> Vec<String> {
        under
            .entry((artist.to_string(), credit.to_string()))
            .or_insert_with(|| {
                let mut out: Vec<String> = Vec::new();
                for n in crate::api::stat_artists(artist, credit, &credits, &pages) {
                    let page = shown.get(&n.to_lowercase()).cloned().unwrap_or(n);
                    if !out.contains(&page) {
                        out.push(page);
                    }
                }
                out
            })
            .clone()
    };
    let mut seconds = 0.0f64;
    let add = |map: &mut HashMap<String, (StatItem, f64)>, id: &str, name: &str, sub: &str, s: &Song| {
        let (item, secs) = map
            .entry(id.to_lowercase())
            .or_insert_with(|| (StatItem { id: id.to_string(), name: name.to_string(), sub: sub.to_string(), ..Default::default() }, 0.0));
        item.plays += s.plays;
        item.finished += s.finished;
        *secs += s.secs as f64;
    };
    let mut decades: HashMap<u32, (u32, f64)> = HashMap::new();
    let mut languages: HashMap<crate::songlang::Lang, (u32, f64)> = HashMap::new();
    for (&t, s) in &range_songs {
        let Some(tr) = track(t) else { continue };
        out.plays += s.plays;
        out.finished += s.finished;
        out.skipped += s.skipped;
        seconds += s.secs as f64;
        add(&mut songs, &tr.id, &tr.title, &tr.artist, s);
        let album = album_of.get(tr.album_id.as_str());
        let credit = album.map_or(tr.album_artist.as_str(), |a| a.artist.as_str());
        for n in artists_of(&tr.artist, credit) {
            add(&mut artists, &n, &n, "", s);
        }
        if let Some(a) = album {
            add(&mut albums, &a.id, &a.title, &a.artist, s);
        }
        if let Some(y) = tr.year.filter(|y| (1000..=2999).contains(y)) {
            let d = decades.entry(y / 10 * 10).or_default();
            d.0 += s.plays - s.skipped.min(s.plays);
            d.1 += s.secs as f64;
        }
        if let Some(l) = lang_of.get(tr.id.as_str()) {
            let d = languages.entry(*l).or_default();
            d.0 += s.plays - s.skipped.min(s.plays);
            d.1 += s.secs as f64;
        }
    }
    out.minutes = (seconds / 60.0).round() as u32;
    if window.is_some() {
        let n = window.map_or(0, |w| w.buckets);
        let mut tl = vec![0.0f64; n];
        for a in &ranged {
            for (i, v) in a.timeline.iter().enumerate().take(n) {
                tl[i] += v;
            }
        }
        out.timeline = tl.iter().map(|s| (s / 60.0).round() as u32).collect();
    }
    out.songs = songs.len() as u32;
    out.artists = artists.len() as u32;
    out.albums = albums.len() as u32;
    let top = |map: HashMap<String, (StatItem, f64)>| {
        let mut v: Vec<StatItem> = map
            .into_values()
            .map(|(mut s, secs)| {
                s.minutes = (secs / 60.0).round() as u32;
                s
            })
            .collect();
        v.sort_by(|a, b| b.finished.cmp(&a.finished).then(b.minutes.cmp(&a.minutes)).then_with(|| a.name.cmp(&b.name)));
        v.truncate(50);
        v
    };
    out.top_songs = top(songs);
    out.top_artists = top(artists);
    out.top_albums = top(albums);

    // ----- listening clock and weekdays (the range) -----
    let mut clock = [0u32; 24];
    let mut weekdays = [0u32; 7];
    for a in &ranged {
        for i in 0..24 {
            clock[i] += a.hours[i];
        }
        for i in 0..7 {
            weekdays[i] += a.weekdays[i];
        }
    }
    out.clock = clock.to_vec();
    out.weekdays = weekdays.to_vec();

    // ----- decades (the range) -----
    let mut dec: Vec<Decade> = decades.into_iter().map(|(decade, (plays, secs))| Decade { decade, plays, minutes: (secs / 60.0).round() as u32 }).collect();
    dec.sort_by_key(|d| d.decade);
    out.decades = dec;

    // ----- languages (the range), most played first -----
    let mut langs: Vec<Language> = languages
        .into_iter()
        .filter(|(_, (plays, _))| *plays > 0)
        .map(|(l, (plays, secs))| Language { code: l.code().into(), name: l.name().into(), plays, minutes: (secs / 60.0).round() as u32 })
        .collect();
    langs.sort_by(|a, b| b.plays.cmp(&a.plays).then_with(|| a.code.cmp(&b.code)));
    out.languages = langs;

    // ----- albums heard through (the range) -----
    let mut whole: HashMap<&str, u32> = HashMap::new();
    for a in &ranged {
        for (album, heard) in a.all_runs() {
            let id = names.name(album);
            let Some(al) = album_of.get(id) else { continue };
            let n = al.track_ids.len();
            // Most of an album of three songs or more, heard to the end in one go.
            if n >= 3 && heard as usize * 5 >= n * 4 {
                *whole.entry(id).or_default() += 1;
            }
        }
    }
    let mut complete: Vec<StatItem> = whole
        .into_iter()
        .filter_map(|(id, n)| {
            let a = album_of.get(id)?;
            Some(StatItem { id: a.id.clone(), name: a.title.clone(), sub: a.artist.clone(), finished: n, plays: n, minutes: (a.duration * n as f64 / 60.0).round() as u32 })
        })
        .collect();
    complete.sort_by(|a, b| b.finished.cmp(&a.finished).then_with(|| a.name.cmp(&b.name)));
    complete.truncate(20);
    out.complete_albums = complete;

    // ----- all-time sections -----
    let all_songs = songs_of(&everything);
    let today = (now as i64 + offset).div_euclid(DAY);
    let (this_year, this_month, this_day) = civil(today);
    out.history_from = everything.iter().filter(|a| a.min_at > 0).map(|a| a.min_at).min().unwrap_or(0);

    // On this day, in earlier years.
    let mut on_day: HashMap<(i64, u32), u32> = HashMap::new();
    for a in &everything {
        for (day, list) in &a.dates {
            let (y, m, d) = civil(*day as i64);
            if m == this_month && d == this_day && y < this_year {
                for (t, n) in list {
                    *on_day.entry((y, *t)).or_default() += *n as u32;
                }
            }
        }
    }
    let mut on: Vec<DayItem> = on_day
        .into_iter()
        .filter_map(|((year, t), plays)| track(t).map(|tr| DayItem { id: tr.id.clone(), name: tr.title.clone(), sub: tr.artist.clone(), year, plays }))
        .collect();
    on.sort_by(|a, b| b.year.cmp(&a.year).then(b.plays.cmp(&a.plays)).then_with(|| a.name.cmp(&b.name)));
    let mut per_year: HashMap<i64, usize> = HashMap::new();
    on.retain(|d| {
        let n = per_year.entry(d.year).or_default();
        *n += 1;
        *n <= 10
    });
    on.truncate(30);
    out.on_this_day = on;

    // Forgotten favourites.
    let marks = state.marks.lock().view();
    let mut forgotten: Vec<(u32, Forgotten)> = all_songs
        .iter()
        .filter(|(_, s)| now.saturating_sub(s.last as u64) >= FORGOTTEN_AFTER)
        .filter_map(|(&t, s)| {
            let tr = track(t)?;
            let m = marks.get(&tr.id).copied().unwrap_or_default();
            (s.finished >= 5 || m.loved || m.rating >= 4).then(|| {
                let score = s.finished + if m.loved { 10 } else { 0 } + m.rating as u32 * 2;
                (score, Forgotten { id: tr.id.clone(), name: tr.title.clone(), sub: tr.artist.clone(), plays: s.finished, last: s.last as u64, loved: m.loved, rating: m.rating })
            })
        })
        .collect();
    forgotten.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.last.cmp(&a.1.last)).then_with(|| a.1.name.cmp(&b.1.name)));
    out.forgotten = forgotten.into_iter().take(30).map(|(_, f)| f).collect();

    // Streaks of days with listening.
    let mut listened: Vec<i64> = everything.iter().flat_map(|a| a.days.iter().filter(|(_, s)| **s > 0.0).map(|(d, _)| *d as i64)).collect();
    listened.sort_unstable();
    listened.dedup();
    if !listened.is_empty() {
        let (mut best, mut best_end, mut run) = (0u32, 0i64, 0u32);
        for (i, d) in listened.iter().enumerate() {
            run = if i > 0 && listened[i - 1] == d - 1 { run + 1 } else { 1 };
            if run > best {
                best = run;
                best_end = *d;
            }
        }
        let last = *listened.last().unwrap_or(&0);
        let current = if last >= today - 1 { run } else { 0 };
        out.streak = Some(Streaks {
            current,
            longest: best,
            longest_from: day_name(best_end - best as i64 + 1),
            longest_to: day_name(best_end),
            days: listened.len() as u32,
        });
    }

    // New artists per month (the month each was first heard), the last 12 months; the first
    // month of the history is left out (everything is new then).
    let mut first_heard: HashMap<String, u32> = HashMap::new();
    for (&t, s) in &all_songs {
        let Some(tr) = track(t) else { continue };
        let credit = album_of.get(tr.album_id.as_str()).map_or(tr.album_artist.as_str(), |a| a.artist.as_str());
        for n in artists_of(&tr.artist, credit) {
            let f = first_heard.entry(n.to_lowercase()).or_insert(s.first);
            *f = (*f).min(s.first);
        }
    }
    let month_of = |secs: u64| {
        let (y, m, _) = civil((secs as i64 + offset).div_euclid(DAY));
        y * 12 + m as i64 - 1
    };
    if out.history_from > 0 {
        let start = month_of(out.history_from) + 1;
        let now_month = this_year * 12 + this_month as i64 - 1;
        let from = start.max(now_month - 11);
        if from <= now_month {
            let mut counts: HashMap<i64, u32> = HashMap::new();
            for f in first_heard.values() {
                *counts.entry(month_of(*f as u64)).or_default() += 1;
            }
            out.discovery = (from..=now_month)
                .map(|m| MonthCount { month: format!("{}-{:02}", m.div_euclid(12), m.rem_euclid(12) + 1), count: counts.get(&m).copied().unwrap_or(0) })
                .collect();
        }
    }

    // The first song of this year.
    out.first_song = everything
        .iter()
        .filter_map(|a| a.firsts.get(&(this_year as i32)))
        .min_by_key(|(at, _)| *at)
        .and_then(|&(at, t)| track(t).map(|tr| FirstSong { id: tr.id.clone(), name: tr.title.clone(), sub: tr.artist.clone(), at }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Track;

    struct Quiet;
    impl crate::ctx::Host for Quiet {
        fn emit(&self, _: &str, _: serde_json::Value) {}
    }

    fn core(name: &str) -> (crate::ctx::Ctx, PathBuf) {
        let base = std::env::temp_dir().join(format!("cadence-stats-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (ctx, _) = crate::hub::open(base.join("data"), &base.join("local"), std::sync::Arc::new(Quiet));
        (ctx, base)
    }

    fn play(id: &str, at: u64) -> PlayEvent {
        PlayEvent { track_id: id.into(), album_id: format!("al-{id}"), at, listened: 200.0, duration: 200.0, skipped: false }
    }

    fn line(e: &PlayEvent) -> String {
        format!("{}\n", serde_json::to_string(e).unwrap())
    }

    fn library(ctx: &crate::ctx::Ctx, ids: &[&str]) {
        let tracks: Vec<Track> = ids
            .iter()
            .map(|id| Track { id: id.to_string(), title: format!("Song {id}"), artist: "Artist".into(), album_artist: "Artist".into(), album_id: format!("al-{id}"), duration: 200.0, year: Some(1995), ..Default::default() })
            .collect();
        ctx.state.library.lock().set_tracks(tracks, Vec::new());
    }

    #[test]
    fn plays_in_the_archive_and_history_json_count_once_and_damaged_lines_are_skipped() {
        let (ctx, base) = core("dupes");
        let data = base.join("data");
        library(&ctx, &["a", "b", "c"]);
        let dir = crate::history::archive_dir(&data);
        std::fs::create_dir_all(&dir).unwrap();
        let t0 = 1_700_000_000;
        // In 2023's file: a, b, a damaged line, and b again (archived twice after a crash).
        std::fs::write(dir.join("2023.jsonl"), format!("{}{}{{\"trackId\": \"x\n{}", line(&play("a", t0)), line(&play("b", t0 + 300)), line(&play("b", t0 + 300)))).unwrap();
        // history.json still has b (archived just before a crash), and c.
        ctx.state.history.lock().events = vec![play("b", t0 + 300), play("c", t0 + 600)];
        let s = listening_stats(&ctx.state, "all".into());
        assert_eq!(s.plays, 3, "a, b and c once each");
        assert_eq!(s.finished, 3);
        assert_eq!(s.top_songs.len(), 3);
        assert!(s.top_songs.iter().all(|t| t.plays == 1));
        assert_eq!(s.decades.len(), 1);
        assert_eq!(s.decades[0].decade, 1990);
        assert_eq!(s.clock.iter().sum::<u32>(), 3);
        assert_eq!(s.weekdays.iter().sum::<u32>(), 3);
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_archive_is_read_again_only_when_it_changes() {
        let (ctx, base) = core("cache");
        let data = base.join("data");
        library(&ctx, &["a", "b"]);
        let dir = crate::history::archive_dir(&data);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("2023.jsonl");
        std::fs::write(&f, line(&play("a", 1_700_000_000))).unwrap();
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 1);
        let ends = || CACHES.lock().as_ref().and_then(|c| c.get(&data).map(|c| c.years.values().map(|y| y.end).sum::<u64>())).unwrap_or(0);
        let first_end = ends();
        assert!(first_end > 0);
        // Unchanged: the summary is used as it is.
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 1);
        assert_eq!(ends(), first_end);
        // The archive grows: the new play counts (only the new line is read).
        let mut more = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        std::io::Write::write_all(&mut more, line(&play("b", 1_700_000_500)).as_bytes()).unwrap();
        drop(more);
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 2);
        assert!(ends() > first_end);
        // A line still being written isn't counted until it's whole.
        let mut more = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        std::io::Write::write_all(&mut more, b"{\"trackId\":\"a\",\"albumId\":\"al-a\",").unwrap();
        drop(more);
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 2);
        let mut more = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        std::io::Write::write_all(&mut more, b"\"at\":1700000900,\"listened\":200.0,\"duration\":200.0,\"skipped\":false}\n").unwrap();
        drop(more);
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 3);
        // Rewritten shorter (a restore): read again from the start.
        std::fs::write(&f, line(&play("a", 1_700_000_000))).unwrap();
        invalidate(&data);
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 1);
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn plays_added_in_the_middle_of_a_year_file_are_noticed() {
        let (ctx, base) = core("merged");
        let data = base.join("data");
        library(&ctx, &["a", "b", "c"]);
        let dir = crate::history::archive_dir(&data);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2023.jsonl"), format!("{}{}", line(&play("a", 1_700_000_000)), line(&play("b", 1_700_000_900)))).unwrap();
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 2);
        // A play older than the file's last is put in the middle: the file grows, but not at its end.
        let m = crate::history::merge(&ctx.state.history, &data, vec![play("c", 1_700_000_300)], || Ok(())).unwrap();
        assert_eq!(m.added, 1);
        // (Not enough plays for history.json to be full: it holds the new one, the archive is untouched.)
        assert_eq!(listening_stats(&ctx.state, "all".into()).plays, 3);
        let mut full = crate::history::History::default();
        full.events = (0..20_000u64).map(|i| play("c", 1_800_000_000 + i * 5)).collect();
        *ctx.state.history.lock() = full;
        let m = crate::history::merge(&ctx.state.history, &data, vec![play("a", 1_700_000_500)], || Ok(())).unwrap();
        assert_eq!((m.added, m.archived), (1, 1));
        let s = listening_stats(&ctx.state, "all".into());
        assert_eq!(s.plays, 20_000 + 3, "the year file's three plays and the full window");
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn new_sections_from_the_whole_history() {
        let (ctx, base) = core("sections");
        let data = base.join("data");
        library(&ctx, &["old", "fav", "new"]);
        let now = crate::history::now();
        let year = 365 * 86_400;
        let dir = crate::history::archive_dir(&data);
        std::fs::create_dir_all(&dir).unwrap();
        // A year ago today: "old" played; "fav" played 6 times a year ago and never since.
        let a_year_ago = now - year - 3600;
        let mut text = line(&play("old", a_year_ago));
        for i in 0..6 {
            text.push_str(&line(&play("fav", a_year_ago - 86_400 * 2 + i * 400)));
        }
        std::fs::write(dir.join("2000.jsonl"), text).unwrap();
        // Three days in a row up to today.
        ctx.state.history.lock().events = vec![play("new", now - 2 * 86_400), play("new", now - 86_400), play("new", now - 10)];
        let s = listening_stats(&ctx.state, "all".into());
        assert!(s.forgotten.iter().any(|f| f.id == "fav" && f.plays == 6), "{:?}", s.forgotten);
        assert!(!s.forgotten.iter().any(|f| f.id == "new"));
        let streak = s.streak.clone().expect("streaks");
        assert!(streak.current >= 2 && streak.longest >= streak.current, "{streak:?}");
        assert!(s.history_from > 0 && s.history_from <= a_year_ago);
        assert!(s.first_song.is_some());
        // On this day (local dates: tolerate the local clock putting it a day off).
        if !s.on_this_day.is_empty() {
            assert!(s.on_this_day.iter().all(|d| d.year < civil((now as i64 + local_offset()).div_euclid(DAY)).0));
        }
        // The week range reads history.json only.
        let w = listening_stats(&ctx.state, "week".into());
        assert_eq!(w.plays, 3);
        assert_eq!(w.timeline.len(), 7);
        // The year range reaches into the archive for what history.json doesn't have.
        let y = listening_stats(&ctx.state, "year".into());
        assert!(y.plays >= 3);
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn albums_heard_through_and_empty_history() {
        let (ctx, base) = core("albums");
        let data = base.join("data");
        let tracks: Vec<Track> = (0..4).map(|i| Track { id: format!("t{i}"), title: format!("T{i}"), artist: "A".into(), album_artist: "A".into(), album_id: "al".into(), album: "Al".into(), duration: 200.0, ..Default::default() }).collect();
        let mut tracks = tracks;
        let albums = crate::library::build_albums(&mut tracks, &data.join("art"));
        ctx.state.library.lock().set_tracks(tracks, albums);
        let album_id = ctx.state.library.lock().snapshot.tracks[0].album_id.clone();
        let empty = listening_stats(&ctx.state, "all".into());
        assert_eq!(empty.plays, 0);
        assert!(empty.on_this_day.is_empty() && empty.forgotten.is_empty() && empty.streak.is_none() && empty.first_song.is_none());
        assert!(empty.complete_albums.is_empty() && empty.discovery.is_empty() && empty.decades.is_empty());
        let now = crate::history::now();
        let ev = |i: usize, at: u64| PlayEvent { track_id: format!("t{i}"), album_id: album_id.clone(), at, listened: 200.0, duration: 200.0, skipped: false };
        ctx.state.history.lock().events = (0..4).map(|i| ev(i, now - 4000 + i as u64 * 200)).collect();
        let s = listening_stats(&ctx.state, "month".into());
        assert_eq!(s.complete_albums.len(), 1, "four of four songs in one go");
        assert_eq!(s.complete_albums[0].finished, 1);
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn one_artist_under_two_spellings_counts_once() {
        crate::identity::set_for_test(Some(crate::identity::Identity::from_pairs(&[("米津玄師", "Kenshi Yonezu")])));
        let (ctx, base) = core("spellings");
        let data = base.join("data");
        let song = |i: usize, artist: &str, album: &str| Track { id: format!("t{i}"), title: format!("T{i}"), artist: artist.into(), album_artist: artist.into(), album: album.into(), album_id: format!("{artist}|{album}"), duration: 200.0, ..Default::default() };
        let mut tracks = vec![song(0, "米津玄師", "BOOTLEG"), song(1, "米津玄師", "BOOTLEG"), song(2, "Kenshi Yonezu", "STRAY SHEEP"), song(3, "Kenshi Yonezu", "STRAY SHEEP"), song(4, "Kenshi Yonezu", "STRAY SHEEP"), song(5, "YOASOBI", "THE BOOK")];
        let albums = crate::library::build_albums(&mut tracks, &data.join("art"));
        ctx.state.library.lock().set_tracks(tracks, albums);
        let now = crate::history::now();
        let album_of = |i: usize| ctx.state.library.lock().snapshot.tracks[i].album_id.clone();
        let events: Vec<PlayEvent> = (0..6).map(|i| PlayEvent { track_id: format!("t{i}"), album_id: album_of(i), at: now - 10_000 + i as u64 * 300, listened: 200.0, duration: 200.0, skipped: false }).collect();
        ctx.state.history.lock().events = events;
        let s = listening_stats(&ctx.state, "all".into());
        assert_eq!(s.artists, 2, "{:?}", s.top_artists.iter().map(|a| (&a.name, a.plays)).collect::<Vec<_>>());
        assert_eq!(s.top_artists[0].name, "Kenshi Yonezu", "the spelling on most songs");
        assert_eq!(s.top_artists[0].plays, 5);
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
        crate::identity::set_for_test(None);
    }

    #[test]
    fn plays_by_language() {
        let (ctx, base) = core("languages");
        let data = base.join("data");
        // Invented titles: kana (Japanese), Hangul (Korean), and a Latin one with nothing to go on.
        let song = |i: usize, title: &str| Track { id: format!("t{i}"), title: title.into(), artist: format!("A{i}"), album_artist: format!("A{i}"), album_id: format!("al{i}"), duration: 200.0, ..Default::default() };
        ctx.state.library.lock().set_tracks(vec![song(0, "よるのうた"), song(1, "밤하늘의 노래"), song(2, "Aurora")], Vec::new());
        let now = crate::history::now();
        ctx.state.history.lock().events = vec![play("t0", now - 900), play("t0", now - 600), play("t1", now - 300), play("t2", now - 100)];
        let s = listening_stats(&ctx.state, "all".into());
        let langs: Vec<(&str, u32)> = s.languages.iter().map(|l| (l.code.as_str(), l.plays)).collect();
        assert_eq!(langs, [("ja", 2), ("ko", 1)], "the unknown one isn't counted");
        assert_eq!(s.languages[0].name, "Japanese");
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `cargo test --lib stats::tests::cost -- --nocapture`: the time and memory for ~200,000 plays.
    #[test]
    fn cost_of_200000_plays() {
        let (ctx, base) = core("cost");
        let data = base.join("data");
        let ids: Vec<String> = (0..8000).map(|i| format!("{:016x}", i * 7919)).collect();
        let tracks: Vec<Track> = ids.iter().map(|id| Track { id: id.clone(), title: id.clone(), artist: format!("Artist {}", &id[..2]), album_artist: "A".into(), album_id: format!("al{}", &id[..3]), duration: 200.0, year: Some(1990 + (id.len() as u32 % 30)), ..Default::default() }).collect();
        ctx.state.library.lock().set_tracks(tracks, Vec::new());
        let dir = crate::history::archive_dir(&data);
        std::fs::create_dir_all(&dir).unwrap();
        let start = 1_600_000_000u64;
        let mut n = 0u64;
        for year in 0..5u64 {
            let mut text = String::new();
            for i in 0..36_000u64 {
                let at = start + year * 31_536_000 + i * 870;
                text.push_str(&line(&PlayEvent { track_id: ids[(n * 31 % 8000) as usize].clone(), album_id: "al".into(), at, listened: 180.0, duration: 200.0, skipped: n % 9 == 0 }));
                n += 1;
            }
            std::fs::write(dir.join(format!("{}.jsonl", crate::stats::civil((start as i64 + year as i64 * 31_536_000) / DAY).0)), text).unwrap();
        }
        let last = start + 5 * 31_536_000;
        ctx.state.history.lock().events = (0..20_000u64).map(|i| PlayEvent { track_id: ids[(i * 17 % 8000) as usize].clone(), album_id: "al".into(), at: last + i * 600, listened: 180.0, duration: 200.0, skipped: false }).collect();
        let t = std::time::Instant::now();
        let s = listening_stats(&ctx.state, "all".into());
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let s2 = listening_stats(&ctx.state, "all".into());
        let warm = t.elapsed();
        assert_eq!(s.plays, s2.plays);
        assert_eq!(s.plays, 200_000);
        let bytes: usize = CACHES.lock().as_ref().unwrap()[&data].years.values().map(|y| y.agg.approx_bytes()).sum();
        let names: usize = CACHES.lock().as_ref().unwrap()[&data].names.all.iter().map(|s| s.len() * 2 + 80).sum();
        println!("200,000 plays: first {cold:?}, then {warm:?}; summaries ~{} KB + ids ~{} KB", bytes / 1024, names / 1024);
        invalidate(&data);
        let _ = std::fs::remove_dir_all(&base);
    }
}
