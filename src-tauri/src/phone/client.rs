//! The phone's side, talking to the computer: pairing with its code, reading the music list, and
//! downloading one song (resumed where it stopped, checked against its key before it's used).
//!
//! The paired computer (its address, the token, this phone's id) is kept with the secrets, so the
//! token is never in a settings file or a backup.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::manifest::{Entry, Manifest};

const LINK: &str = "phone-sync-link";
/// The largest music list read (a list of 100,000 songs is about 30 MB).
const MAX_LIST: u64 = 64 << 20;
const CHUNK: usize = 256 << 10;

/// The computer this phone is paired with.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Link {
    /// "192.168.1.20:47300".
    pub address: String,
    pub token: String,
    pub computer: String,
    /// This phone's own id (kept when it's paired again).
    pub phone_id: String,
}

impl Link {
    pub fn paired(&self) -> bool {
        !self.address.is_empty() && !self.token.is_empty()
    }
}

pub fn load_link() -> Link {
    crate::secrets::get(LINK).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_link(l: &Link) -> Result<(), String> {
    crate::secrets::set(LINK, &serde_json::to_string(l).map_err(|e| e.to_string())?)
}

/// Forgets the computer (this phone's id stays, so pairing again replaces the old pairing there).
pub fn forget() -> Result<(), String> {
    let l = load_link();
    save_link(&Link { phone_id: l.phone_id, ..Default::default() })
}

/// "192.168.1.20", "http://192.168.1.20:47300/" → "192.168.1.20:47300". Err for anything that
/// isn't a plain host and port.
pub fn normalize_address(input: &str) -> Result<String, String> {
    let bad = || "That address doesn't look right. Type it as the computer shows it, like 192.168.1.20:47300.".to_string();
    let s = input.trim().trim_start_matches("http://").trim_end_matches('/');
    if s.is_empty() || s.len() > 100 || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-')) {
        return Err(bad());
    }
    match s.split_once(':') {
        None => Ok(format!("{s}:{}", super::PORT)),
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0) => Ok(s.to_string()),
        Some(_) => Err(bad()),
    }
}

fn http(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    crate::net::builder()
        .timeout(timeout)
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

fn unreachable(e: reqwest::Error, address: &str) -> String {
    log::warn!("phone sync: {address}: {e}");
    format!("Couldn't reach the computer at {address}. Check that it's on, that Phone sync is on there, and that both are on the same Wi-Fi (or the cable with USB tethering).")
}

/// The words the computer answered with, for a refusal.
fn refusal(r: reqwest::blocking::Response) -> String {
    let status = r.status().as_u16();
    let text = crate::net::text(r).unwrap_or_default();
    let text: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
    if text.trim().is_empty() { format!("The computer said no ({status}).") } else { text }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairAsk<'a> {
    code: &'a str,
    phone_id: &'a str,
    name: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairAnswer {
    token: String,
    computer: String,
}

/// Pairs with the computer at `address` using the code it shows; keeps the link.
pub fn pair(address: &str, code: &str, name: &str) -> Result<Link, String> {
    let address = normalize_address(address)?;
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 {
        return Err("The code has 6 digits.".into());
    }
    let mut link = load_link();
    if link.phone_id.is_empty() {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|e| e.to_string())?;
        link.phone_id = b.iter().map(|x| format!("{x:02x}")).collect();
    }
    let client = http(Duration::from_secs(15))?;
    let r = client
        .post(format!("http://{address}/pair"))
        .json(&PairAsk { code: &code, phone_id: &link.phone_id, name })
        .send()
        .map_err(|e| unreachable(e, &address))?;
    if !r.status().is_success() {
        return Err(refusal(r));
    }
    let a: PairAnswer = crate::net::json(r).map_err(|_| "The computer's answer wasn't understood. Update Cadence on both.".to_string())?;
    if a.token.len() < 32 {
        return Err("The computer's answer wasn't understood. Update Cadence on both.".into());
    }
    link = Link { address, token: a.token, computer: a.computer.chars().filter(|c| !c.is_control()).take(80).collect(), phone_id: link.phone_id };
    save_link(&link)?;
    Ok(link)
}

/// The music list from the computer.
pub fn fetch_list(link: &Link) -> Result<Manifest, String> {
    let client = http(Duration::from_secs(60))?;
    let r = client
        .get(format!("http://{}/manifest", link.address))
        .bearer_auth(&link.token)
        .send()
        .map_err(|e| unreachable(e, &link.address))?;
    if r.status().as_u16() == 401 {
        return Err("This phone isn't paired with the computer anymore. Pair it again.".into());
    }
    if !r.status().is_success() {
        return Err(refusal(r));
    }
    let bytes = crate::net::read_limited(r, MAX_LIST).map_err(|e| format!("The music list didn't arrive whole ({e})."))?;
    serde_json::from_slice(&bytes).map_err(|_| "The computer's music list wasn't understood. Update Cadence on both.".into())
}

/// The file a download goes to until it's whole and checked.
pub fn part_of(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    target.with_file_name(name)
}

/// The shared client for downloads (no overall time limit: a song can take minutes).
pub fn download_client() -> Result<reqwest::blocking::Client, String> {
    crate::net::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Downloads `e` to `target`: into its `.part` file (continuing one left by an earlier try), then,
/// once it has every byte and its key matches, renamed into place. `progress` gets the bytes as
/// they arrive; `stop` ends it early (the `.part` stays for next time).
pub fn download(client: &reqwest::blocking::Client, link: &Link, e: &Entry, target: &Path, stop: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    let part = part_of(target);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("Couldn't make the folder {}: {err}", dir.display()))?;
    }
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > e.size {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    if have < e.size {
        fetch(client, link, e, &part, have, stop, progress)?;
    }
    if stop.load(Ordering::SeqCst) {
        return Err("Stopped.".into());
    }
    let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let key = super::keys::file_key(&part).map_err(|err| err.to_string())?;
    if got != e.size || key != e.key {
        let _ = std::fs::remove_file(&part);
        return Err(format!("“{}” arrived damaged; it's fetched again next time.", e.title));
    }
    std::fs::rename(&part, target).map_err(|err| format!("Couldn't put “{}” in place: {err}", e.title))
}

/// Appends the bytes from `from` on to `part`, bounded by the song's size.
fn fetch(client: &reqwest::blocking::Client, link: &Link, e: &Entry, part: &Path, from: u64, stop: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    let mut ask = client.get(format!("http://{}/song/{}", link.address, e.key)).bearer_auth(&link.token);
    if from > 0 {
        ask = ask.header("Range", format!("bytes={from}-"));
    }
    let mut r = ask.send().map_err(|err| unreachable(err, &link.address))?;
    let status = r.status().as_u16();
    let start = match status {
        206 => from,
        200 => 0,
        410 => return Err(format!("“{}” is gone from the computer.", e.title)),
        _ => return Err(refusal(r)),
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(start > 0)
        .truncate(start == 0)
        .open(part)
        .map_err(|err| format!("Couldn't write to {}: {err}", part.display()))?;
    progress(start);
    let mut written = start;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let n = r.read(&mut buf).map_err(|err| format!("The connection dropped ({err}); it carries on next time."))?;
        if n == 0 {
            break;
        }
        // Never more than the song's size, whatever the other side sends.
        written = written.saturating_add(n as u64);
        if written > e.size {
            let _ = std::fs::remove_file(part);
            return Err(format!("“{}” came bigger than expected; it's fetched again next time.", e.title));
        }
        file.write_all(buf.get(..n).unwrap_or_default()).map_err(|err| format!("Couldn't write “{}”: {err}. Is the phone full?", e.title))?;
        progress(n as u64);
    }
    file.sync_all().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_read_the_way_people_type_them() {
        assert_eq!(normalize_address("192.168.1.20").unwrap(), "192.168.1.20:47300");
        assert_eq!(normalize_address(" http://192.168.1.20:47301/ ").unwrap(), "192.168.1.20:47301");
        assert_eq!(normalize_address("desk-pc.local").unwrap(), "desk-pc.local:47300");
        for bad in ["", "192.168.1.20/../x", "a b", "http://x/y", "x;rm", "x:", "x:99999", ":80", "a:1:2"] {
            assert!(normalize_address(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_part_file_sits_next_to_the_song() {
        assert_eq!(part_of(Path::new("/m/A/01 One.flac")), Path::new("/m/A/01 One.flac.part"));
    }
}
