//! Scrobbling to ListenBrainz (listenbrainz.org).
//!
//! A song counts as listened once you've heard half of it or 4 minutes, whichever comes first
//! (ListenBrainz's rule). Listens that can't be sent (offline) are kept in a file and retried.
//!
//! What ListenBrainz answers decides what happens to waiting listens: accepted, they're done;
//! refused as bad (400), they're dropped (one bad listen can't hold up the rest); busy or offline
//! (429, 5xx, no connection), they wait and are tried again (after the Retry-After it asks for).
//! A token it doesn't accept (401) is checked again a couple of minutes later, and only when it's
//! refused again is the account signed out (one 401 can be a hiccup). Signed out that way, listens
//! are still kept for that account: connecting it again sends them, connecting another account
//! lets them go. Disconnecting by hand forgets them. At most `MOST_WAITING` wait (the newest).

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
/// The most listens kept waiting (the newest; ~250 hours of music, ~1 MB on disk): signed out or
/// offline for months, the queue can't grow without end.
const MOST_WAITING: usize = 5000;
/// After a 401, the token is checked again this much later before the account is signed out.
const RECHECK: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// MusicBrainz ids from the song's tags, when it has them (sent as `recording_mbid`,
    /// `release_mbid` and `artist_mbids`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_mbid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_mbid: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artist_mbids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_no: Option<u32>,
}

enum Job {
    NowPlaying(Listen),
    /// Try sending anything waiting in the offline queue.
    Flush,
}

/// What ListenBrainz made of a request.
#[derive(Debug, PartialEq)]
pub(crate) enum Sent {
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
    /// How many of the oldest listens were let go to keep the queue at `MOST_WAITING` (so a send
    /// in flight takes off only those of its listens still there).
    trimmed: AtomicU64,
    /// The account last connected (kept in `<queue file>.account.json`): listens heard while it
    /// was signed out by ListenBrainz are kept for it. Empty: none, or disconnected by hand.
    last: Mutex<String>,
}

impl Waiting {
    fn new(file: PathBuf) -> Self {
        let mut queue: Vec<Listen> = crate::state::read_json(&file);
        let last: String = crate::state::read_json(&account_file(&file));
        let w = Waiting { file, queue: Mutex::new(Vec::new()), cleared: AtomicU64::new(0), trimmed: AtomicU64::new(0), last: Mutex::new(last) };
        w.trim(&mut queue);
        *w.queue.lock() = queue;
        w
    }

    fn save(&self, q: &[Listen]) {
        crate::state::write_json(&self.file, &q);
    }

    /// Keeps the newest `MOST_WAITING` listens.
    fn trim(&self, q: &mut Vec<Listen>) {
        if q.len() > MOST_WAITING {
            let n = q.len() - MOST_WAITING;
            q.drain(..n);
            self.trimmed.fetch_add(n as u64, Ordering::SeqCst);
            log::warn!("more than {MOST_WAITING} listens waiting to be sent; the {n} oldest let go");
        }
    }

    /// Remembers the connected account (saved when it changes).
    fn connected(&self, user: &str) {
        let mut last = self.last.lock();
        if *last != user {
            *last = user.to_string();
            crate::state::write_json(&account_file(&self.file), &*last);
        }
    }

    /// Keeps a listen for `user`, or, signed out by ListenBrainz (`user` empty), for the account
    /// last connected. False when there's no account to keep it for.
    fn keep(&self, mut l: Listen, user: &str) -> bool {
        if !user.is_empty() {
            self.connected(user);
        }
        let account = self.last.lock().clone();
        if account.is_empty() {
            return false;
        }
        l.user = account;
        let mut q = self.queue.lock();
        q.push(l);
        self.trim(&mut q);
        self.save(&q);
        true
    }
}

/// Where hearts waiting to be sent are kept, beside the queue: `scrobble-loved.json`.
fn loved_file(queue_file: &std::path::Path) -> PathBuf {
    queue_file.with_file_name("scrobble-loved.json")
}

/// Where the last connected account is kept, beside the queue: `scrobble-queue.json.account.json`.
fn account_file(queue_file: &std::path::Path) -> PathBuf {
    let mut name = queue_file.file_name().unwrap_or_default().to_os_string();
    name.push(".account.json");
    queue_file.with_file_name(name)
}

/// A token ListenBrainz refused once (401) isn't believed refused until a check `RECHECK` later
/// is refused too: one 401 can be a hiccup, and signing out on it would stop recording listens.
#[derive(Default)]
struct Doubt {
    /// When the refusal (or the last check that couldn't be made) was.
    since: Option<Instant>,
}

impl Doubt {
    fn refused(&mut self, now: Instant) {
        self.since.get_or_insert(now);
    }
    fn in_doubt(&self) -> bool {
        self.since.is_some()
    }
    /// When to check the token again.
    fn check_at(&self) -> Option<Instant> {
        self.since.map(|t| t + RECHECK)
    }
    fn check_due(&self, now: Instant) -> bool {
        self.check_at().is_some_and(|t| now >= t)
    }
    /// What the check found: true when the token is refused for sure (sign out). A check that
    /// couldn't be made is made again later.
    fn checked(&mut self, answer: &Sent, now: Instant) -> bool {
        match answer {
            Sent::Unauthorized => {
                self.since = None;
                true
            }
            Sent::Ok => {
                self.since = None;
                false
            }
            Sent::Retry(..) | Sent::Rejected(_) => {
                self.since = Some(now);
                false
            }
        }
    }
}

/// The running scrobbler.
pub struct Scrobbler {
    tx: Sender<Job>,
    waiting: Arc<Waiting>,
    /// Hearts waiting to be sent to ListenBrainz (`lbloved`).
    loved: Arc<crate::lbloved::Queue>,
}

static RUNNING: OnceLock<Scrobbler> = OnceLock::new();
static NO_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Starts sending. `account`: the connected ListenBrainz user name (empty: not connected).
/// `signed_out`: ListenBrainz stopped accepting the token; the app should show it as disconnected.
pub fn start(
    queue_file: PathBuf,
    account: impl Fn() -> String + Send + 'static,
    signed_out: impl Fn() + Send + 'static,
) -> &'static Scrobbler {
    let (tx, rx) = crossbeam_channel::unbounded();
    let loved = Arc::new(crate::lbloved::Queue::new(loved_file(&queue_file)));
    let recordings = loved_file(&queue_file).with_file_name("listenbrainz-recordings.json");
    let waiting = Arc::new(Waiting::new(queue_file));
    let (shared, shared_loved) = (waiting.clone(), loved.clone());
    let started = RUNNING.get_or_init(move || {
        std::thread::Builder::new()
            .name("cadence-scrobble".into())
            .spawn(move || run(rx, shared, shared_loved, recordings, account, signed_out))
            .expect("scrobble thread");
        Scrobbler { tx, waiting, loved }
    });
    let _ = started.tx.send(Job::Flush);
    started
}

impl Scrobbler {
    pub fn now_playing(&self, l: Listen) {
        let _ = self.tx.send(Job::NowPlaying(l));
    }

    /// Keeps a listen (on disk at once, so it survives a quit) for `user` — or, when ListenBrainz
    /// signed the account out (`user` empty), for the account last connected, to send if it's
    /// connected again — then sends what waits.
    pub fn listened(&self, l: Listen, user: &str) {
        if self.waiting.keep(l, user) {
            let _ = self.tx.send(Job::Flush);
        }
    }

    /// Keeps a heart (or an un-heart) for sending (on disk at once), then sends what waits. It never
    /// waits for the network: the recording is looked up and the request made on this thread.
    pub fn loved(&self, item: crate::lbloved::Item) {
        self.loved.push(item);
        let _ = self.tx.send(Job::Flush);
    }

    pub fn loved_waiting(&self) -> usize {
        self.loved.len()
    }

    /// Forgets every waiting listen and the account (disconnecting by hand: another account
    /// mustn't get them, and nothing is kept until one is connected).
    fn clear(&self) {
        {
            let mut q = self.waiting.queue.lock();
            self.waiting.cleared.fetch_add(1, Ordering::SeqCst);
            q.clear();
            self.waiting.save(&q);
        }
        self.loved.clear();
        self.waiting.connected("");
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
        None => {
            if queue_file.exists() {
                crate::state::write_json(queue_file, &Vec::<Listen>::new());
            }
            if account_file(queue_file).exists() {
                crate::state::write_json(&account_file(queue_file), &String::new());
            }
            crate::lbloved::clear_file(&loved_file(queue_file));
        }
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

pub(crate) fn metadata(l: &Listen) -> serde_json::Value {
    let mut info = json!({
        "media_player": "Cadence",
        "media_player_version": env!("CARGO_PKG_VERSION"),
        "submission_client": "Cadence",
        "submission_client_version": env!("CARGO_PKG_VERSION"),
        "duration_ms": (l.duration * 1000.0).round() as u64,
    });
    // Only what the tags have, and only ids written like MusicBrainz's (a bad one would make
    // ListenBrainz refuse the listen).
    let ok = |s: &String| crate::tagids::is_mbid(s);
    if let Some(m) = l.recording_mbid.as_ref().filter(|m| ok(m)) {
        info["recording_mbid"] = json!(m);
    }
    if let Some(m) = l.release_mbid.as_ref().filter(|m| ok(m)) {
        info["release_mbid"] = json!(m);
    }
    let artists: Vec<&String> = l.artist_mbids.iter().filter(|m| ok(m)).collect();
    if !artists.is_empty() {
        info["artist_mbids"] = json!(artists);
    }
    if let Some(n) = l.track_no.filter(|n| *n > 0) {
        info["tracknumber"] = json!(n.to_string());
    }
    let mut m = json!({ "artist_name": l.artist, "track_name": l.title, "additional_info": info });
    // (A listen with no album sends no release name rather than an empty one.)
    if !l.album.trim().is_empty() {
        m["release_name"] = json!(l.album);
    }
    m
}

/// What an answer's status means (with its Retry-After header, if any).
pub(crate) fn outcome(status: u16, retry_after: Option<&str>) -> Sent {
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
        let (batch, cleared, trimmed) = {
            let q = waiting.queue.lock();
            if q.is_empty() {
                return Sent::Ok;
            }
            (q[..q.len().min(size)].to_vec(), waiting.cleared.load(Ordering::SeqCst), waiting.trimmed.load(Ordering::SeqCst))
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
                // The oldest let go meanwhile (`MOST_WAITING`) were this batch's first ones.
                let gone = (waiting.trimmed.load(Ordering::SeqCst) - trimmed) as usize;
                let n = batch.len().saturating_sub(gone).min(q.len());
                q.drain(..n);
                waiting.save(&q);
            }
        }
    }
}

fn run(rx: Receiver<Job>, waiting: Arc<Waiting>, loved: Arc<crate::lbloved::Queue>, recordings: PathBuf, account: impl Fn() -> String, signed_out: impl Fn()) {
    let Some(http) = client() else {
        // Nothing can be sent; take the jobs so senders never block.
        for _ in rx {}
        return;
    };
    // ListenBrainz asked to wait until then.
    let mut not_before: Option<Instant> = None;
    let mut told_no_token = false;
    // A 401 waiting to be checked again.
    let mut doubt = Doubt::default();
    // Hearts are sent with the client of `lbapi`; the recordings found for songs are kept in a file.
    let mut lb = crate::lbapi::Http::new(None);
    let mut resolver = crate::lbloved::Resolver::new(recordings);
    let mut lb_clock = crate::lbapi::Wall(Instant::now(), &NO_STOP);
    loop {
        // Retry the offline queue every few minutes even if nothing new happens (and check a
        // refused token again when it's time).
        let now = Instant::now();
        let idle = [not_before, doubt.check_at()]
            .into_iter()
            .flatten()
            .map(|t| t.saturating_duration_since(now))
            .fold(Duration::from_secs(300), Duration::min);
        let job = match rx.recv_timeout(idle) {
            Ok(j) => j,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Job::Flush,
            Err(_) => return,
        };
        let user = account();
        if user.is_empty() {
            doubt = Doubt::default();
            continue;
        }
        waiting.connected(&user);
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
        if doubt.in_doubt() {
            // Refused once: nothing is sent until the token is checked again (listens wait).
            if !doubt.check_due(Instant::now()) {
                continue;
            }
            let answer = check_token(&http, &token);
            if doubt.checked(&answer, Instant::now()) {
                log::warn!("ListenBrainz refused the token of {user} again; signed out (listens are kept for when {user} is connected again)");
                signed_out();
                continue;
            }
            if doubt.in_doubt() {
                log::info!("couldn't check the ListenBrainz token ({answer:?}); trying again later");
                continue;
            }
            log::info!("ListenBrainz accepts the token of {user} again");
        }
        if let Job::NowPlaying(l) = job {
            match submit(&http, &token, "playing_now", &[l]) {
                Sent::Ok => {}
                Sent::Unauthorized => {
                    log::warn!("ListenBrainz refused the token of {user} (401); checking again in {} s, listens wait", RECHECK.as_secs());
                    doubt.refused(Instant::now());
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
                log::warn!("ListenBrainz refused the token of {user} (401); checking again in {} s, listens wait", RECHECK.as_secs());
                doubt.refused(Instant::now());
                continue;
            }
            Sent::Retry(wait, why) => {
                not_before = wait.map(|w| Instant::now() + w);
                log::info!("{} listens waiting to be sent: {why}", waiting.queue.lock().len());
                continue;
            }
        }
        // Then the hearts, a few requests a turn (the next turn comes a second later).
        if !loved.is_empty()
            && let Some(lb) = lb.as_mut()
        {
            lb.set_token(Some(token.clone()));
            let (turn, wait) = crate::lbloved::turn(&loved, &user, lb, &mut lb_clock, &mut resolver);
            match turn.outcome {
                Sent::Unauthorized => {
                    log::warn!("ListenBrainz refused the token of {user} (401); checking again in {} s", RECHECK.as_secs());
                    doubt.refused(Instant::now());
                }
                Sent::Retry(w, why) => {
                    not_before = w.map(|w| Instant::now() + w);
                    log::info!("{} loved songs waiting to be sent: {why}", turn.left);
                }
                Sent::Ok | Sent::Rejected(_) => {
                    if turn.left > 0 {
                        not_before = Some(Instant::now() + wait.unwrap_or(Duration::from_millis(crate::musicbrainz::GAP_MS)));
                    }
                }
            }
            lb.set_token(None);
        }
    }
}

/// Whether ListenBrainz accepts the token (after a 401): Ok, Unauthorized, or Retry when it can't
/// be asked now.
fn check_token(http: &reqwest::blocking::Client, token: &str) -> Sent {
    let r = match http.get(format!("{API}/validate-token")).header("Authorization", format!("Token {token}")).send() {
        Ok(r) => r,
        Err(e) => return Sent::Retry(None, e.to_string()),
    };
    let status = r.status().as_u16();
    if status != 200 {
        return match outcome(status, None) {
            Sent::Rejected(why) => Sent::Retry(None, why),
            other => other,
        };
    }
    match r.json::<serde_json::Value>() {
        Ok(v) => token_answer(&v),
        Err(e) => Sent::Retry(None, e.to_string()),
    }
}

/// What validate-token's answer says about the token.
fn token_answer(v: &serde_json::Value) -> Sent {
    match v["valid"].as_bool() {
        Some(true) => Sent::Ok,
        Some(false) => Sent::Unauthorized,
        None => Sent::Retry(None, "ListenBrainz gave no answer about the token".into()),
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
        Listen { artist: "A".into(), title: title.into(), album: "B".into(), duration: 200.0, at: 1, user: user.into(), ..Default::default() }
    }

    fn waiting(name: &str, q: Vec<Listen>) -> Waiting {
        let dir = std::env::temp_dir().join(format!("cadence-scrobble-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = Waiting::new(dir.join("scrobble-queue.json"));
        *w.queue.lock() = q;
        w
    }

    #[test]
    fn listens_heard_while_signed_out_wait_for_that_account() {
        let w = waiting("signed-out", Vec::new());
        // Never connected: nothing to keep them for.
        assert!(!w.keep(listen("before", ""), ""));
        assert!(w.queue.lock().is_empty());
        assert!(w.keep(listen("a", ""), "me"));
        // ListenBrainz signed the account out (the user name is cleared): still kept, for "me".
        assert!(w.keep(listen("b", ""), ""));
        assert_eq!(w.queue.lock().iter().map(|l| l.user.as_str()).collect::<Vec<_>>(), ["me", "me"]);
        // The account survives a restart.
        let again = Waiting::new(w.file.clone());
        assert_eq!(*again.last.lock(), "me");
        assert_eq!(again.queue.lock().len(), 2);
        // "me" connected again: sent.
        let mut got = Vec::new();
        assert_eq!(flush(&w, "me", |b| { got.extend(b.iter().map(|l| l.title.clone())); Sent::Ok }), Sent::Ok);
        assert_eq!(got, ["a", "b"]);
        // Signed out again, then someone else connects: theirs are let go, never sent to them.
        assert!(w.keep(listen("c", ""), ""));
        let mut sent = 0;
        assert_eq!(flush(&w, "someone", |b| { sent += b.len(); Sent::Ok }), Sent::Ok);
        assert_eq!(sent, 0);
        assert!(w.queue.lock().is_empty());
        let _ = std::fs::remove_dir_all(w.file.parent().unwrap());
    }

    #[test]
    fn the_queue_keeps_the_newest_listens() {
        let w = waiting("bounded", (0..MOST_WAITING - 1).map(|i| listen(&format!("s{i}"), "me")).collect());
        for i in MOST_WAITING - 1..MOST_WAITING + 3 {
            w.keep(listen(&format!("s{i}"), ""), "me");
        }
        let q = w.queue.lock().clone();
        assert_eq!(q.len(), MOST_WAITING);
        assert_eq!(q[0].title, "s3", "the oldest let go");
        assert_eq!(q.last().unwrap().title, format!("s{}", MOST_WAITING + 2));
        // Read back from the file, it's the same.
        assert_eq!(Waiting::new(w.file.clone()).queue.lock().len(), MOST_WAITING);
        // A send in flight while the oldest are let go takes off only what it sent that's still there.
        let w = waiting("bounded-flight", (0..MOST_WAITING).map(|i| listen(&format!("t{i}"), "me")).collect());
        let mut first = true;
        let r = flush(&w, "me", |b| {
            if first {
                first = false;
                // Two more heard while the first 1000 are being sent: t0 and t1 are let go.
                w.keep(listen("new1", ""), "me");
                w.keep(listen("new2", ""), "me");
                assert_eq!(b[0].title, "t0");
                return Sent::Retry(None, "offline".into());
            }
            Sent::Retry(None, "offline".into())
        });
        assert!(matches!(r, Sent::Retry(..)));
        let w2 = waiting("bounded-flight-2", (0..10).map(|i| listen(&format!("u{i}"), "me")).collect());
        let mut extra = true;
        let r = flush(&w2, "me", |b| {
            if extra {
                extra = false;
                assert_eq!(b.len(), 10);
                // Trimming two of this batch's listens while it's sent (as `trim` does).
                let mut q = w2.queue.lock();
                q.drain(..2);
                w2.trimmed.fetch_add(2, Ordering::SeqCst);
                q.push(listen("later", "me"));
            }
            Sent::Ok
        });
        assert_eq!(r, Sent::Ok);
        assert!(w2.queue.lock().is_empty(), "the batch's 8 left were taken off, then the later one sent");
        assert_eq!(w.queue.lock().len(), MOST_WAITING);
        let _ = std::fs::remove_dir_all(w.file.parent().unwrap());
        let _ = std::fs::remove_dir_all(w2.file.parent().unwrap());
    }

    #[test]
    fn one_401_doesnt_sign_out() {
        let t0 = Instant::now();
        let mut d = Doubt::default();
        assert!(!d.in_doubt());
        d.refused(t0);
        // A second 401 straight after (now-playing, then the queue) is the same refusal.
        d.refused(t0 + Duration::from_secs(1));
        assert_eq!(d.check_at(), Some(t0 + RECHECK));
        assert!(!d.check_due(t0 + Duration::from_secs(30)));
        assert!(d.check_due(t0 + RECHECK));
        // The check is accepted: it was a hiccup.
        assert!(!d.checked(&Sent::Ok, t0 + RECHECK));
        assert!(!d.in_doubt());
        // Refused, and the check can't be made (offline): checked again later, not signed out.
        d.refused(t0);
        let later = t0 + RECHECK;
        assert!(!d.checked(&Sent::Retry(None, "offline".into()), later));
        assert_eq!(d.check_at(), Some(later + RECHECK));
        // Refused on the check too: signed out.
        assert!(d.checked(&Sent::Unauthorized, later + RECHECK));
        assert!(!d.in_doubt());
        // What validate-token says.
        assert_eq!(token_answer(&serde_json::json!({"valid": true, "user_name": "me"})), Sent::Ok);
        assert_eq!(token_answer(&serde_json::json!({"valid": false})), Sent::Unauthorized);
        assert!(matches!(token_answer(&serde_json::json!({})), Sent::Retry(..)));
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
    fn submitted_listens_carry_ids_track_number_duration_and_the_player() {
        let mut l = listen("Song", "me");
        l.recording_mbid = Some("98255a8c-017a-4bc7-8dd6-1fa36124572b".into());
        l.release_mbid = Some("bf9e91ea-8029-4a04-a26a-224e00a83266".into());
        l.artist_mbids = vec!["db92a151-1ac2-438b-bc43-b82e149ddd50".into(), "not an id".into()];
        l.track_no = Some(7);
        let m = metadata(&l);
        assert_eq!((m["artist_name"].as_str(), m["track_name"].as_str(), m["release_name"].as_str()), (Some("A"), Some("Song"), Some("B")));
        let i = &m["additional_info"];
        assert_eq!(i["recording_mbid"], "98255a8c-017a-4bc7-8dd6-1fa36124572b");
        assert_eq!(i["release_mbid"], "bf9e91ea-8029-4a04-a26a-224e00a83266");
        assert_eq!(i["artist_mbids"], serde_json::json!(["db92a151-1ac2-438b-bc43-b82e149ddd50"]), "a badly written id is left out");
        assert_eq!(i["tracknumber"], "7");
        assert_eq!(i["duration_ms"], 200_000);
        assert_eq!(i["media_player"], "Cadence");
        assert_eq!(i["media_player_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(i["submission_client"], "Cadence");
        assert_eq!(i["submission_client_version"], env!("CARGO_PKG_VERSION"));
        // Without ids in the tags, none are sent (and no empty album name).
        let mut bare = listen("Song", "me");
        bare.album.clear();
        let m = metadata(&bare);
        for k in ["recording_mbid", "release_mbid", "artist_mbids", "tracknumber"] {
            assert!(m["additional_info"].get(k).is_none(), "{k}");
        }
        assert!(m.get("release_name").is_none());
        // The queue file keeps the ids, and one saved before them still reads.
        let back: Listen = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn queues_saved_before_accounts_were_kept_still_read() {
        let old: Vec<Listen> = serde_json::from_str(r#"[{"artist":"A","title":"T","album":"B","duration":200.0,"at":5}]"#).unwrap();
        assert_eq!(old[0].user, "");
    }
}
