//! Updates for the native Windows app (the Tauri app has tauri-plugin-updater): the same signed
//! releases on GitHub (latest.json), checked against the same minisign key, installed the same
//! way: the NSIS installer runs passively in update mode and starts Cadence again when it's done.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};

const ENDPOINT: &str = "https://github.com/akbarharyadi/cadence/releases/latest/download/latest.json";
/// tauri.conf.json → plugins.updater.pubkey (base64 of the minisign public key file).
const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDFFQjBCRjM2NjA4QjczNkMKUldSc2M0dGdOcit3SGhOVW9qc1hqQXJ4ZWJMcW4vazZvRTNLWWQvNEd6MkY3TDkwOTcxRDBPbFgK";

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: String,
    platforms: std::collections::HashMap<String, Platform>,
}

#[derive(Deserialize, Clone)]
struct Platform {
    signature: String,
    url: String,
}

/// A newer version on offer.
#[derive(Serialize, Clone)]
pub struct Available {
    pub version: String,
    pub notes: String,
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(crate::net::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION"), " (open-source music player)"))
        .build()?)
}

fn manifest() -> Result<(Manifest, Platform)> {
    let m: Manifest = client()?.get(ENDPOINT).send()?.error_for_status()?.json()?;
    let p = ["windows-x86_64-nsis", "windows-x86_64"]
        .iter()
        .find_map(|k| m.platforms.get(*k).cloned())
        .ok_or_else(|| anyhow!("no Windows build in this release"))?;
    Ok((m, p))
}

/// "0.2.10" is newer than "0.2.9".
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |s: &str| s.trim_start_matches('v').split(['.', '-']).map(|n| n.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// This version (CADENCE_UPDATE_FROM pretends to be another, to try the updater).
fn current() -> String {
    std::env::var("CADENCE_UPDATE_FROM").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

/// The newer version on GitHub, if there is one.
pub fn check() -> Result<Option<Available>> {
    let (m, _) = manifest()?;
    Ok(newer(&m.version, &current()).then(|| Available { version: m.version, notes: m.notes }))
}

/// Downloads the newer version's installer into `dir` and checks its signature; the installer's
/// path. `progress(fraction)` is called as it arrives.
pub fn download(dir: &Path, mut progress: impl FnMut(f64)) -> Result<PathBuf> {
    let (m, p) = manifest()?;
    if !newer(&m.version, &current()) {
        bail!("Cadence {} is the latest version", current());
    }
    let mut resp = fetch(&p.url)?;
    let total = resp.content_length().unwrap_or(0);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut buf = vec![0; 256 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if total > 0 {
            progress(bytes.len() as f64 / total as f64);
        }
    }
    verify(&bytes, &p.signature).context("the download isn't a genuine Cadence release")?;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("Cadence_{}_x64-setup.exe", m.version));
    std::fs::write(&path, &bytes)?;
    Ok(path)
}

/// The release's files are GitHub API asset links: they send the file only when asked for bytes.
fn fetch(url: &str) -> Result<reqwest::blocking::Response> {
    Ok(client()?.get(url).header(reqwest::header::ACCEPT, "application/octet-stream").send()?.error_for_status()?)
}

/// Checks `data` against a minisign signature (base64 of the .sig file) made with Cadence's key.
fn verify(data: &[u8], signature: &str) -> Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let key = String::from_utf8(b64.decode(PUBKEY)?)?;
    let sig = String::from_utf8(b64.decode(signature.trim())?)?;
    let key = minisign_verify::PublicKey::decode(&key).map_err(|e| anyhow!("{e}"))?;
    let sig = minisign_verify::Signature::decode(&sig).map_err(|e| anyhow!("{e}"))?;
    key.verify(data, &sig, true).map_err(|e| anyhow!("{e}"))
}

/// Starts the installer: passive (a progress bar, no questions), in update mode, starting Cadence
/// again afterwards. The caller quits right away so the installer can replace the files.
pub fn install(installer: &Path) -> Result<()> {
    std::process::Command::new(installer)
        .args(["/P", "/UPDATE", "/R"])
        .spawn()
        .with_context(|| format!("couldn't start {}", installer.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.2.10", "0.2.9"));
        assert!(newer("v0.3.0", "0.2.25"));
        assert!(!newer("0.2.25", "0.2.25"));
        assert!(!newer("0.2.24", "0.2.25"));
    }

    /// The real latest release's Windows installer passes, and one changed byte fails
    /// (network; `cargo test --features ffi --lib update -- --ignored`).
    #[test]
    #[ignore]
    fn genuine_release() {
        let (_, p) = manifest().unwrap();
        let mut bytes = Vec::new();
        fetch(&p.url).unwrap().read_to_end(&mut bytes).unwrap();
        verify(&bytes, &p.signature).unwrap();
        bytes[1000] ^= 1;
        assert!(verify(&bytes, &p.signature).is_err());
    }

    #[test]
    fn rejects_unsigned() {
        // A signature for other bytes (or none) never passes.
        assert!(verify(b"not an installer", "").is_err());
    }
}
