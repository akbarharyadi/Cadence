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

    #[test]
    fn short_flicks_are_ignored() {
        let mut t = Tracker::default();
        let album = |_: &str| "a".to_string();
        t.observe(&status("x", 0.0, true), album);
        t.observe(&status("x", 1.0, true), album);
        assert!(t.observe(&status("y", 0.0, true), album).is_none());
    }
}
