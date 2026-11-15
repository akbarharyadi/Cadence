//! Asking ListenBrainz questions (listens, loved songs, fresh releases, a song's recording id), as
//! MusicBrainz is asked in `musicbrainz`: one request at a time, paced (ListenBrainz asks for no
//! more than one a second), backing off when it's busy (429 / 5xx, honouring Retry-After), and
//! waiting out the window when its `X-RateLimit-*` headers say none are left. The network and the
//! clock are parameters, so tests use fakes and nothing from tests reaches the network.
//!
//! The user token is only ever put in the `Authorization` header: never in a URL, a log line, an
//! error or an event.

use std::time::Duration;

use serde_json::Value;

use crate::musicbrainz::{Clock, Pacer};

pub const API: &str = "https://api.listenbrainz.org/1";
/// Busy answers in a row before a job gives up for now.
const MAX_BUSY: u32 = 5;
/// The longest wait taken from a header.
const LONGEST_WAIT_SECS: u64 = 120;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Clone, Debug)]
pub struct Req {
    pub method: Method,
    /// After the API's root: "/user/name/listens?count=1000".
    pub path: String,
    pub body: Option<Value>,
    /// Sends the token (needed to write, and by the recording lookup).
    pub auth: bool,
}

impl Req {
    pub fn get(path: impl Into<String>, auth: bool) -> Req {
        Req { method: Method::Get, path: path.into(), body: None, auth }
    }
    pub fn post(path: impl Into<String>, body: Value) -> Req {
        Req { method: Method::Post, path: path.into(), body: Some(body), auth: true }
    }
}

/// What the `X-RateLimit-*` headers of an answer say.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rate {
    pub remaining: Option<u64>,
    /// Seconds until the window is over (`X-RateLimit-Reset-In`: right even if the clock is wrong).
    pub reset_in: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Ok { status: u16, body: String, rate: Rate },
    /// 429 / 5xx: wait (this many seconds, when it says) and ask again.
    Busy(Option<u64>),
    Unauthorized,
    /// Any other status (404: no such user; 400: bad question).
    Failed(u16),
    /// No answer at all.
    Offline,
}

pub trait Net {
    fn call(&mut self, req: &Req) -> Reply;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    Offline,
    /// Still busy after trying again a few times.
    Busy,
    Unauthorized,
    Refused(u16),
    Stopped,
}

impl Fail {
    /// A sentence for the person (never anything secret).
    pub fn say(&self) -> String {
        match self {
            Fail::Offline => "Couldn’t reach ListenBrainz. Check the connection and try again.".into(),
            Fail::Busy => "ListenBrainz is busy right now. Try again in a few minutes.".into(),
            Fail::Unauthorized => "ListenBrainz doesn’t accept the saved token. Connect it again in Settings.".into(),
            Fail::Refused(404) => "ListenBrainz doesn’t know that account.".into(),
            Fail::Refused(c) => format!("ListenBrainz answered {c}."),
            Fail::Stopped => "Stopped.".into(),
        }
    }
}

/// One request, paced and retried as above. `stop` is asked before each try.
pub fn request(net: &mut dyn Net, clock: &mut dyn Clock, pacer: &mut Pacer, req: &Req, stop: &dyn Fn() -> bool) -> Result<String, Fail> {
    let mut busy = 0u32;
    loop {
        if stop() {
            return Err(Fail::Stopped);
        }
        pacer.wait(clock);
        match net.call(req) {
            Reply::Ok { body, rate, .. } => {
                // None left in this window: wait it out before the next request.
                if rate.remaining == Some(0) {
                    clock.sleep_ms(rate.reset_in.unwrap_or(1).clamp(1, LONGEST_WAIT_SECS) * 1000);
                }
                return Ok(body);
            }
            Reply::Busy(after) => {
                busy += 1;
                if busy >= MAX_BUSY {
                    return Err(Fail::Busy);
                }
                clock.sleep_ms(after.unwrap_or(1 << busy).clamp(1, LONGEST_WAIT_SECS) * 1000);
            }
            Reply::Unauthorized => return Err(Fail::Unauthorized),
            Reply::Failed(code) => return Err(Fail::Refused(code)),
            Reply::Offline => return Err(Fail::Offline),
        }
    }
}

/// Real time. Sleeps in short steps so Stop and Pause take effect at once (`stop` is that flag).
pub struct Wall(pub std::time::Instant, pub &'static std::sync::atomic::AtomicBool);

impl Clock for Wall {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
    fn sleep_ms(&mut self, ms: u64) {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < end && !self.1.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(250).min(end - std::time::Instant::now()));
        }
    }
    fn unix(&self) -> u64 {
        crate::history::now()
    }
}

// ---------- the real network ----------

pub struct Http {
    client: reqwest::blocking::Client,
    token: Option<String>,
}

impl Http {
    /// None when the system can't make secure connections.
    pub fn new(token: Option<String>) -> Option<Http> {
        let client = crate::net::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(crate::musicbrainz::USER_AGENT)
            .build()
            .inspect_err(|e| log::warn!("no HTTP client for ListenBrainz: {e}"))
            .ok()?;
        Some(Http { client, token })
    }
}

impl Http {
    /// The token for the requests that need one; `None` once they're made (it isn't kept around).
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }
}

fn header_u64(r: &reqwest::blocking::Response, name: &str) -> Option<u64> {
    r.headers().get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok()).map(|v| v.max(0.0).ceil() as u64)
}

impl Net for Http {
    fn call(&mut self, req: &Req) -> Reply {
        let url = format!("{API}{}", req.path);
        let mut b = match req.method {
            Method::Get => self.client.get(url),
            Method::Post => self.client.post(url),
        };
        if req.auth {
            let Some(t) = &self.token else { return Reply::Unauthorized };
            b = b.header("Authorization", format!("Token {t}"));
        }
        if let Some(body) = &req.body {
            b = b.json(body);
        }
        let r = match b.send() {
            Ok(r) => r,
            Err(_) => return Reply::Offline,
        };
        let status = r.status().as_u16();
        let rate = Rate { remaining: header_u64(&r, "x-ratelimit-remaining"), reset_in: header_u64(&r, "x-ratelimit-reset-in") };
        match status {
            // (Read with a cap: no answer of these is anywhere near it.)
            200..=299 => crate::net::text(r).map(|body| Reply::Ok { status, body, rate }).unwrap_or(Reply::Failed(status)),
            401 => Reply::Unauthorized,
            408 | 429 | 500..=599 => Reply::Busy(header_u64(&r, "retry-after").or(rate.reset_in)),
            _ => Reply::Failed(status),
        }
    }
}

// ---------- the answers ----------

/// One listen as ListenBrainz keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Remote {
    pub at: u64,
    pub artist: String,
    pub title: String,
    pub album: String,
    /// Seconds, when the listen says (`additional_info.duration_ms` or `duration`).
    pub duration: Option<f64>,
    /// Its recording: ListenBrainz's own match (`mbid_mapping`), else what the submitter wrote.
    pub recording_mbid: Option<String>,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

fn mbid(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.trim().to_ascii_lowercase()).filter(|s| crate::tagids::is_mbid(s))
}

/// 2100-01-01: listens said to start later are left out.
const LATEST_LISTEN: u64 = 4_102_444_800;

/// `GET /user/<name>/listens`: its listens, newest first (those without a time or a title are left out).
pub fn parse_listens(body: &str) -> Option<Vec<Remote>> {
    let v: Value = serde_json::from_str(body).ok()?;
    let listens = v["payload"]["listens"].as_array()?;
    Some(
        listens
            .iter()
            .filter_map(|l| {
                let m = &l["track_metadata"];
                let info = &m["additional_info"];
                let (artist, title) = (text(&m["artist_name"]), text(&m["track_name"]));
                // (A time past 2100 is a broken listen, not one to file in a year of its own.)
                let at = l["listened_at"].as_u64().filter(|t| *t < LATEST_LISTEN)?;
                if artist.is_empty() || title.is_empty() {
                    return None;
                }
                let duration = info["duration_ms"].as_f64().map(|ms| ms / 1000.0).or_else(|| info["duration"].as_f64()).filter(|d| *d > 0.0);
                Some(Remote {
                    at,
                    artist,
                    title,
                    album: text(&m["release_name"]),
                    duration,
                    recording_mbid: mbid(&m["mbid_mapping"]["recording_mbid"]).or_else(|| mbid(&info["recording_mbid"])),
                })
            })
            .collect(),
    )
}

/// The oldest time among the raw listens of a page (also those parse_listens leaves out): where the
/// next page starts.
pub fn oldest_of(body: &str) -> Option<u64> {
    let v: Value = serde_json::from_str(body).ok()?;
    v["payload"]["listens"].as_array()?.iter().filter_map(|l| l["listened_at"].as_u64()).min()
}

/// A song loved (or hated) on ListenBrainz.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loved {
    pub recording_mbid: Option<String>,
    pub score: i64,
    pub artist: String,
    pub title: String,
    pub album: String,
}

/// `GET /feedback/user/<name>/get-feedback`: (the items of this page, how many there are in all).
pub fn parse_feedback(body: &str) -> Option<(Vec<Loved>, u64)> {
    let v: Value = serde_json::from_str(body).ok()?;
    let items = v["feedback"].as_array()?;
    let out: Vec<Loved> = items
        .iter()
        .map(|f| {
            let m = &f["track_metadata"];
            Loved {
                recording_mbid: mbid(&f["recording_mbid"]),
                score: f["score"].as_i64().unwrap_or(0),
                artist: text(&m["artist_name"]),
                title: text(&m["track_name"]),
                album: text(&m["release_name"]),
            }
        })
        .collect();
    let total = v["total_count"].as_u64().unwrap_or(out.len() as u64);
    Some((out, total))
}

/// A release on its way or just out.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Fresh {
    pub release_mbid: String,
    pub group_mbid: String,
    pub title: String,
    pub artist: String,
    pub artist_mbids: Vec<String>,
    /// "2026-11-14".
    pub date: String,
    /// "Album", "Single", "EP"…
    pub kind: String,
    pub caa_id: Option<u64>,
    pub caa_release_mbid: Option<String>,
}

/// `GET /explore/fresh-releases/`: a list, or one wrapped as `{"payload": {"releases": […]}}`.
pub fn parse_fresh(body: &str) -> Option<Vec<Fresh>> {
    let v: Value = serde_json::from_str(body).ok()?;
    let list = v.as_array().or_else(|| v["payload"]["releases"].as_array()).or_else(|| v["releases"].as_array())?;
    Some(
        list.iter()
            .filter_map(|r| {
                let (title, artist) = (text(&r["release_name"]), text(&r["artist_credit_name"]));
                if title.is_empty() || artist.is_empty() {
                    return None;
                }
                Some(Fresh {
                    release_mbid: mbid(&r["release_mbid"]).unwrap_or_default(),
                    group_mbid: mbid(&r["release_group_mbid"]).unwrap_or_default(),
                    title,
                    artist,
                    artist_mbids: r["artist_mbids"].as_array().into_iter().flatten().filter_map(mbid).collect(),
                    date: text(&r["release_date"]),
                    kind: text(&r["release_group_primary_type"]),
                    caa_id: r["caa_id"].as_u64(),
                    caa_release_mbid: mbid(&r["caa_release_mbid"]),
                })
            })
            .collect(),
    )
}

impl Fresh {
    /// A cover on the Cover Art Archive: the one the answer names, else the release group's front
    /// cover, else the release's.
    pub fn cover_url(&self) -> Option<String> {
        if let (Some(id), Some(rel)) = (self.caa_id, &self.caa_release_mbid) {
            return Some(format!("https://archive.org/download/mbid-{rel}/mbid-{rel}-{id}_thumb250.jpg"));
        }
        if !self.group_mbid.is_empty() {
            return Some(format!("https://coverartarchive.org/release-group/{}/front-250", self.group_mbid));
        }
        (!self.release_mbid.is_empty()).then(|| format!("https://coverartarchive.org/release/{}/front-250", self.release_mbid))
    }
}

/// `GET /metadata/lookup/`: the recording ListenBrainz knows for an artist and a title.
pub fn parse_lookup(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    mbid(&v["recording_mbid"])
}

// ---------- the questions ----------

pub fn listens_path(user: &str, before: Option<u64>) -> String {
    let mut p = format!("/user/{}/listens?count=1000", urlencoding::encode(user));
    if let Some(t) = before {
        p.push_str(&format!("&max_ts={t}"));
    }
    p
}

pub fn feedback_path(user: &str, offset: u64) -> String {
    format!("/feedback/user/{}/get-feedback?score=1&count=100&offset={offset}&metadata=true", urlencoding::encode(user))
}

pub fn fresh_path(days: u32) -> String {
    format!("/explore/fresh-releases/?days={}&past=true&future=true&sort=release_date", days.min(90))
}

pub fn lookup_path(artist: &str, title: &str) -> String {
    format!("/metadata/lookup/?artist_name={}&recording_name={}", urlencoding::encode(artist), urlencoding::encode(title))
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Answers in turn, and remembers what was asked (with the time on the fake clock).
    pub struct FakeNet {
        pub replies: RefCell<Vec<Reply>>,
        pub asked: Vec<(String, u64)>,
        pub clock: Rc<Cell<u64>>,
        /// Answers by path (checked first), for jobs that ask different things.
        pub by_path: Vec<(String, Reply)>,
    }
    impl Net for FakeNet {
        fn call(&mut self, req: &Req) -> Reply {
            self.asked.push((req.path.clone(), self.clock.get()));
            if let Some((_, r)) = self.by_path.iter().find(|(p, _)| *p == req.path) {
                return r.clone();
            }
            let mut r = self.replies.borrow_mut();
            if r.is_empty() { Reply::Offline } else { r.remove(0) }
        }
    }
    pub struct FakeClock(pub Rc<Cell<u64>>, pub Vec<u64>);
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
    pub fn fakes(replies: Vec<Reply>) -> (FakeNet, FakeClock) {
        let t = Rc::new(Cell::new(0));
        (FakeNet { replies: RefCell::new(replies), asked: Vec::new(), clock: t.clone(), by_path: Vec::new() }, FakeClock(t, Vec::new()))
    }
    pub fn ok(body: &str) -> Reply {
        Reply::Ok { status: 200, body: body.to_string(), rate: Rate::default() }
    }
    pub fn ok_rate(body: &str, remaining: u64, reset_in: u64) -> Reply {
        Reply::Ok { status: 200, body: body.to_string(), rate: Rate { remaining: Some(remaining), reset_in: Some(reset_in) } }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;

    const LISTENS: &str = r#"{"payload":{"count":3,"user_id":"me","listens":[
      {"listened_at":1790000300,"track_metadata":{"artist_name":"Nova Quartet","track_name":"Blue Hour","release_name":"Evenings",
        "additional_info":{"duration_ms":201000,"recording_mbid":"98255a8c-017a-4bc7-8dd6-1fa36124572b"},
        "mbid_mapping":{"recording_mbid":"30d08f4c-d825-4ae1-b79c-44242cddd7c0"}}},
      {"listened_at":1790000100,"track_metadata":{"artist_name":"Lumen","track_name":"Glass","additional_info":{"duration":180}}},
      {"track_metadata":{"artist_name":"No Time","track_name":"x"}}]}}"#;

    #[test]
    fn listens_are_read_with_their_length_and_recording() {
        let l = parse_listens(LISTENS).unwrap();
        assert_eq!(l.len(), 2, "the one without a time is left out");
        assert_eq!(l[0].at, 1_790_000_300);
        assert_eq!(l[0].duration, Some(201.0));
        assert_eq!(l[0].recording_mbid.as_deref(), Some("30d08f4c-d825-4ae1-b79c-44242cddd7c0"), "ListenBrainz's own match first");
        assert_eq!(l[1].duration, Some(180.0));
        assert_eq!(l[1].recording_mbid, None);
        assert_eq!(oldest_of(LISTENS), Some(1_790_000_100));
        assert!(parse_listens("nope").is_none());
    }

    #[test]
    fn loved_songs_and_fresh_releases_are_read() {
        let (items, total) = parse_feedback(r#"{"feedback":[{"score":1,"recording_mbid":"30d08f4c-d825-4ae1-b79c-44242cddd7c0","track_metadata":{"artist_name":"Lumen","track_name":"Glass","release_name":"Prism"}},{"score":1}],"total_count":250,"count":2,"offset":0}"#).unwrap();
        assert_eq!((items.len(), total), (2, 250));
        assert_eq!(items[0].artist, "Lumen");
        assert_eq!(items[1].recording_mbid, None);
        let rel = r#"{"release_name":"Prism","artist_credit_name":"Lumen","artist_mbids":["db92a151-1ac2-438b-bc43-b82e149ddd50"],"release_date":"2026-11-10","release_group_primary_type":"Album","release_mbid":"bf9e91ea-8029-4a04-a26a-224e00a83266","release_group_mbid":"1d98b0bc-5832-49d2-a93e-463032631a2f"}"#;
        for body in [format!("[{rel}]"), format!(r#"{{"payload":{{"releases":[{rel}]}}}}"#)] {
            let f = parse_fresh(&body).unwrap();
            assert_eq!(f.len(), 1);
            assert_eq!((f[0].title.as_str(), f[0].kind.as_str()), ("Prism", "Album"));
            assert_eq!(f[0].cover_url().unwrap(), "https://coverartarchive.org/release-group/1d98b0bc-5832-49d2-a93e-463032631a2f/front-250");
        }
        let named = Fresh { caa_id: Some(7), caa_release_mbid: Some("f1418001-7f1e-46af-bfdb-95faeded8841".into()), ..Default::default() };
        assert!(named.cover_url().unwrap().ends_with("mbid-f1418001-7f1e-46af-bfdb-95faeded8841-7_thumb250.jpg"));
        assert_eq!(Fresh::default().cover_url(), None);
        assert_eq!(parse_lookup(r#"{"recording_mbid":"30d08f4c-d825-4ae1-b79c-44242cddd7c0","recording_name":"x"}"#).as_deref(), Some("30d08f4c-d825-4ae1-b79c-44242cddd7c0"));
        assert_eq!(parse_lookup("{}"), None);
    }

    #[test]
    fn paths_are_escaped_and_carry_no_token() {
        assert_eq!(listens_path("a b", Some(5)), "/user/a%20b/listens?count=1000&max_ts=5");
        assert!(lookup_path("AC/DC", "T & C").contains("AC%2FDC"));
        assert!(fresh_path(500).contains("days=90"));
    }

    #[test]
    fn requests_are_paced_and_honour_rate_limit_headers_and_busy() {
        let req = Req::get("/x", false);
        let (mut net, mut clock) = fakes(vec![ok_rate("a", 0, 7), ok("b"), Reply::Busy(Some(3)), Reply::Busy(None), ok("c")]);
        let mut pacer = Pacer::default();
        let never = || false;
        assert_eq!(request(&mut net, &mut clock, &mut pacer, &req, &never).unwrap(), "a");
        assert!(clock.1.contains(&7_000), "none left: the window is waited out ({:?})", clock.1);
        assert_eq!(request(&mut net, &mut clock, &mut pacer, &req, &never).unwrap(), "b");
        assert_eq!(request(&mut net, &mut clock, &mut pacer, &req, &never).unwrap(), "c");
        assert!(clock.1.contains(&3_000), "Retry-After honoured ({:?})", clock.1);
        let times: Vec<u64> = net.asked.iter().map(|(_, t)| *t).collect();
        assert!(times.windows(2).all(|w| w[1] - w[0] >= crate::musicbrainz::GAP_MS), "{times:?}");
        // Busy for good, unauthorized, refused, offline, stopped.
        let (mut net, mut clock) = fakes(vec![Reply::Busy(None); 10]);
        assert_eq!(request(&mut net, &mut clock, &mut Pacer::default(), &req, &never), Err(Fail::Busy));
        for (reply, want) in [(Reply::Unauthorized, Fail::Unauthorized), (Reply::Failed(404), Fail::Refused(404)), (Reply::Offline, Fail::Offline)] {
            let (mut net, mut clock) = fakes(vec![reply]);
            assert_eq!(request(&mut net, &mut clock, &mut Pacer::default(), &req, &never), Err(want));
        }
        let (mut net, mut clock) = fakes(vec![ok("x")]);
        assert_eq!(request(&mut net, &mut clock, &mut Pacer::default(), &req, &|| true), Err(Fail::Stopped));
        assert!(net.asked.is_empty());
    }
}
