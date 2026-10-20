//! Listening history: what was played, for how long, and whether it was skipped.
//!
//! Built from the engine's status updates, so it records what was actually heard (seeks and
//! pauses don't count as listening time). Stored only on this computer.

use std::collections::HashMap;
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

pub fn push(history: &mut History, event: PlayEvent) {
    history.events.push(event);
    if history.events.len() > MAX_EVENTS {
        let extra = history.events.len() - MAX_EVENTS;
        history.events.drain(..extra);
    }
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

    #[test]
    fn short_flicks_are_ignored() {
        let mut t = Tracker::default();
        let album = |_: &str| "a".to_string();
        t.observe(&status("x", 0.0, true), album);
        t.observe(&status("x", 1.0, true), album);
        assert!(t.observe(&status("y", 0.0, true), album).is_none());
    }
}
