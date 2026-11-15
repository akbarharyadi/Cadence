//! Tokens and API keys, kept in the system's credential store (Windows Credential Manager, macOS
//! Keychain, Linux Secret Service) rather than in settings files. On Android, where apps have no shared
//! credential store, they live in a file in the app's private storage, which other apps can't read.

/// The keyring service name every release has always used. Existing installs keep their secrets under it.
const LEGACY_SERVICE: &str = "Cadence";
/// The identifier of the shipped app (`identifier` in tauri.conf.json; a test pins the two together).
const PRODUCTION_ID: &str = "com.cadence.player";

static IDENTIFIER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Remember the running app's identifier; called first thing in setup, before any secret is read.
pub fn init_identifier(identifier: &str) {
    let _ = IDENTIFIER.set(identifier.to_string());
}

/// The keyring service for an app identifier. The shipped identifier keeps the legacy name; any other
/// build (a test copy, a dev build) gets its own, so it starts empty and can never read or overwrite the
/// real token and keys.
fn service_name(identifier: &str) -> String {
    let id = identifier.trim();
    if id == PRODUCTION_ID {
        return LEGACY_SERVICE.to_string();
    }
    if id.is_empty() {
        return format!("{LEGACY_SERVICE} (unknown)");
    }
    let id: String = id.chars().map(|c| if c.is_control() { '_' } else { c }).collect();
    format!("{LEGACY_SERVICE} ({id})")
}

/// None until `init_identifier` ran: better no secret than one from the wrong namespace.
#[cfg(not(target_os = "android"))]
fn entry(name: &str) -> Result<keyring::Entry, String> {
    let id = IDENTIFIER.get().ok_or("Storage isn't ready yet")?;
    keyring::Entry::new(&service_name(id), name).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "android"))]
pub fn get(name: &str) -> Option<String> {
    entry(name)
        .ok()?
        .get_password()
        .ok()
        .filter(|s| !s.is_empty())
}

#[cfg(not(target_os = "android"))]
pub fn set(name: &str, value: &str) -> Result<(), String> {
    let entry = entry(name)?;
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
    // Already per app: the file lives in this identifier's own data folder.
    let _ = service_name;
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
    let bytes = serde_json::to_vec(&all).map_err(|e| e.to_string())?;
    // Whole or not at all (no backup copy: a removed key must not linger in one).
    crate::state::write_file(&dir.join("secrets.json"), &bytes, false).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_identifier_keeps_the_legacy_service_name() {
        assert_eq!(service_name("com.cadence.player"), "Cadence");
        assert_eq!(service_name("  com.cadence.player "), "Cadence");
    }

    #[test]
    fn the_constant_matches_the_identifier_in_the_tauri_config() {
        let conf = include_str!("../tauri.conf.json");
        assert!(conf.contains(&format!("\"identifier\": \"{PRODUCTION_ID}\"")));
    }

    #[test]
    fn a_test_identifier_gets_its_own_service_name() {
        let n = service_name("com.cadence.player.test");
        assert_eq!(n, "Cadence (com.cadence.player.test)");
        assert_ne!(n, service_name(PRODUCTION_ID));
    }

    #[test]
    fn an_empty_or_odd_identifier_never_gets_the_production_name() {
        for id in ["", "   ", "Cadence", "com.cadence.player.", "COM.CADENCE.PLAYER", "a\nb"] {
            let n = service_name(id);
            assert_ne!(n, LEGACY_SERVICE, "{id:?}");
            assert!(!n.contains('\n'));
        }
        assert_eq!(service_name(""), "Cadence (unknown)");
    }
}
