//! Tokens and API keys, kept in the Windows Credential Manager rather than in settings files.

const SERVICE: &str = "Cadence";

pub fn get(name: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, name)
        .ok()?
        .get_password()
        .ok()
        .filter(|s| !s.is_empty())
}

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
