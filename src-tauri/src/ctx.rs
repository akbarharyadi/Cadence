//! What the core needs from the app around it: the shared state, the voice models, and somewhere
//! to send events, provided by the Tauri app as a `Host`; commands, scans, analysis and voice
//! only go through it.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;
use crate::voice::Voice;

/// The front end: receives events ("library-changed", "player-status", …) as JSON.
pub trait Host: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);
    /// To one window only (by label); hosts with one window send it to that.
    fn emit_to(&self, _label: &str, event: &str, payload: serde_json::Value) {
        self.emit(event, payload)
    }
    /// Turns the global hotkeys on or off (desktop only).
    fn set_hotkeys(&self, _on: bool) {}
    /// A folder for temporary downloads (Android update packages).
    fn cache_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub state: Arc<AppState>,
    pub voice: Arc<Voice>,
    host: Arc<dyn Host>,
}

impl Ctx {
    pub fn new(state: Arc<AppState>, voice: Arc<Voice>, host: Arc<dyn Host>) -> Self {
        Self { state, voice, host }
    }

    pub fn emit(&self, event: &str, payload: impl Serialize) {
        match serde_json::to_value(payload) {
            Ok(v) => {
                if progress::due(event, &v) {
                    self.host.emit(event, v)
                }
            }
            Err(e) => log::warn!("can't send {event}: {e}"),
        }
    }

    pub fn host(&self) -> &dyn Host {
        &*self.host
    }
}

/// The background jobs' progress reaches the window at most twice a second per job. A job over
/// tiny files finishes an album or a song many times a second, and every update restarts the
/// sidebar's bar and redraws it (at 160 Hz, layout on every frame for as long as the job runs,
/// which on a big library is hours). The first update, the last one and any that says the job
/// stopped always go through, so the window never keeps a finished job on screen.
pub(crate) mod progress {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;

    /// Events that report a job's `done` of `total` (the others pass untouched).
    const THROTTLED: &[&str] = &[
        "library-progress",
        "analysis-progress",
        "sonic-progress",
        "health-progress",
        "waveform-progress",
        "lyric-index-progress",
        "fix-all-progress",
        "cover-job-progress",
        "artist-check-progress",
    ];
    pub const GAP: Duration = Duration::from_millis(500);

    /// When each event was last sent, and what it said besides how far the job is.
    type Sent = HashMap<&'static str, (Instant, serde_json::Value)>;
    static LAST: Mutex<Option<Sent>> = Mutex::new(None);

    /// An update without its counts: a change in it (paused, resumed, another kind of check) is
    /// news, not progress.
    fn state_of(payload: &serde_json::Value) -> serde_json::Value {
        let mut v = payload.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("done");
            o.remove("found");
        }
        v
    }

    /// A start (nothing done yet), an end (all done) or a stop (`running: false`).
    pub fn edge(payload: &serde_json::Value) -> bool {
        let num = |k: &str| payload.get(k).and_then(serde_json::Value::as_u64);
        let stopped = payload.get("running").and_then(serde_json::Value::as_bool) == Some(false);
        let (done, total) = (num("done"), num("total"));
        stopped || done.is_none_or(|d| d == 0) || matches!((done, total), (Some(d), Some(t)) if d >= t)
    }

    /// Whether an update of `event` should be sent now (and, if so, noted as sent).
    pub fn due(event: &str, payload: &serde_json::Value) -> bool {
        let Some(&name) = THROTTLED.iter().find(|n| **n == event) else {
            return true;
        };
        due_at(name, payload, Instant::now(), &mut LAST.lock().get_or_insert_with(HashMap::new))
    }

    pub fn due_at(name: &'static str, payload: &serde_json::Value, now: Instant, last: &mut Sent) -> bool {
        let state = state_of(payload);
        let go = edge(payload)
            || last.get(name).is_none_or(|(t, was)| now.saturating_duration_since(*t) >= GAP || *was != state);
        if go {
            last.insert(name, (now, state));
        }
        go
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        #[test]
        fn a_fast_job_reports_twice_a_second_with_its_start_and_end() {
            let mut last = HashMap::new();
            let t0 = Instant::now();
            let mut sent = 0;
            // 1,000 albums in 5 s: one every 5 ms.
            for i in 0..=1000u64 {
                let at = t0 + Duration::from_millis(i * 5);
                if due_at("analysis-progress", &json!({ "done": i, "total": 1000 }), at, &mut last) {
                    sent += 1;
                }
            }
            assert!((10..=13).contains(&sent), "{sent} updates in 5 s");
            // The last one went through (it is the end).
            assert!(due_at("analysis-progress", &json!({ "done": 1000, "total": 1000 }), t0 + Duration::from_millis(5001), &mut last));
        }

        #[test]
        fn a_stop_always_goes_through() {
            let mut last = HashMap::new();
            let t0 = Instant::now();
            assert!(due_at("health-progress", &json!({ "done": 3, "total": 9, "running": true }), t0, &mut last));
            assert!(!due_at("health-progress", &json!({ "done": 4, "total": 9, "running": true }), t0, &mut last));
            assert!(due_at("health-progress", &json!({ "done": 4, "total": 9, "running": false }), t0, &mut last));
        }

        #[test]
        fn pausing_and_resuming_always_go_through() {
            let mut last = HashMap::new();
            let t0 = Instant::now();
            let at = |done: u64, paused: bool| json!({ "done": done, "total": 9, "running": true, "paused": paused });
            assert!(due_at("waveform-progress", &at(3, false), t0, &mut last));
            assert!(due_at("waveform-progress", &at(3, true), t0, &mut last), "paused");
            assert!(due_at("waveform-progress", &at(3, false), t0, &mut last), "resumed");
            assert!(!due_at("waveform-progress", &at(4, false), t0, &mut last), "progress again");
        }

        #[test]
        fn other_events_are_never_held_back() {
            for _ in 0..5 {
                assert!(due("player-status", &json!({ "done": 1, "total": 9 })));
            }
        }
    }
}
