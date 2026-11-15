//! Listening history: what was played, for how long, and whether it was skipped.
//!
//! Built from the engine's status updates, so it records what was actually heard (seeks and
//! pauses don't count as listening time). Stored only on this computer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::audio::Status;

/// Plays shorter than this aren't recorded at all (flicking through the queue).
const MIN_LISTEN_SECS: f64 = 5.0;
/// A song left within this time (and before its halfway point) counts as skipped.
const SKIP_SECS: f64 = 30.0;
const MAX_EVENTS: usize = 20_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayEvent {
    pub track_id: String,
    pub album_id: String,
    /// Unix seconds when the song started.
    pub at: u64,
    /// Seconds actually listened to.
    pub listened: f64,
    pub duration: f64,
    pub skipped: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct History {
    pub events: Vec<PlayEvent>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStats {
    /// Completed (not skipped) plays per track.
    pub plays: HashMap<String, u32>,
    pub skips: HashMap<String, u32>,
    /// Unix seconds of the last play per track.
    pub last_played: HashMap<String, u64>,
    /// Album ids, most recently played first.
    pub recent_albums: Vec<String>,
    pub total_listened: f64,
}

impl History {
    pub fn stats(&self) -> HistoryStats {
        let mut s = HistoryStats::default();
        for e in &self.events {
            if e.skipped {
                *s.skips.entry(e.track_id.clone()).or_default() += 1;
            } else {
                *s.plays.entry(e.track_id.clone()).or_default() += 1;
            }
            s.last_played.insert(e.track_id.clone(), e.at);
            s.total_listened += e.listened;
        }
        for e in self.events.iter().rev() {
            if !s.recent_albums.contains(&e.album_id) {
                s.recent_albums.push(e.album_id.clone());
                if s.recent_albums.len() >= 30 {
                    break;
                }
            }
        }
        s
    }
}

struct Current {
    track_id: String,
    album_id: String,
    at: u64,
    listened: f64,
    last_pos: f64,
    duration: f64,
}

/// Turns the stream of status updates into play events.
#[derive(Default)]
pub struct Tracker {
    current: Option<Current>,
}

impl Tracker {
    /// Feeds one status update; returns a finished play when the song changed.
    pub fn observe(&mut self, s: &Status, album_of: impl Fn(&str) -> String) -> Option<PlayEvent> {
        let id = s.index.and_then(|i| s.queue.get(i)).cloned();
        let same = matches!((&self.current, &id), (Some(c), Some(id)) if &c.track_id == id);
        // A restart of the same song (position jumped back to the start) is a new play.
        let restarted = same
            && self
                .current
                .as_ref()
                .is_some_and(|c| s.position + 2.0 < c.last_pos && s.position < 2.0);
        if same && !restarted {
            let c = self.current.as_mut().unwrap();
            let delta = s.position - c.last_pos;
            // Normal progress only; seeks and pauses don't add listening time.
            if s.playing && delta > 0.0 && delta < 2.0 {
                c.listened += delta;
            }
            c.last_pos = s.position;
            return None;
        }
        let finished = self.finish();
        self.current = id.map(|track_id| Current {
            album_id: album_of(&track_id),
            track_id,
            at: now(),
            listened: 0.0,
            last_pos: s.position,
            duration: s.duration,
        });
        finished
    }

    /// Closes the current play (song changed, or the app is quitting).
    pub fn finish(&mut self) -> Option<PlayEvent> {
        let c = self.current.take()?;
        if c.listened < MIN_LISTEN_SECS {
            return None;
        }
        let skipped =
            c.listened < SKIP_SECS && (c.duration <= 0.0 || c.listened < c.duration / 2.0);
        Some(PlayEvent {
            track_id: c.track_id,
            album_id: c.album_id,
            at: c.at,
            listened: c.listened,
            duration: c.duration,
            skipped,
        })
    }
}

/// Adds a play. history.json keeps the last `MAX_EVENTS`; older plays move to the archive in
/// `archive` (one file per year, see `archive_dir`) before they leave it. If the archive can't be
/// written they stay in history.json for now, so a play is never lost.
pub fn push(history: &mut History, event: PlayEvent, archive: &Path) {
    history.events.push(event);
    if history.events.len() > MAX_EVENTS {
        let extra = history.events.len() - MAX_EVENTS;
        match append_to_archive(archive, &history.events[..extra]) {
            Ok(()) => {
                history.events.drain(..extra);
            }
            Err(e) => log::error!("can't archive old plays in {}: {e}; kept in the history for now", archive.display()),
        }
    }
}

/// The folder of older plays beside history.json: `history-archive/2026.jsonl`, one play per line.
pub fn archive_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("history-archive")
}

/// Appends plays to the year files they belong in (by when they started, UTC), synced to the disk.
fn append_to_archive(dir: &Path, events: &[PlayEvent]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut by_year: std::collections::BTreeMap<i64, Vec<u8>> = Default::default();
    for e in events {
        let line = by_year.entry(year_of(e.at)).or_default();
        serde_json::to_writer(&mut *line, e).map_err(std::io::Error::other)?;
        line.push(b'\n');
    }
    for (year, lines) in by_year {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{year}.jsonl")))?;
        f.write_all(&lines)?;
        f.sync_all()?;
    }
    Ok(())
}

/// The calendar year (UTC) of a unix time.
fn year_of(secs: u64) -> i64 {
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(month <= 2)
}

// ---------------------------------------------------------------------------------------------
// Adding plays from elsewhere (a listening history restored from ListenBrainz)
// ---------------------------------------------------------------------------------------------

/// Two plays of one song starting this close together are one play (the same listen, as this
/// computer and ListenBrainz each noted its time).
pub const SAME_PLAY_SECS: u64 = 10;

/// The plays already there, per song: their start times.
#[derive(Clone, Default)]
pub struct Known(HashMap<String, std::collections::BTreeSet<u64>>);

impl Known {
    pub fn add(&mut self, track: &str, at: u64) {
        self.0.entry(track.to_string()).or_default().insert(at);
    }
    pub fn has(&self, track: &str, at: u64) -> bool {
        self.0.get(track).is_some_and(|s| s.range(at.saturating_sub(SAME_PLAY_SECS)..=at + SAME_PLAY_SECS).next().is_some())
    }
}

/// The archive's plays in the years `events` start in (and the years next to them: a play near the
/// turn of the year).
pub fn known_in_archive(data_dir: &Path, events: &[PlayEvent]) -> Known {
    let mut years = std::collections::BTreeSet::new();
    for e in events {
        years.insert(year_of(e.at.saturating_sub(SAME_PLAY_SECS)));
        years.insert(year_of(e.at + SAME_PLAY_SECS));
    }
    let mut known = Known::default();
    for y in years {
        let file = archive_dir(data_dir).join(format!("{y}.jsonl"));
        if file.exists() {
            let _ = read_archive_from(&file, 0, |e| known.add(&e.track_id, e.at));
        }
    }
    known
}

/// The plays of `incoming` that are new (oldest first), and how many were already there: in
/// `known`, or offered twice. Nothing is left in `known` of this call's plays but what it adds.
pub fn new_plays(mut incoming: Vec<PlayEvent>, known: &mut Known) -> (Vec<PlayEvent>, usize) {
    incoming.sort_by_key(|e| e.at);
    let mut add = Vec::with_capacity(incoming.len());
    let mut already = 0;
    for e in incoming {
        if known.has(&e.track_id, e.at) {
            already += 1;
        } else {
            known.add(&e.track_id, e.at);
            add.push(e);
        }
    }
    (add, already)
}

/// What `merge` did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Merged {
    pub added: usize,
    /// Of those, plays now in history.json…
    pub live: usize,
    /// …and plays now in the archive's year files (older than the window's plays).
    pub archived: usize,
    pub already: usize,
}

fn known_with_live(archive: &Known, live: &[PlayEvent]) -> Known {
    let mut k = archive.clone();
    for e in live {
        k.add(&e.track_id, e.at);
    }
    k
}

/// How many of `incoming` would be added, and how many are already there (the dry run: nothing is
/// written; the archive is read before the history's lock is taken).
pub fn count_new(data_dir: &Path, history: &parking_lot::Mutex<History>, incoming: Vec<PlayEvent>) -> (usize, usize) {
    let archive = known_in_archive(data_dir, &incoming);
    let mut known = known_with_live(&archive, &history.lock().events);
    let (add, already) = new_plays(incoming, &mut known);
    (add.len(), already)
}

/// Adds plays from elsewhere. Those already there (the same song within `SAME_PLAY_SECS`, in
/// history.json or the archive, or twice in `incoming`) are skipped. The rest are put where they
/// belong: history.json keeps the newest `MAX_EVENTS` plays, so the new ones that are recent enough
/// go into it, in order, and the plays that leave it (the older new ones, and the oldest it had) go
/// into the archive's year files.
///
/// A year file is appended to when plays leave history.json, but a play older than what the file
/// holds can't be appended: such a file is read, the new plays put in their places (ordered by
/// start time, as everything else expects), and written whole through the usual atomic save
/// (`write_file`: a temporary file, synced, then put in place; the old one stays as `.bak`). Damaged
/// lines are kept, first. Stats' summaries of the archive are dropped, so they're read again.
///
/// `before_write` runs once, only when something will be written, before the first write: it
/// takes the safety snapshot; if it fails nothing is written. The archive is written before
/// history.json is changed in memory (the caller saves it), so a failure leaves the history as it
/// was, and a crash in between leaves plays in both places, which readers count once. The history's
/// lock is held for the writing, as `push` holds it for archiving.
pub fn merge(
    history: &parking_lot::Mutex<History>,
    data_dir: &Path,
    incoming: Vec<PlayEvent>,
    before_write: impl FnOnce() -> Result<(), String>,
) -> Result<Merged, String> {
    let archive_known = known_in_archive(data_dir, &incoming);
    // Is there anything to add? (Without the lock held for the archive's snapshot.)
    {
        let live = history.lock();
        let (add, _) = new_plays(incoming.clone(), &mut known_with_live(&archive_known, &live.events));
        if add.is_empty() {
            return Ok(Merged { already: incoming.len(), ..Default::default() });
        }
    }
    before_write()?;
    let mut h = history.lock();
    let (add, already) = new_plays(incoming, &mut known_with_live(&archive_known, &h.events));
    if add.is_empty() {
        return Ok(Merged { already, ..Default::default() });
    }
    let added = add.len();
    let new_keys: std::collections::HashSet<(String, u64)> = add.iter().map(|e| (e.track_id.clone(), e.at)).collect();
    let mut all = std::mem::take(&mut h.events);
    all.extend(add);
    all.sort_by_key(|e| e.at);
    let extra = all.len().saturating_sub(MAX_EVENTS);
    let kept = all.split_off(extra);
    let leaving = all;
    if let Err(e) = rewrite_archive(data_dir, &leaving) {
        // As it was: the old events, without the new ones.
        h.events = leaving.into_iter().chain(kept).filter(|e| !new_keys.contains(&(e.track_id.clone(), e.at))).collect();
        return Err(format!("The archive couldn't be written ({e}); your history is as it was."));
    }
    let archived = leaving.iter().filter(|e| new_keys.contains(&(e.track_id.clone(), e.at))).count();
    h.events = kept;
    drop(h);
    // The archive's year files changed in the middle, not only at the end: forget the summaries.
    crate::stats::invalidate(data_dir);
    Ok(Merged { added, live: added - archived, archived, already })
}

/// Puts plays into the year files they belong in, ordered by start time, skipping the ones a file
/// already has. Only files that change are written.
fn rewrite_archive(data_dir: &Path, events: &[PlayEvent]) -> std::io::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let dir = archive_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let mut by_year: std::collections::BTreeMap<i64, Vec<&PlayEvent>> = Default::default();
    for e in events {
        by_year.entry(year_of(e.at)).or_default().push(e);
    }
    for (year, new) in by_year {
        let file = dir.join(format!("{year}.jsonl"));
        let text = match std::fs::read(&file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        let mut damaged: Vec<&[u8]> = Vec::new();
        let mut valid: Vec<(u64, Vec<u8>)> = Vec::new();
        let mut have = std::collections::HashSet::new();
        for line in text.split(|b| *b == b'\n').filter(|l| !l.iter().all(u8::is_ascii_whitespace)) {
            match serde_json::from_slice::<PlayEvent>(line) {
                Ok(e) => {
                    have.insert((e.track_id, e.at));
                    valid.push((e.at, line.to_vec()));
                }
                Err(_) => damaged.push(line),
            }
        }
        let before = valid.len();
        for e in new {
            if have.insert((e.track_id.clone(), e.at)) {
                valid.push((e.at, serde_json::to_vec(e).map_err(std::io::Error::other)?));
            }
        }
        if valid.len() == before {
            continue;
        }
        valid.sort_by_key(|(at, _)| *at);
        let mut out = Vec::with_capacity(text.len() + 160 * (valid.len() - before));
        for l in damaged {
            out.extend_from_slice(l);
            out.push(b'\n');
        }
        for (_, l) in valid {
            out.extend_from_slice(&l);
            out.push(b'\n');
        }
        crate::state::write_file(&file, &out, true)?;
    }
    Ok(())
}

/// Every play ever recorded, oldest first: the archive, then the plays still in history.json
/// (a play in both, archived just before a crash, counts once). Damaged lines are skipped.
/// (For features that look further back than history.json: nothing uses it yet.)
#[allow(dead_code)]
pub fn all_events(data_dir: &Path, live: &History) -> Vec<PlayEvent> {
    let mut years: Vec<PathBuf> = std::fs::read_dir(archive_dir(data_dir))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    years.sort();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for file in years {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(e) = serde_json::from_str::<PlayEvent>(line)
                && seen.insert((e.track_id.clone(), e.at)) {
                    out.push(e);
                }
        }
    }
    out.extend(live.events.iter().filter(|e| seen.insert((e.track_id.clone(), e.at))).cloned());
    out.sort_by_key(|e| e.at);
    out
}

/// Reads an archive year file line by line from byte `from`, handing each play to `f` (damaged
/// lines are skipped, never the whole file). Returns where the last whole line ends: a line still
/// being written (no newline yet) is left for the next read. Nothing but the current line is held.
pub fn read_archive_from(path: &Path, from: u64, mut f: impl FnMut(PlayEvent)) -> std::io::Result<u64> {
    use std::io::{BufRead, Seek};
    let file = std::fs::File::open(path)?;
    let mut r = std::io::BufReader::with_capacity(64 * 1024, file);
    r.seek(std::io::SeekFrom::Start(from))?;
    let mut end = from;
    let mut line = Vec::with_capacity(256);
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        end += n as u64;
        if let Ok(e) = serde_json::from_slice::<PlayEvent>(&line) {
            f(e);
        }
    }
    Ok(end)
}

/// The archive's year files (`2025.jsonl`, …), oldest first.
pub fn archive_files(data_dir: &Path) -> Vec<PathBuf> {
    let mut years: Vec<PathBuf> = std::fs::read_dir(archive_dir(data_dir))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    years.sort();
    years
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(id: &str, pos: f64, playing: bool) -> Status {
        Status {
            queue: vec![id.into()],
            index: Some(0),
            playing,
            position: pos,
            duration: 200.0,
            ..Default::default()
        }
    }

    #[test]
    fn counts_listening_and_skips() {
        let mut t = Tracker::default();
        let album = |_: &str| "a".to_string();
        assert!(t.observe(&status("x", 0.0, true), album).is_none());
        let mut pos = 0.0;
        while pos < 12.0 {
            pos += 0.25;
            t.observe(&status("x", pos, true), album);
        }
        // Seek far ahead: not counted as listening.
        t.observe(&status("x", 150.0, true), album);
        let e = t
            .observe(&status("y", 0.0, true), album)
            .expect("play recorded");
        assert!((e.listened - 12.0).abs() < 0.3, "{}", e.listened);
        assert!(e.skipped);
    }

    fn play(id: &str, at: u64) -> PlayEvent {
        PlayEvent { track_id: id.into(), album_id: "a".into(), at, listened: 100.0, duration: 200.0, skipped: false }
    }

    #[test]
    fn years_of_unix_times() {
        assert_eq!(year_of(0), 1970);
        assert_eq!(year_of(1_735_689_599), 2024); // 2024-12-31 23:59:59
        assert_eq!(year_of(1_735_689_600), 2025); // 2025-01-01 00:00:00
        assert_eq!(year_of(1_709_164_800), 2024); // 2024-02-29
    }

    #[test]
    fn old_plays_move_to_the_archive_instead_of_being_lost() {
        let dir = std::env::temp_dir().join(format!("cadence-history-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let archive = archive_dir(&dir);
        let mut h = History::default();
        // The window is full; the oldest plays are from 2024 and 2025.
        h.events = (0..MAX_EVENTS as u64).map(|i| play(&format!("t{i}"), 1_735_689_590 + i * 5)).collect();
        push(&mut h, play("new1", 1_800_000_000), &archive);
        push(&mut h, play("new2", 1_800_000_100), &archive);
        push(&mut h, play("new3", 1_800_000_200), &archive);
        assert_eq!(h.events.len(), MAX_EVENTS, "what history.json keeps doesn't change");
        assert_eq!(h.events[0].track_id, "t3");
        let y2024 = std::fs::read_to_string(archive.join("2024.jsonl")).unwrap();
        let y2025 = std::fs::read_to_string(archive.join("2025.jsonl")).unwrap();
        assert_eq!(y2024.lines().count() + y2025.lines().count(), 3);
        assert!(y2024.contains("\"t0\"") && y2024.contains("\"t1\"") && y2025.contains("\"t2\""));
        // A damaged line (a crash mid-write) is skipped; a play in both places counts once.
        std::fs::write(archive.join("2025.jsonl"), format!("{y2025}{{\"trackId\": \"x\n")).unwrap();
        let mut live = History { events: h.events.clone() };
        live.events.insert(0, play("t2", 1_735_689_600));
        let all = all_events(&dir, &live);
        assert_eq!(all.len(), MAX_EVENTS + 3);
        assert_eq!(all[0].track_id, "t0");
        assert_eq!(all.last().unwrap().track_id, "new3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cadence-history-merge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn year_lines(dir: &Path, year: i64) -> Vec<PlayEvent> {
        let mut out = Vec::new();
        let _ = read_archive_from(&archive_dir(dir).join(format!("{year}.jsonl")), 0, |e| out.push(e));
        out
    }

    #[test]
    fn plays_already_there_are_not_added_again() {
        let dir = temp("dupes");
        let t0 = 1_735_689_000; // 2024
        // One play in the archive, one in history.json.
        std::fs::create_dir_all(archive_dir(&dir)).unwrap();
        std::fs::write(archive_dir(&dir).join("2024.jsonl"), format!("{}\n", serde_json::to_string(&play("a", t0)).unwrap())).unwrap();
        let live = parking_lot::Mutex::new(History { events: vec![play("b", t0 + 1000)] });
        let incoming = vec![
            play("a", t0 + 4),        // the archive's play, a few seconds off
            play("b", t0 + 1000 - 9), // history.json's
            play("b", t0 + 1000 + 40), // the same song 40 s later: another play
            play("c", t0 + 2000),
            play("c", t0 + 2000), // offered twice
            play("a", t0 + 500),
        ];
        let (n, already) = count_new(&dir, &live, incoming.clone());
        assert_eq!((n, already), (3, 3));
        let mut snapshots = 0;
        let m = merge(&live, &dir, incoming.clone(), || {
            snapshots += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(snapshots, 1);
        assert_eq!((m.added, m.already), (3, 3), "the dry run's numbers are the real run's");
        assert_eq!(live.lock().events.len(), 4);
        // Again: nothing to add, so no snapshot and no writing.
        let again = merge(&live, &dir, incoming, || panic!("nothing to write: no snapshot")).unwrap();
        assert_eq!((again.added, again.already), (0, 6));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plays_go_where_they_belong_and_stay_in_order() {
        let dir = temp("place");
        let (t2024, t2025) = (1_720_000_000u64, 1_740_000_000u64);
        std::fs::create_dir_all(archive_dir(&dir)).unwrap();
        let existing = format!("{}\n{}\n", serde_json::to_string(&play("a", t2024 + 100)).unwrap(), serde_json::to_string(&play("b", t2024 + 900)).unwrap());
        // A damaged line in the middle stays (first), and the file keeps its other lines as written.
        std::fs::write(archive_dir(&dir).join("2024.jsonl"), format!("{{\"trackId\": \"x\n{existing}")).unwrap();
        // history.json is full: its oldest play is from 2025.
        let live = parking_lot::Mutex::new(History { events: (0..MAX_EVENTS as u64).map(|i| play(&format!("t{i}"), t2025 + i * 5)).collect() });
        let incoming = vec![
            play("n1", t2024 + 500),      // older than everything in the window: the 2024 file, between a and b
            play("n2", t2024 + 50),       // before the file's first
            play("n3", t2025 - 1000),     // 2025 file, older than the window
            play("n4", t2025 + 12),       // inside the window
            play("n5", t2025 + 5 * MAX_EVENTS as u64 + 50), // newer than the window
        ];
        let m = merge(&live, &dir, incoming, || Ok(())).unwrap();
        assert_eq!(m.added, 5);
        // Five plays joined a full window: the five oldest leave it (n4 is among those that stay).
        let h = live.lock();
        assert_eq!(h.events.len(), MAX_EVENTS);
        assert!(h.events.windows(2).all(|w| w[0].at <= w[1].at), "history.json stays in order");
        assert!(h.events.iter().any(|e| e.track_id == "n4") || year_lines(&dir, 2025).iter().any(|e| e.track_id == "n4"));
        assert_eq!(h.events.last().unwrap().track_id, "n5");
        drop(h);
        let y2024 = year_lines(&dir, 2024);
        assert_eq!(y2024.iter().map(|e| e.track_id.as_str()).collect::<Vec<_>>(), ["n2", "a", "n1", "b"]);
        let raw = std::fs::read_to_string(archive_dir(&dir).join("2024.jsonl")).unwrap();
        assert!(raw.starts_with("{\"trackId\": \"x\n"), "the damaged line is kept");
        assert!(archive_dir(&dir).join("2024.jsonl.bak").exists(), "the old file stays as .bak");
        let y2025 = year_lines(&dir, 2025);
        assert!(y2025.iter().any(|e| e.track_id == "n3"));
        assert!(y2025.windows(2).all(|w| w[0].at <= w[1].at));
        // Everything counts once across archive and window, oldest first.
        let all = all_events(&dir, &live.lock());
        assert_eq!(all.len(), MAX_EVENTS + 5 + 2);
        assert!(all.windows(2).all(|w| w[0].at <= w[1].at));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_snapshot_or_archive_leaves_the_history_alone() {
        let dir = temp("fail");
        let live = parking_lot::Mutex::new(History { events: vec![play("a", 1_740_000_000)] });
        let r = merge(&live, &dir, vec![play("z", 1_740_000_500)], || Err("disk full".into()));
        assert_eq!(r, Err("disk full".to_string()));
        assert_eq!(live.lock().events.len(), 1);
        assert!(!archive_dir(&dir).exists(), "nothing was written");
        // The archive can't be written (a file where its folder should be): the new play isn't added.
        let blocked = temp("blocked");
        std::fs::write(archive_dir(&blocked), b"in the way").unwrap();
        let full = parking_lot::Mutex::new(History { events: (0..MAX_EVENTS as u64).map(|i| play(&format!("t{i}"), 1_740_000_000 + i * 5)).collect() });
        let r = merge(&full, &blocked, vec![play("old", 1_700_000_000)], || Ok(()));
        assert!(r.is_err());
        assert_eq!(full.lock().events.len(), MAX_EVENTS);
        assert!(full.lock().events.iter().all(|e| e.track_id != "old"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&blocked);
    }

    #[test]
    fn short_flicks_are_ignored() {
        let mut t = Tracker::default();
        let album = |_: &str| "a".to_string();
        t.observe(&status("x", 0.0, true), album);
        t.observe(&status("x", 1.0, true), album);
        assert!(t.observe(&status("y", 0.0, true), album).is_none());
    }
}
