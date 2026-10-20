//! Tokens and API keys, kept in the system's credential store (Windows Credential Manager, macOS
//! Keychain, Linux Secret Service) rather than in settings files. On Android, where apps have no shared
//! credential store, they live in a file in the app's private storage, which other apps can't read.

const SERVICE: &str = "Cadence";

#[cfg(not(target_os = "android"))]
pub fn get(name: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, name)
        .ok()?
        .get_password()
        .ok()
        .filter(|s| !s.is_empty())
}

#[cfg(not(target_os = "android"))]
pub fn set(name: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, name).map_err(|e| e.to_string())?;
    if value.trim().is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    } else {
        entry.set_password(value.trim()).map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "android")]
static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Where secrets are kept on Android (the app's private data folder); set once at startup.
#[cfg(target_os = "android")]
pub fn init(data_dir: &std::path::Path) {
    let _ = DIR.set(data_dir.to_path_buf());
}

#[cfg(target_os = "android")]
fn load() -> std::collections::HashMap<String, String> {
    DIR.get()
        .and_then(|d| std::fs::read_to_string(d.join("secrets.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[cfg(target_os = "android")]
pub fn get(name: &str) -> Option<String> {
    let _ = SERVICE;
    load().remove(name).filter(|s| !s.is_empty())
}

#[cfg(target_os = "android")]
pub fn set(name: &str, value: &str) -> Result<(), String> {
    let dir = DIR.get().ok_or("Storage isn't ready yet")?;
    let mut all = load();
    if value.trim().is_empty() {
        all.remove(name);
    } else {
        all.insert(name.to_string(), value.trim().to_string());
    }
    std::fs::write(dir.join("secrets.json"), serde_json::to_vec(&all).unwrap_or_default()).map_err(|e| e.to_string())
}
