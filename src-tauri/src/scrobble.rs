//! Scrobbling to ListenBrainz (listenbrainz.org).
//!
//! A song counts as listened once you've heard half of it or 4 minutes, whichever comes first
//! (ListenBrainz's rule). Listens that can't be sent (offline) are kept in a file and retried.
//!
//! What ListenBrainz answers decides what happens to waiting listens: accepted, they're done;
//! refused as bad (400), they're dropped (one bad listen can't hold up the rest); a token it no
//! longer accepts (401) signs the account out and keeps them; busy or offline (429, 5xx, no
//! connection), they wait and are tried again (after the Retry-After it asks for).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;

const API: &str = "https://api.listenbrainz.org/1";
pub const TOKEN_SECRET: &str = "listenbrainz-token";
/// The API accepts up to 1000 listens per "import" request.
const MOST_PER_REQUEST: usize = 1000;
/// A Retry-After longer than this is taken as this.
const LONGEST_WAIT: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listen {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: f64,
    /// Unix seconds when the song started.
    pub at: u64,
    /// The ListenBrainz account it was heard under (empty in queues saved before this was kept:
    /// sent to whoever is connected).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
}

enum Job {
    NowPlaying(Listen),
    /// Try sending anything waiting in the offline queue.
    Flush,
}

/// What ListenBrainz made of a request.
#[derive(Debug, PartialEq)]
enum Sent {
    Ok,
    /// Refused as bad (400 and the like): sending it again can't help.
    Rejected(String),
    /// The token isn't accepted (any more).
    Unauthorized,
    /// Busy, down or unreachable: try again later (after this long, if it said).
    Retry(Option<Duration>, String),
}

/// Whether a play counts as a listen.
pub fn counts(listened: f64, duration: f64) -> bool {
    duration >= 30.0 && listened >= (duration / 2.0).min(240.0)
}

/// The listens waiting to be sent, kept in a file.
struct Waiting {
    file: PathBuf,
    queue: Mutex<Vec<Listen>>,
    /// Goes up when the queue is emptied (disconnecting), so a send in flight then doesn't take
    /// listens off the front that are no longer the ones it sent.
    cleared: AtomicU64,
}

impl Waiting {
    fn save(&self, q: &[Listen]) {
        crate::state::write_json(&self.file, &q);
    }
}

/// The running scrobbler.
pub struct Scrobbler {
    tx: Sender<Job>,
    waiting: Arc<Waiting>,
}

static RUNNING: OnceLock<Scrobbler> = OnceLock::new();

/// Starts sending. `account`: the connected ListenBrainz user name (empty: not connected).
/// `signed_out`: ListenBrainz stopped accepting the token; the app should show it as disconnected.
pub fn start(
    queue_file: PathBuf,
    account: impl Fn() -> String + Send + 'static,
    signed_out: impl Fn() + Send + 'static,
) -> &'static Scrobbler {
    let (tx, rx) = crossbeam_channel::unbounded();
    let waiting = Arc::new(Waiting {
        queue: Mutex::new(crate::state::read_json(&queue_file)),
        file: queue_file,
        cleared: AtomicU64::new(0),
    });
    let shared = waiting.clone();
    let started = RUNNING.get_or_init(move || {
        std::thread::Builder::new()
            .name("cadence-scrobble".into())
            .spawn(move || run(rx, shared, account, signed_out))
            .expect("scrobble thread");
        Scrobbler { tx, waiting }
    });
    let _ = started.tx.send(Job::Flush);
    started
}

impl Scrobbler {
    pub fn now_playing(&self, l: Listen) {
        let _ = self.tx.send(Job::NowPlaying(l));
    }

    /// Keeps a listen (on disk at once, so it survives a quit) for `user`, then sends what waits.
    pub fn listened(&self, mut l: Listen, user: &str) {
        if user.is_empty() {
            return;
        }
        l.user = user.to_string();
        {
            let mut q = self.waiting.queue.lock();
            q.push(l);
            self.waiting.save(&q);
        }
        let _ = self.tx.send(Job::Flush);
    }

    /// Forgets every waiting listen (disconnecting: another account mustn't get them).
    fn clear(&self) {
        let mut q = self.waiting.queue.lock();
        self.waiting.cleared.fetch_add(1, Ordering::SeqCst);
        q.clear();
        self.waiting.save(&q);
    }
}

/// The scrobbler, once `start` has run.
pub fn running() -> Option<&'static Scrobbler> {
    RUNNING.get()
}

/// Forgets the listens waiting to be sent (when disconnecting), the running scrobbler's or, before
/// it runs, the saved file's.
pub fn clear(queue_file: &std::path::Path) {
    match RUNNING.get() {
        Some(s) => s.clear(),
        None if queue_file.exists() => crate::state::write_json(queue_file, &Vec::<Listen>::new()),
        None => {}
    }
}

/// None when the system can't make secure connections (no CA certificates): scrobbling is off,
/// rather than Cadence closing at launch.
fn client() -> Option<reqwest::blocking::Client> {
    crate::net::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .inspect_err(|e| log::warn!("no HTTP client for scrobbling: {e}"))
        .ok()
}

fn metadata(l: &Listen) -> serde_json::Value {
    json!({
        "artist_name": l.artist,
        "track_name": l.title,
        "release_name": l.album,
        "additional_info": {
            "media_player": "Cadence",
            "submission_client": "Cadence",
            "submission_client_version": env!("CARGO_PKG_VERSION"),
            "duration_ms": (l.duration * 1000.0).round() as u64,
        }
    })
}

/// What an answer's status means (with its Retry-After header, if any).
fn outcome(status: u16, retry_after: Option<&str>) -> Sent {
    match status {
        200..=299 => Sent::Ok,
        401 => Sent::Unauthorized,
        408 | 429 | 500..=599 => {
            let wait = retry_after.and_then(|s| s.trim().parse::<u64>().ok()).map(|s| Duration::from_secs(s).min(LONGEST_WAIT));
            Sent::Retry(wait, format!("ListenBrainz answered {status}"))
        }
        _ => Sent::Rejected(format!("ListenBrainz answered {status}")),
    }
}

fn submit(http: &reqwest::blocking::Client, token: &str, kind: &str, listens: &[Listen]) -> Sent {
    let payload: Vec<_> = listens
        .iter()
        .map(|l| {
            if kind == "playing_now" {
                json!({ "track_metadata": metadata(l) })
            } else {
                json!({ "listened_at": l.at, "track_metadata": metadata(l) })
            }
        })
        .collect();
    let r = match http
        .post(format!("{API}/submit-listens"))
        .header("Authorization", format!("Token {token}"))
        .json(&json!({ "listen_type": kind, "payload": payload }))
        .send()
    {
        Ok(r) => r,
        Err(e) => return Sent::Retry(None, e.to_string()),
    };
    let retry_after = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
    let sent = outcome(r.status().as_u16(), retry_after.as_deref());
    if let Sent::Rejected(why) = &sent {
        // ListenBrainz says what was wrong in the body.
        let body = r.text().unwrap_or_default();
        return Sent::Rejected(format!("{why}: {}", body.chars().take(300).collect::<String>()));
    }
    sent
}

/// Sends the waiting listens for `user`, oldest first, in as few requests as it can. A refused
/// request is split in halves until the refused listen is found alone; only it is dropped.
/// Stops at the first answer that isn't "accepted" or "refused" and returns it (the rest wait).
fn flush(waiting: &Waiting, user: &str, mut send: impl FnMut(&[Listen]) -> Sent) -> Sent {
    {
        // Listens heard under another account never go to this one.
        let mut q = waiting.queue.lock();
        let before = q.len();
        q.retain(|l| l.user.is_empty() || l.user == user);
        if q.len() != before {
            log::info!("{} waiting listens were for another ListenBrainz account; let go", before - q.len());
            waiting.save(&q);
        }
    }
    let mut size = MOST_PER_REQUEST;
    loop {
        let (batch, cleared) = {
            let q = waiting.queue.lock();
            if q.is_empty() {
                return Sent::Ok;
            }
            (q[..q.len().min(size)].to_vec(), waiting.cleared.load(Ordering::SeqCst))
        };
        let sent = send(&batch);
        let done = match &sent {
            Sent::Ok => {
                size = (size * 2).min(MOST_PER_REQUEST);
                true
            }
            Sent::Rejected(why) if batch.len() == 1 => {
                let l = &batch[0];
                log::warn!("ListenBrainz refused the listen of {} – {} ({why}); let go", l.artist, l.title);
                true
            }
            Sent::Rejected(_) => {
                size = (batch.len() / 2).max(1);
                false
            }
            _ => return sent,
        };
        if done {
            let mut q = waiting.queue.lock();
            // Emptied meanwhile (disconnected): nothing of this batch is left to take off.
            if waiting.cleared.load(Ordering::SeqCst) == cleared {
                let n = batch.len().min(q.len());
                q.drain(..n);
                waiting.save(&q);
            }
        }
    }
}

fn run(rx: Receiver<Job>, waiting: Arc<Waiting>, account: impl Fn() -> String, signed_out: impl Fn()) {
    let Some(http) = client() else {
        // Nothing can be sent; take the jobs so senders never block.
        for _ in rx {}
        return;
    };
    // ListenBrainz asked to wait until then.
    let mut not_before: Option<Instant> = None;
    let mut told_no_token = false;
    loop {
        // Retry the offline queue every few minutes even if nothing new happens.
        let idle = not_before.map_or(Duration::from_secs(300), |t| t.saturating_duration_since(Instant::now()).min(Duration::from_secs(300)));
        let job = match rx.recv_timeout(idle) {
            Ok(j) => j,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Job::Flush,
            Err(_) => return,
        };
        let user = account();
        if user.is_empty() {
            continue;
        }
        let Some(token) = crate::secrets::get(TOKEN_SECRET) else {
            // Connected, but the token can't be read (the system's key store is locked, say):
            // the listens wait until it can.
            if !told_no_token {
                log::warn!("ListenBrainz is connected as {user} but its token can't be read; listens wait");
                told_no_token = true;
            }
            continue;
        };
        told_no_token = false;
        if let Job::NowPlaying(l) = job {
            match submit(&http, &token, "playing_now", &[l]) {
                Sent::Ok => {}
                Sent::Unauthorized => {
                    log::warn!("ListenBrainz no longer accepts the token of {user}; signed out");
                    signed_out();
                    continue;
                }
                other => log::info!("now-playing not sent: {other:?}"),
            }
        }
        if not_before.is_some_and(|t| Instant::now() < t) {
            continue;
        }
        not_before = None;
        let kind = |b: &[Listen]| if b.len() == 1 { "single" } else { "import" };
        match flush(&waiting, &user, |batch| submit(&http, &token, kind(batch), batch)) {
            Sent::Ok | Sent::Rejected(_) => {}
            Sent::Unauthorized => {
                log::warn!("ListenBrainz no longer accepts the token of {user}; signed out, listens kept");
                signed_out();
            }
            Sent::Retry(wait, why) => {
                not_before = wait.map(|w| Instant::now() + w);
                log::info!("{} listens waiting to be sent: {why}", waiting.queue.lock().len());
            }
        }
    }
}

/// Checks a token and returns the ListenBrainz user name.
pub fn validate(token: &str) -> Result<String, String> {
    let r = client()
        .ok_or("This system can’t make secure connections (no CA certificates).")?
        .get(format!("{API}/validate-token"))
        .header("Authorization", format!("Token {}", token.trim()))
        .send()
        .map_err(|e| format!("Couldn't reach ListenBrainz: {e}"))?;
    let v: serde_json::Value = r.json().map_err(|e| e.to_string())?;
    if v["valid"].as_bool() == Some(true) {
        Ok(v["user_name"].as_str().unwrap_or("").to_string())
    } else {
        Err(
            "ListenBrainz didn’t accept this token. Copy it again from listenbrainz.org/settings."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_rule() {
        assert!(!counts(100.0, 25.0)); // too short a song
        assert!(counts(100.0, 200.0)); // half heard
        assert!(!counts(90.0, 200.0));
        assert!(counts(240.0, 900.0)); // 4 minutes of a long song
    }

    fn listen(title: &str, user: &str) -> Listen {
        Listen { artist: "A".into(), title: title.into(), album: "B".into(), duration: 200.0, at: 1, user: user.into() }
    }

    fn waiting(name: &str, q: Vec<Listen>) -> Waiting {
        let dir = std::env::temp_dir().join(format!("cadence-scrobble-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Waiting { file: dir.join("scrobble-queue.json"), queue: Mutex::new(q), cleared: AtomicU64::new(0) }
    }

    #[test]
    fn answers_decide_what_happens() {
        assert_eq!(outcome(200, None), Sent::Ok);
        assert_eq!(outcome(401, None), Sent::Unauthorized);
        assert!(matches!(outcome(400, None), Sent::Rejected(_)));
        assert!(matches!(outcome(503, None), Sent::Retry(None, _)));
        assert!(matches!(outcome(429, Some("120")), Sent::Retry(Some(d), _) if d == Duration::from_secs(120)));
        assert!(matches!(outcome(429, Some("99999")), Sent::Retry(Some(d), _) if d == LONGEST_WAIT));
    }

    #[test]
    fn a_bad_listen_is_dropped_alone_and_the_rest_are_sent() {
        let q: Vec<Listen> = (0..7).map(|i| listen(&format!("s{i}"), "")).collect();
        let w = waiting("bad", q);
        let mut sent = Vec::new();
        let r = flush(&w, "me", |b| {
            if b.iter().any(|l| l.title == "s4") {
                Sent::Rejected("400".into())
            } else {
                sent.extend(b.iter().map(|l| l.title.clone()));
                Sent::Ok
            }
        });
        assert_eq!(r, Sent::Ok);
        assert!(w.queue.lock().is_empty());
        assert_eq!(sent, ["s0", "s1", "s2", "s3", "s5", "s6"]);
        let _ = std::fs::remove_dir_all(w.file.parent().unwrap());
    }

    #[test]
    fn listens_wait_when_offline_or_signed_out_and_never_go_to_another_account() {
        let w = waiting("wait", vec![listen("old", ""), listen("theirs", "someone"), listen("mine", "me")]);
        assert!(matches!(flush(&w, "me", |_| Sent::Retry(None, "offline".into())), Sent::Retry(..)));
        assert_eq!(w.queue.lock().len(), 2, "the other account's listen is let go, the rest wait");
        assert_eq!(flush(&w, "me", |_| Sent::Unauthorized), Sent::Unauthorized);
        assert_eq!(w.queue.lock().len(), 2);
        let mut got = Vec::new();
        assert_eq!(flush(&w, "me", |b| { got.extend(b.iter().map(|l| l.title.clone())); Sent::Ok }), Sent::Ok);
        assert_eq!(got, ["old", "mine"]);
        // Saved: what's left is in the file.
        assert_eq!(crate::state::read_json::<Vec<Listen>>(&w.file), Vec::<Listen>::new());
        let _ = std::fs::remove_dir_all(w.file.parent().unwrap());
    }

    #[test]
    fn queues_saved_before_accounts_were_kept_still_read() {
        let old: Vec<Listen> = serde_json::from_str(r#"[{"artist":"A","title":"T","album":"B","duration":200.0,"at":5}]"#).unwrap();
        assert_eq!(old[0].user, "");
    }
}
