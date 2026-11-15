//! App state shared by commands and background jobs, plus its persistence.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use parking_lot::Mutex;

/// Settings changed with `save_settings_later` and not saved yet.
static SETTINGS_LATER: AtomicBool = AtomicBool::new(false);
/// How long the settings must stay unchanged before a `save_settings_later` save.
const SETTINGS_QUIET: Duration = Duration::from_millis(400);
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
    /// Songs play in a random order.
    pub shuffle: bool,
    /// Closing the window keeps Cadence playing in the tray.
    pub close_to_tray: bool,
    /// Ctrl+Alt hotkeys that work from any app (desktop).
    pub global_hotkeys: bool,
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
    /// Once the voice models are downloaded: lyrics follow the singer for every song whose
    /// lyrics are opened (and the song after it in the queue).
    pub voice_auto: bool,
    /// Following the singer in all songs with lyrics was running when Cadence closed: carry on.
    pub voice_batch: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            volume: 1.0,
            dsp: DspSettings::default(),
            repeat: Repeat::Off,
            shuffle: false,
            close_to_tray: true,
            global_hotkeys: true,
            lyrics_online: true,
            listenbrainz_user: String::new(),
            ai: Default::default(),
            fix_tags_online: true,
            skipped: BTreeSet::new(),
            skip_instrumentals: false,
            voice_auto: true,
            voice_batch: false,
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
    /// Favourites and star ratings.
    pub marks: Mutex<crate::marks::Marks>,
    /// The sleep timer's minutes countdown.
    pub sleep: crate::sleep::Sleep,
    /// Library health checks (quality, integrity) and their saved results.
    pub health: crate::health::Health,
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
        // Held while writing, so two saves at once land in the order the changes were made.
        let settings = self.settings.lock();
        SETTINGS_LATER.store(false, Ordering::SeqCst);
        write_json(&self.data_dir.join("settings.json"), &*settings);
    }

    /// For changes that come many times a second (dragging the volume): the settings are saved
    /// once they've stopped changing for a moment (and at exit, `save_settings_now`).
    pub fn save_settings_later(self: &Arc<Self>) {
        static LATER: OnceLock<crossbeam_channel::Sender<Weak<AppState>>> = OnceLock::new();
        SETTINGS_LATER.store(true, Ordering::SeqCst);
        let tx = LATER.get_or_init(|| {
            let (tx, rx) = crossbeam_channel::unbounded::<Weak<AppState>>();
            let _ = std::thread::Builder::new().name("cadence-settings".into()).spawn(move || {
                while let Ok(mut state) = rx.recv() {
                    // Wait for a quiet moment: every change in between restarts the wait.
                    loop {
                        match rx.recv_timeout(SETTINGS_QUIET) {
                            Ok(newer) => state = newer,
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    if let Some(state) = state.upgrade() {
                        state.save_settings_now();
                    }
                }
            });
            tx
        });
        let _ = tx.send(Arc::downgrade(self));
    }

    /// Saves settings still waiting from `save_settings_later` (at exit).
    pub fn save_settings_now(&self) {
        if SETTINGS_LATER.load(Ordering::SeqCst) {
            self.save_settings();
        }
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

    pub fn save_marks(&self) {
        write_json(&self.data_dir.join("marks.json"), &*self.marks.lock());
    }

    pub fn save_health(&self) {
        write_json(&self.data_dir.join("health.json"), &*self.health.store.lock());
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

    /// Songs in the queue whose files went away in a rescan (moved, renamed, or deleted as a
    /// duplicate of another copy) are pointed at the copy the library has now: same title and
    /// length, the same artist and album preferred. `before`: those songs as the library had them.
    /// Returns how many were found.
    pub fn remap_queue(&self, before: &[Track]) -> usize {
        let pairs: Vec<(String, String)> = {
            let lib = self.library.lock();
            let gone: Vec<&Track> = before.iter().filter(|t| !lib.by_id.contains_key(&t.id)).collect();
            if gone.is_empty() {
                return 0;
            }
            moved_songs(&gone, &lib.snapshot.tracks)
        };
        if pairs.is_empty() {
            return 0;
        }
        let items = self.queue_items(&pairs.iter().map(|(_, new)| new.clone()).collect::<Vec<_>>());
        let replace: Vec<(String, QueueItem)> = pairs
            .into_iter()
            .filter_map(|(old, new)| items.iter().find(|q| q.id == new).map(|q| (old, q.clone())))
            .collect();
        let n = replace.len();
        self.engine.send(crate::audio::Cmd::Replace(replace));
        n
    }

    /// Playlist songs whose files went away in a rescan follow the song to its new id, as the
    /// queue does (`remap_queue`). A song that can't be found stays in the playlist under its old
    /// id: its file may come back (a drive that isn't plugged in). `before`: those songs as the
    /// library had them. Returns how many entries changed.
    pub fn remap_playlists(&self, before: &[Track]) -> usize {
        let pairs: HashMap<String, String> = {
            let lib = self.library.lock();
            let gone: Vec<&Track> = before.iter().filter(|t| !lib.by_id.contains_key(&t.id)).collect();
            if gone.is_empty() {
                return 0;
            }
            moved_songs(&gone, &lib.snapshot.tracks).into_iter().collect()
        };
        if pairs.is_empty() {
            return 0;
        }
        remember_moves(&pairs);
        let changed = self.playlists.lock().follow(&pairs);
        if changed > 0 {
            self.save_playlists();
        }
        changed
    }

    /// A playlist saved by the window, with songs that moved since it was loaded (the window may
    /// still have their old ids) pointed at where they are now.
    pub fn follow_moves(&self, p: &mut crate::playlists::Playlist) {
        let moves = MOVES.lock();
        let Some(moves) = moves.as_ref().filter(|m| !m.is_empty()) else { return };
        let lib = self.library.lock();
        for id in &mut p.track_ids {
            if lib.by_id.contains_key(id) {
                continue;
            }
            // Moved more than once: follow it to the end (a loop can't go on forever).
            let mut now = id.clone();
            for _ in 0..8 {
                match moves.get(&now) {
                    Some(next) => now = next.clone(),
                    None => break,
                }
            }
            if lib.by_id.contains_key(&now) {
                *id = now;
            }
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
                start: t.cue.map_or(0.0, |c| c.start),
                end: t.cue.and_then(|c| c.end),
            })
            .collect()
    }
}

/// For songs whose files went away (moved, renamed, replaced by a better copy): the song the
/// library has now with the same title and length (within 2.5 s), the same artist and then the
/// same album preferred. (old id, new id) for each one found.
pub(crate) fn moved_songs(gone: &[&Track], tracks: &[Track]) -> Vec<(String, String)> {
    let mut by_title: HashMap<String, Vec<&Track>> = HashMap::new();
    for t in tracks {
        by_title.entry(match_key(&t.title)).or_default().push(t);
    }
    gone.iter()
        .filter_map(|old| {
            let best = by_title
                .get(&match_key(&old.title))?
                .iter()
                .filter(|t| (t.duration - old.duration).abs() < 2.5)
                .min_by_key(|t| {
                    (
                        match_key(&t.artist) != match_key(&old.artist),
                        match_key(&t.album) != match_key(&old.album),
                        ((t.duration - old.duration).abs() * 1000.0) as u64,
                    )
                })?;
            Some((old.id.clone(), best.id.clone()))
        })
        .collect()
}

/// Songs that moved while Cadence runs (old id → new id), for playlists the window saves with
/// ids it loaded before the move.
static MOVES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn remember_moves(pairs: &HashMap<String, String>) {
    let mut moves = MOVES.lock();
    let moves = moves.get_or_insert_with(HashMap::new);
    for (old, new) in pairs {
        if old != new {
            moves.insert(old.clone(), new.clone());
        }
    }
}

/// The copy of a saved file as it was before its last save (`settings.json.bak`).
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// Where an unreadable file is kept aside: `history.broken.json`, or `history.broken-2.json` and
/// so on when an earlier one is still there (a second accident never overwrites the first).
fn aside_path(path: &Path) -> PathBuf {
    let first = path.with_extension("broken.json");
    if !first.exists() {
        return first;
    }
    (2..1000)
        .map(|n| path.with_extension(format!("broken-{n}.json")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.with_extension(format!("broken-{}.json", crate::history::now())))
}

/// Reads a saved file. When it's missing or can't be read, the copy from before its last save
/// (`.bak`) is used instead; an unreadable file is kept aside (`*.broken.json`) rather than
/// overwritten by the next save, so nothing is lost for good. Neither there: the default.
pub fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(v) => return v,
            Err(e) => {
                let aside = aside_path(path);
                match std::fs::rename(path, &aside) {
                    Ok(()) => log::error!("can't read {}: {e}; kept as {}", path.display(), aside.display()),
                    Err(re) => log::error!("can't read {}: {e}; and can't keep it aside: {re}", path.display()),
                }
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // Locked or not allowed: the backup rather than nothing, so the next save can't write
        // an empty file over the real one.
        Err(e) => log::error!("can't read {}: {e}", path.display()),
    }
    let bak = backup_path(path);
    let Ok(bytes) = std::fs::read(&bak) else {
        return T::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => {
            log::warn!("{} is missing or unreadable; using its backup {}", path.display(), bak.display());
            v
        }
        Err(e) => {
            log::error!("can't read the backup {} either: {e}", bak.display());
            T::default()
        }
    }
}

/// Saves `value` as JSON with `write_file` (keeping the previous save as `.bak`); a failure is logged.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    match serde_json::to_vec(value) {
        Ok(bytes) => {
            if let Err(e) = write_file(path, &bytes, true) {
                log::error!("can't save {}: {e}", path.display());
            }
        }
        Err(e) => log::error!("can't save {}: {e}", path.display()),
    }
}

/// The last step of every save (keeping the old copy, putting the new one in place), one at a
/// time: two threads saving the same file can't mix up its backup.
static REPLACING: Mutex<()> = Mutex::new(());

/// Writes a file so a crash or power cut never leaves it half-written: into a new temporary file
/// beside it (its own name, so two saves at once can't mix), synced to the disk, then put in
/// place of the old one in one step. With `backup`, the old one stays as `<name>.bak`.
pub fn write_file(path: &Path, bytes: &[u8], backup: bool) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let prefix = format!(".{}.", path.file_name().unwrap_or_default().to_string_lossy());
    let mut tmp = tempfile::Builder::new().prefix(&prefix).suffix(".tmp").tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    let _one = REPLACING.lock();
    if backup && path.exists() {
        keep_previous(path);
    }
    // Windows refuses a replace while another program (a virus scanner, say) has the old file
    // open for a moment: try a few times.
    let mut attempt = 0;
    loop {
        match tmp.persist(path) {
            Ok(_) => break,
            Err(e) if attempt < 4 && e.error.kind() == std::io::ErrorKind::PermissionDenied => {
                tmp = e.file;
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(50 << attempt));
            }
            Err(e) => return Err(e.error),
        }
    }
    sync_dir(dir);
    Ok(())
}

/// Keeps the file as it is now as `<name>.bak`: a second name for the same data (no copying),
/// or a copy where the disk can't do that.
fn keep_previous(path: &Path) {
    let bak = backup_path(path);
    let _ = std::fs::remove_file(&bak);
    if std::fs::hard_link(path, &bak).is_err()
        && let Err(e) = std::fs::copy(path, &bak) {
            log::warn!("can't keep a backup of {}: {e}", path.display());
        }
}

/// The new name must reach the disk too (Linux, Android, macOS); Windows does that with the file.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::{backup_path, is_instrumental, read_json, write_json};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cadence-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keeps_an_unreadable_file_aside() {
        let dir = temp_dir("read-json");
        let path = dir.join("history.json");
        std::fs::write(&path, b"{\"plays\": [nul").unwrap();
        let read: std::collections::HashMap<String, u32> = read_json(&path);
        assert!(read.is_empty());
        assert!(!path.exists());
        assert_eq!(std::fs::read(dir.join("history.broken.json")).unwrap(), b"{\"plays\": [nul");
        // A second accident is kept too, without overwriting the first.
        std::fs::write(&path, b"[[[").unwrap();
        let _: std::collections::HashMap<String, u32> = read_json(&path);
        assert_eq!(std::fs::read(dir.join("history.broken.json")).unwrap(), b"{\"plays\": [nul");
        assert_eq!(std::fs::read(dir.join("history.broken-2.json")).unwrap(), b"[[[");
        // A file that isn't there yet is simply empty.
        let none: Vec<u32> = read_json(&dir.join("missing.json"));
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_keep_the_previous_copy_and_reads_fall_back_to_it() {
        let dir = temp_dir("write-json");
        let path = dir.join("marks.json");
        write_json(&path, &vec![1u32]);
        assert!(!backup_path(&path).exists(), "nothing to back up on the first save");
        write_json(&path, &vec![1u32, 2]);
        assert_eq!(read_json::<Vec<u32>>(&path), vec![1, 2]);
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), b"[1]");
        // No temporary files left behind.
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        // The file is damaged: set aside, and the copy before it is used.
        std::fs::write(&path, b"[1, 2, ").unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path), vec![1]);
        assert!(dir.join("marks.broken.json").exists());
        // The file is gone: the backup again.
        assert!(!path.exists());
        assert_eq!(read_json::<Vec<u32>>(&path), vec![1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_at_once_from_many_threads_leave_a_whole_file() {
        let dir = temp_dir("write-race");
        let path = dir.join("session.json");
        std::thread::scope(|s| {
            for n in 0..8u32 {
                let path = &path;
                s.spawn(move || {
                    for i in 0..20u32 {
                        write_json(path, &vec![n; 200 + i as usize]);
                    }
                });
            }
        });
        let v: Vec<u32> = serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("a whole file");
        assert!(v.len() >= 200 && v.iter().all(|&x| x == v[0]));
        let bak: Vec<u32> = serde_json::from_slice(&std::fs::read(backup_path(&path)).unwrap()).expect("a whole backup");
        assert!(bak.iter().all(|&x| x == bak[0]));
        let _ = std::fs::remove_dir_all(&dir);
    }

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

/// A name reduced to its letters and digits, lower-case: "コノヨ Loading..." and "コノヨLoading…"
/// match, as do "REOL" and "Reol".
pub(crate) fn match_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod match_tests {
    use super::{match_key, moved_songs};
    use crate::library::Track;

    fn song(id: &str, title: &str, artist: &str, album: &str, duration: f64) -> Track {
        Track { id: id.into(), title: title.into(), artist: artist.into(), album: album.into(), duration, ..Default::default() }
    }

    #[test]
    fn a_replaced_file_is_found_again() {
        // The lossy copy was replaced by a FLAC; a cover by someone else, and a missing song.
        let gone = [song("m4a", "Lemon", "Kenshi Yonezu", "Lemon", 255.0), song("lost", "Unknown", "X", "Y", 100.0)];
        let now = vec![
            song("cover", "Lemon", "Someone", "Covers", 255.1),
            song("flac", "LEMON", "Kenshi Yonezu", "Lemon", 256.0),
            song("long", "Lemon", "Kenshi Yonezu", "Lemon", 260.0),
        ];
        let gone: Vec<&Track> = gone.iter().collect();
        assert_eq!(moved_songs(&gone, &now), [("m4a".to_string(), "flac".to_string())]);
    }

    #[test]
    fn names_match_across_spacing_case_and_punctuation() {
        assert_eq!(match_key("コノヨ Loading..."), match_key("コノヨLoading…"));
        assert_eq!(match_key("REOL"), match_key("Reol"));
        assert_ne!(match_key("RE"), match_key("REOL"));
    }
}
