//! App state shared by commands and background jobs, plus its persistence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::audio::{DspSettings, Engine, QueueItem, Repeat};
use crate::library::{Snapshot, Track};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub folders: Vec<String>,
    pub volume: f32,
    pub dsp: DspSettings,
    pub repeat: Repeat,
    /// Closing the window keeps Cadence playing in the tray.
    pub close_to_tray: bool,
    /// Look up lyrics on LRCLIB when a song has none (sends artist, title and album).
    pub lyrics_online: bool,
    /// ListenBrainz account name when scrobbling is connected (the token is in Credential Manager).
    pub listenbrainz_user: String,
    /// The AI DJ (the API key is in Credential Manager).
    pub ai: crate::ai::AiSettings,
    /// Fill in missing song info (title, artist, album, cover) from the iTunes catalogue when an
    /// untagged song plays, and save it into the file.
    pub fix_tags_online: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            volume: 1.0,
            dsp: DspSettings::default(),
            repeat: Repeat::Off,
            close_to_tray: true,
            lyrics_online: true,
            listenbrainz_user: String::new(),
            ai: Default::default(),
            fix_tags_online: true,
        }
    }
}

#[derive(Default)]
pub struct Library {
    pub snapshot: Snapshot,
    pub by_id: HashMap<String, usize>,
    pub scanning: bool,
    /// Files changed while a scan was running; scan again when it finishes.
    pub rescan_pending: bool,
    pub analyzing: bool,
    /// Sound analysis (bliss) is running.
    pub listening: bool,
}

impl Library {
    pub fn set_tracks(&mut self, tracks: Vec<Track>, albums: Vec<crate::library::Album>) {
        self.by_id = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();
        self.snapshot = Snapshot { tracks, albums };
    }
}

pub struct AppState {
    pub engine: Engine,
    pub history: Mutex<crate::history::History>,
    pub playlists: Mutex<crate::playlists::Playlists>,
    pub library: Mutex<Library>,
    /// What each song sounds like, for "sounds like" radio and mixes.
    pub sonic: Mutex<crate::sonic::Store>,
    pub settings: Mutex<Settings>,
    pub data_dir: PathBuf,
    /// Watches the music folders for changes (replaced when the folder list changes).
    pub watcher: Mutex<Option<crate::jobs::FolderWatcher>>,
}

impl AppState {
    pub fn art_dir(&self) -> PathBuf {
        self.data_dir.join("art")
    }

    pub fn save_settings(&self) {
        write_json(&self.data_dir.join("settings.json"), &*self.settings.lock());
    }

    pub fn save_playlists(&self) {
        write_json(
            &self.data_dir.join("playlists.json"),
            &*self.playlists.lock(),
        );
    }

    pub fn save_history(&self) {
        write_json(&self.data_dir.join("history.json"), &*self.history.lock());
    }

    pub fn save_library(&self) {
        let tracks = self.library.lock().snapshot.tracks.clone();
        write_json(
            &self.data_dir.join("library.json"),
            &crate::library::LibraryCache { tracks },
        );
    }

    pub fn save_sonic(&self) {
        write_json(&self.data_dir.join("sonic.json"), &*self.sonic.lock());
    }

    pub fn queue_items(&self, ids: &[String]) -> Vec<QueueItem> {
        let lib = self.library.lock();
        let sonic = self.sonic.lock();
        ids.iter()
            .filter_map(|id| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]))
            .map(|t| QueueItem {
                id: t.id.clone(),
                path: PathBuf::from(&t.path),
                duration: t.duration,
                album_id: t.album_id.clone(),
                gain: t.gain,
                tone: sonic.tone(t),
            })
            .collect()
    }
}

pub fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes atomically (temp file + rename) so a crash never leaves a half-written file.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    let tmp = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_vec(value) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}
