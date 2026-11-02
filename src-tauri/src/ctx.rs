//! What the core needs from the app around it: the shared state, the voice models, and somewhere
//! to send events. The Tauri app (web UI) and the native Windows app (ffi.rs) each provide a
//! `Host`; everything else — commands, scans, analysis, voice — is the same code for both.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;
use crate::voice::Voice;

/// The front end: receives events ("library-changed", "player-status", …) as JSON.
pub trait Host: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);
    /// A folder for temporary downloads (Android update packages).
    fn cache_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub state: Arc<AppState>,
    pub voice: Arc<Voice>,
    host: Arc<dyn Host>,
}

impl Ctx {
    pub fn new(state: Arc<AppState>, voice: Arc<Voice>, host: Arc<dyn Host>) -> Self {
        Self { state, voice, host }
    }

    pub fn emit(&self, event: &str, payload: impl Serialize) {
        match serde_json::to_value(payload) {
            Ok(v) => self.host.emit(event, v),
            Err(e) => log::warn!("can't send {event}: {e}"),
        }
    }

    pub fn host(&self) -> &dyn Host {
        &*self.host
    }
}
