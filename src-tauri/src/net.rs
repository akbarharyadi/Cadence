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
