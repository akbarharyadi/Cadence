//! The computer's side of Sync with phone: a small HTTP service on the local network, on only
//! while the listener has Phone sync turned on (off by default: it opens a port).
//!
//! - `POST /pair` {code, phoneId, name}: the 6-digit code shown on the computer (for 10 minutes,
//!   5 tries) gets the phone a random token. Only the token's hash is kept, in the keyring.
//! - `GET /manifest`: the music list (see `manifest`), with `Authorization: Bearer <token>`.
//! - `GET /song/<key>`: one song's file, by its key, with `Range` for resuming. Only songs on the
//!   current list can be asked for, so no other file on the computer can be reached.
//!
//! Only addresses of private networks (home Wi-Fi, USB tethering) are answered. Each request runs
//! on a thread of its own, at most `MAX_BUSY` at a time.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use super::{computer_name, manifest::Manifest, now};
use crate::ctx::Ctx;

/// The port tried first, then the next few.
pub const PORT: u16 = 47300;
const PORT_TRIES: u16 = 10;
const PAIR_SECS: u64 = 600;
const PAIR_TRIES: u32 = 5;
/// The keyring entry: phone id → hash of its token.
const TOKENS: &str = "phone-sync-tokens";
const MAX_BUSY: usize = 4;
const MAX_BODY: u64 = 4096;

struct Running {
    server: Arc<Server>,
    port: u16,
}

struct Pairing {
    code: String,
    until: u64,
    tries: u32,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static PAIRING: Mutex<Option<Pairing>> = Mutex::new(None);
/// The files the current list names (key → path): the only files `/song` hands out.
static FILES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static BUSY: AtomicUsize = AtomicUsize::new(0);

/// What the settings page shows.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub on: bool,
    /// "192.168.1.20:47300", what to type on the phone (empty when off or unknown).
    pub address: String,
    pub computer: String,
    /// The pairing code, while pairing is open.
    pub code: Option<String>,
    pub code_until: u64,
}

pub fn status() -> ServerStatus {
    let port = RUNNING.lock().as_ref().map(|r| r.port);
    let pairing = PAIRING.lock();
    let open = pairing.as_ref().filter(|p| p.until > now());
    ServerStatus {
        on: port.is_some(),
        address: port.and_then(|p| local_ip().map(|ip| format!("{ip}:{p}"))).unwrap_or_default(),
        computer: computer_name(),
        code: open.map(|p| p.code.clone()),
        code_until: open.map_or(0, |p| p.until),
    }
}

/// This computer's address on the local network (the one its default route uses; nothing is sent).
fn local_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// Starts the service (if it isn't on). Err: no port could be opened.
pub fn start(ctx: &Ctx) -> Result<(), String> {
    let mut running = RUNNING.lock();
    if running.is_some() {
        return Ok(());
    }
    let (server, port) = (PORT..PORT + PORT_TRIES)
        .find_map(|p| Server::http(("0.0.0.0", p)).ok().map(|s| (Arc::new(s), p)))
        .ok_or("Couldn't open a network port for Phone sync. Another program may be using it.")?;
    let (srv, ctx) = (server.clone(), ctx.clone());
    std::thread::Builder::new()
        .name("cadence-phone-sync".into())
        .spawn(move || serve(&srv, &ctx))
        .map_err(|e| format!("Couldn't start Phone sync: {e}"))?;
    log::info!("phone sync listening on port {port}");
    *running = Some(Running { server, port });
    Ok(())
}

pub fn stop() {
    if let Some(r) = RUNNING.lock().take() {
        r.server.unblock();
    }
    *PAIRING.lock() = None;
    *FILES.lock() = None;
}

/// Opens pairing: a new 6-digit code for 10 minutes.
pub fn start_pairing() -> Result<String, String> {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).map_err(|e| format!("Couldn't make a pairing code: {e}"))?;
    let code = format!("{:06}", u32::from_le_bytes(b) % 1_000_000);
    *PAIRING.lock() = Some(Pairing { code: code.clone(), until: now() + PAIR_SECS, tries: 0 });
    Ok(code)
}

/// Forgets a phone: its token stops working and it's off the list.
pub fn unpair(ctx: &Ctx, phone_id: &str) -> Result<(), String> {
    let mut tokens = tokens();
    tokens.remove(phone_id);
    save_tokens(&tokens)?;
    super::change(&ctx.state.data_dir, |c| c.phones.retain(|p| p.id != phone_id));
    Ok(())
}

fn tokens() -> HashMap<String, String> {
    crate::secrets::get(TOKENS).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_tokens(t: &HashMap<String, String>) -> Result<(), String> {
    crate::secrets::set(TOKENS, &serde_json::to_string(t).map_err(|e| e.to_string())?)
}

fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

fn serve(server: &Server, ctx: &Ctx) {
    for req in server.incoming_requests() {
        if BUSY.load(Ordering::SeqCst) >= MAX_BUSY {
            let _ = req.respond(Response::from_string("Busy").with_status_code(503));
            continue;
        }
        BUSY.fetch_add(1, Ordering::SeqCst);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("cadence-phone-request".into()).spawn(move || {
            handle(req, &ctx);
            BUSY.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            BUSY.fetch_sub(1, Ordering::SeqCst);
        }
    }
    log::info!("phone sync stopped");
}

/// A home network, USB tethering or this computer itself; nothing from the internet.
pub(crate) fn private(addr: Option<&SocketAddr>) -> bool {
    match addr.map(SocketAddr::ip) {
        Some(IpAddr::V4(ip)) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        Some(IpAddr::V6(ip)) => {
            let first = ip.segments().first().copied().unwrap_or(0);
            ip.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_private())
        }
        None => false,
    }
}

fn text(status: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(msg).with_status_code(status)
}

fn json<T: Serialize>(v: &T) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(v).unwrap_or_default();
    let mut r = Response::from_data(body);
    if let Ok(h) = Header::from_bytes("Content-Type", "application/json") {
        r = r.with_header(h);
    }
    r
}

fn handle(mut req: Request, ctx: &Ctx) {
    if !private(req.remote_addr()) {
        let _ = req.respond(text(403, "Only phones on your own network can sync."));
        return;
    }
    let url = req.url().to_string();
    let r = match (req.method().clone(), url.as_str()) {
        (Method::Post, "/pair") => {
            let mut body = String::new();
            let _ = req.as_reader().take(MAX_BODY).read_to_string(&mut body);
            pair(ctx, &body)
        }
        (Method::Get, "/manifest") => match phone_of(&req) {
            Some(phone) => json(&manifest_for(ctx, &phone)),
            None => text(401, "This phone isn't paired."),
        },
        (Method::Get, u) if u.starts_with("/song/") => {
            if phone_of(&req).is_none() {
                text(401, "This phone isn't paired.")
            } else {
                return send_song(req, u.trim_start_matches("/song/"));
            }
        }
        _ => text(404, "Not found."),
    };
    let _ = req.respond(r);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairAsk {
    code: String,
    phone_id: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairAnswer {
    token: String,
    computer: String,
}

fn pair(ctx: &Ctx, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let Ok(ask) = serde_json::from_str::<PairAsk>(body) else {
        return text(400, "Bad pairing request.");
    };
    let phone_id: String = ask.phone_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect();
    if phone_id.is_empty() {
        return text(400, "Bad pairing request.");
    }
    if let Err(msg) = check_code(&ask.code) {
        return text(403, msg);
    }
    let mut b = [0u8; 32];
    if getrandom::fill(&mut b).is_err() {
        return text(500, "Couldn't pair. Try again.");
    }
    let token: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let mut tokens = tokens();
    tokens.insert(phone_id.clone(), token_hash(&token));
    if let Err(e) = save_tokens(&tokens) {
        log::error!("can't keep the phone's token: {e}");
        return text(500, "Couldn't pair. Try again.");
    }
    let name: String = ask.name.chars().filter(|c| !c.is_control()).take(60).collect();
    super::change(&ctx.state.data_dir, |c| {
        c.phones.retain(|p| p.id != phone_id);
        c.phones.push(super::Phone { id: phone_id.clone(), name, paired: now(), last_seen: 0 });
    });
    ctx.emit("phone-paired", ());
    json(&PairAnswer { token, computer: computer_name() })
}

/// The code matches the open pairing. A wrong code uses a try; the last one closes pairing.
fn check_code(code: &str) -> Result<(), &'static str> {
    let mut p = PAIRING.lock();
    let Some(open) = p.as_mut().filter(|o| o.until > now()) else {
        *p = None;
        return Err("Pairing isn't open. Start it on the computer first.");
    };
    if code.trim() == open.code {
        *p = None;
        return Ok(());
    }
    open.tries += 1;
    if open.tries >= PAIR_TRIES {
        *p = None;
        return Err("Wrong code too many times. Start pairing again on the computer.");
    }
    Err("Wrong code.")
}

/// The paired phone the request's token belongs to.
fn phone_of(req: &Request) -> Option<String> {
    let auth = req.headers().iter().find(|h| h.field.equiv("Authorization"))?;
    let token = auth.value.as_str().strip_prefix("Bearer ")?.trim();
    let hash = token_hash(token);
    tokens().into_iter().find(|(_, h)| *h == hash).map(|(id, _)| id)
}

fn manifest_for(ctx: &Ctx, phone: &str) -> Manifest {
    let (m, files) = super::manifest_now(ctx);
    *FILES.lock() = Some(files);
    super::change(&ctx.state.data_dir, |c| {
        if let Some(p) = c.phones.iter_mut().find(|p| p.id == phone) {
            p.last_seen = now();
        }
    });
    ctx.emit("phone-synced", ());
    m
}

/// "bytes=START-" or "bytes=START-END" within a file of `size` bytes: (start, length).
pub(crate) fn range(header: Option<&str>, size: u64) -> Option<(u64, u64)> {
    let spec = header?.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let start: u64 = a.trim().parse().ok()?;
    let end: u64 = if b.trim().is_empty() { size.checked_sub(1)? } else { b.trim().parse::<u64>().ok()?.min(size.checked_sub(1)?) };
    (start <= end && start < size).then(|| (start, end - start + 1))
}

fn send_song(req: Request, key: &str) {
    let path = FILES.lock().as_ref().and_then(|f| f.get(key).cloned());
    let Some(path) = path else {
        let _ = req.respond(text(404, "That song isn't on the list. Sync again."));
        return;
    };
    let Ok(mut file) = std::fs::File::open(&path) else {
        let _ = req.respond(text(410, "The song's file is gone from the computer."));
        return;
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let asked = req.headers().iter().find(|h| h.field.equiv("Range")).map(|h| h.value.as_str().to_string());
    let (status, start, len) = match asked {
        Some(r) => match range(Some(&r), size) {
            Some((s, l)) => (206, s, l),
            None => {
                let _ = req.respond(text(416, "Bad range."));
                return;
            }
        },
        None => (200, 0, size),
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        let _ = req.respond(text(500, "Couldn't read the song."));
        return;
    }
    let mut headers = Vec::new();
    if status == 206
        && let Ok(h) = Header::from_bytes("Content-Range", format!("bytes {start}-{}/{size}", start + len - 1))
    {
        headers.push(h);
    }
    if let Ok(h) = Header::from_bytes("Content-Type", "application/octet-stream") {
        headers.push(h);
    }
    let len_usize = usize::try_from(len).ok();
    let _ = req.respond(Response::new(StatusCode(status), headers, file.take(len), len_usize, None));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_private_networks_are_answered() {
        let at = |s: &str| s.parse::<SocketAddr>().ok();
        for ok in ["192.168.1.5:1", "10.0.0.2:1", "172.20.1.1:1", "127.0.0.1:1", "169.254.3.3:1", "[fe80::1]:1", "[fd00::5]:1", "[::1]:1"] {
            assert!(private(at(ok).as_ref()), "{ok}");
        }
        for no in ["8.8.8.8:1", "172.32.0.1:1", "[2001:db8::1]:1"] {
            assert!(!private(at(no).as_ref()), "{no}");
        }
        assert!(!private(None));
    }

    #[test]
    fn ranges_for_resuming_a_song() {
        assert_eq!(range(Some("bytes=0-"), 100), Some((0, 100)));
        assert_eq!(range(Some("bytes=40-"), 100), Some((40, 60)));
        assert_eq!(range(Some("bytes=10-19"), 100), Some((10, 10)));
        assert_eq!(range(Some("bytes=90-500"), 100), Some((90, 10)));
        assert_eq!(range(Some("bytes=100-"), 100), None);
        assert_eq!(range(Some("bytes=-5"), 100), None);
        assert_eq!(range(Some("bytes=0-"), 0), None);
        assert_eq!(range(Some("items=0-"), 100), None);
    }

    #[test]
    fn a_song_is_sent_whole_or_from_where_it_stopped_and_nothing_else_is() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.flac");
        std::fs::write(&f, b"0123456789").unwrap();
        *FILES.lock() = Some([("k1".to_string(), f.to_string_lossy().into_owned())].into());
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests().take(3) {
                let key = req.url().trim_start_matches("/song/").to_string();
                send_song(req, &key);
            }
        });
        let http = reqwest::blocking::Client::new();
        let url = |k: &str| format!("http://127.0.0.1:{port}/song/{k}");
        let whole = http.get(url("k1")).send().unwrap();
        assert_eq!(whole.status().as_u16(), 200);
        assert_eq!(whole.bytes().unwrap().as_ref(), b"0123456789");
        let rest = http.get(url("k1")).header("Range", "bytes=4-").send().unwrap();
        assert_eq!(rest.status().as_u16(), 206);
        assert_eq!(rest.headers().get("Content-Range").unwrap(), "bytes 4-9/10");
        assert_eq!(rest.bytes().unwrap().as_ref(), b"456789");
        assert_eq!(http.get(url("..%2F..%2Fsecret")).send().unwrap().status().as_u16(), 404);
    }

    #[test]
    fn a_wrong_code_uses_a_try_and_the_right_one_closes_pairing() {
        *PAIRING.lock() = Some(Pairing { code: "123456".into(), until: now() + 60, tries: 0 });
        assert_eq!(check_code("000000"), Err("Wrong code."));
        assert_eq!(check_code(" 123456 "), Ok(()));
        assert!(check_code("123456").is_err(), "a code works once");
        *PAIRING.lock() = Some(Pairing { code: "123456".into(), until: now() + 60, tries: PAIR_TRIES - 1 });
        assert!(check_code("1").is_err());
        assert!(PAIRING.lock().is_none(), "too many tries close pairing");
        *PAIRING.lock() = Some(Pairing { code: "123456".into(), until: now().saturating_sub(1), tries: 0 });
        assert!(check_code("123456").is_err(), "an old code doesn't work");
    }
}
