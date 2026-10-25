//! App state shared by commands and background jobs, plus its persistence.

use std::collections::{BTreeSet, HashMap};
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
    /// Songs the user chose to skip: left out when an album, playlist, mix or shuffle plays.
    pub skipped: BTreeSet<String>,
    /// Also skip instrumental, karaoke and off-vocal versions.
    pub skip_instrumentals: bool,
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
            skipped: BTreeSet::new(),
            skip_instrumentals: false,
        }
    }
}

/// What was playing: restored, paused, when Cadence opens again.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Session {
    pub queue: Vec<String>,
    pub index: Option<usize>,
    pub position: f64,
}

/// Which songs are left out when a list plays.
pub struct SkipRule {
    ids: BTreeSet<String>,
    instrumentals: bool,
}

impl SkipRule {
    pub fn skips(&self, t: &Track) -> bool {
        self.ids.contains(&t.id) || (self.instrumentals && is_instrumental(&t.title))
    }
}

/// "Song (Instrumental)", "Song - Off Vocal", "Song (Karaoke Ver.)", "Song (Inst.)"…
pub fn is_instrumental(title: &str) -> bool {
    let t = title.to_lowercase();
    ["instrumental", "off vocal", "off-vocal", "offvocal", "karaoke", "backing track", "(inst", "[inst", "- inst", "inst.)", "inst.]"]
        .iter()
        .any(|w| t.contains(w))
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

    pub fn save_session(&self, s: &crate::audio::Status) {
        write_json(
            &self.data_dir.join("session.json"),
            &Session {
                queue: s.queue.clone(),
                index: s.index,
                position: s.position,
            },
        );
    }

    /// Puts the last session's queue back, paused where it was left (songs no longer in the
    /// library are left out).
    pub fn restore_session(&self) {
        let session: Session = read_json(&self.data_dir.join("session.json"));
        let Some(current) = session.index.and_then(|i| session.queue.get(i)).cloned() else { return };
        let items = self.queue_items(&session.queue);
        let Some(index) = items.iter().position(|q| q.id == current) else { return };
        self.engine.send(crate::audio::Cmd::Restore {
            items,
            index,
            position: session.position,
        });
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

    pub fn skip_rule(&self) -> SkipRule {
        let s = self.settings.lock();
        SkipRule {
            ids: s.skipped.clone(),
            instrumentals: s.skip_instrumentals,
        }
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

#[cfg(test)]
mod tests {
    use super::is_instrumental;

    #[test]
    fn spots_instrumental_versions() {
        for t in [
            "「拉海洛」之心 Heart of Lahai-Roi (Instrumental)",
            "A Beautiful Mistake instrumental",
            "Catch Me If You Can (Instrumental with Backing Vocals)",
            "Sakura (Off Vocal)",
            "Lemon (Karaoke Ver.)",
            "Gurenge (Inst.)",
        ] {
            assert!(is_instrumental(t), "{t}");
        }
        for t in ["Instinct", "Institute of Sound", "24k Magic", "-FINAL SIGMA-"] {
            assert!(!is_instrumental(t), "{t}");
        }
    }
}
