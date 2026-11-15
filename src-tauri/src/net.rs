//! HTTP clients for online lookups (lyrics, AI DJ, catalogue, scrobbling).
//!
//! On Android, reqwest's default certificate check goes through Android's verifier, which must be
//! set up from Java first; without that every HTTPS request panics. Cadence instead trusts exactly
//! the certificates the phone itself trusts, read from the system's certificate folder.

/// A client builder that works on every platform.
pub fn builder() -> reqwest::blocking::ClientBuilder {
    let b = reqwest::blocking::Client::builder();
    #[cfg(target_os = "android")]
    let b = b.tls_certs_only(android_roots().iter().cloned());
    b
}

/// The most a JSON answer of the online services Cadence asks may be: far more than any real one
/// (ListenBrainz's biggest pages are a few hundred KB), so a broken or hostile server can't fill
/// the memory.
pub const MAX_BODY: u64 = 16 << 20;

/// Reads at most `limit` bytes of a body; more is an error, not a cut-off answer.
pub fn read_limited(r: impl std::io::Read, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    r.take(limit + 1).read_to_end(&mut out)?;
    if out.len() as u64 > limit {
        return Err(std::io::Error::other(format!("the answer is larger than {} MB", limit >> 20)));
    }
    Ok(out)
}

/// A response's body as text, at most `MAX_BODY` bytes.
pub fn text(r: reqwest::blocking::Response) -> std::io::Result<String> {
    let bytes = read_limited(r, MAX_BODY)?;
    String::from_utf8(bytes).map_err(std::io::Error::other)
}

/// A response's body as JSON, at most `MAX_BODY` bytes.
pub fn json<T: serde::de::DeserializeOwned>(r: reqwest::blocking::Response) -> std::io::Result<T> {
    let bytes = read_limited(r, MAX_BODY)?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    #[test]
    fn bodies_are_read_up_to_a_limit() {
        assert_eq!(super::read_limited(&b"12345"[..], 5).unwrap(), b"12345");
        assert!(super::read_limited(&b"123456"[..], 5).is_err());
        assert!(super::read_limited(std::io::repeat(b'x'), 1 << 20).is_err(), "an endless body stops at the limit");
    }
}

/// The phone's trusted certificate authorities (PEM files), read once.
#[cfg(target_os = "android")]
fn android_roots() -> &'static Vec<reqwest::Certificate> {
    static ROOTS: std::sync::OnceLock<Vec<reqwest::Certificate>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        // Android 14+ keeps them in the Conscrypt module; older versions in /system.
        for dir in ["/apex/com.android.conscrypt/cacerts", "/system/etc/security/cacerts"] {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let certs: Vec<_> = rd
                .flatten()
                .filter_map(|e| std::fs::read(e.path()).ok())
                .filter_map(|pem| reqwest::Certificate::from_pem(&pem).ok())
                .collect();
            if !certs.is_empty() {
                log::info!("{} trusted certificates from {dir}", certs.len());
                return certs;
            }
        }
        log::warn!("no system certificates found; online lookups will fail");
        Vec::new()
    })
}
