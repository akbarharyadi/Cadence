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
/// The longest a `save_settings_later` save waits while the changes keep coming.
const SETTINGS_LONGEST: Duration = Duration::from_secs(3);
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
    /// Which shuffle: balanced (artists spread out, recently played songs later) or fully random.
    pub shuffle_mode: crate::shuffle::ShuffleMode,
    /// When the queue ends (and nothing repeats): keep playing songs like the last ones.
    pub autoplay: bool,
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
    /// Ask MusicBrainz for the other names of the library's artists (sends artist names only).
    pub artist_names_online: bool,
    /// Send hearts and un-hearts to ListenBrainz as "loved" feedback (when it's connected).
    pub listenbrainz_loved: bool,
    /// Once a day, when Discover opens, ask ListenBrainz for new releases (nothing about you is
    /// sent) and show those by artists in the library.
    pub new_releases_online: bool,
    /// Songs the user chose to skip: left out when an album, playlist, mix or shuffle plays.
    pub skipped: BTreeSet<String>,
    /// Also skip instrumental, karaoke and off-vocal versions.
    pub skip_instrumentals: bool,
    /// Once the voice models are downloaded: lyrics follow the singer for every song whose
    /// lyrics are opened (and the song after it in the queue).
    pub voice_auto: bool,
    /// Following the singer in all songs with lyrics was running when Cadence closed: carry on.
    pub voice_batch: bool,
    /// The output device chosen on the Sound page (None: the system default, followed).
    pub output_device: Option<crate::audio::DeviceRef>,
    /// Remember the equalizer, crossfeed, Direct output and volume per output device.
    pub device_profiles: bool,
    /// Those, per device (`profiles`).
    pub sound_profiles: crate::profiles::Profiles,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            volume: 1.0,
            dsp: DspSettings::default(),
            repeat: Repeat::Off,
            shuffle: false,
            shuffle_mode: Default::default(),
            autoplay: false,
            close_to_tray: true,
            global_hotkeys: true,
            lyrics_online: true,
            listenbrainz_user: String::new(),
            ai: Default::default(),
            fix_tags_online: true,
            artist_names_online: true,
            listenbrainz_loved: true,
            new_releases_online: true,
            skipped: BTreeSet::new(),
            skip_instrumentals: false,
            voice_auto: true,
            voice_batch: false,
            output_device: None,
            device_profiles: true,
            sound_profiles: Default::default(),
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
    /// While shuffled: the order to put back when shuffle is turned off, as places in `queue`
    /// (`shuffle::positions`: a few digits per song, not its id again). Empty: not shuffled.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unshuffled: Vec<u32>,
    /// Where the songs Autoplay added begin in `queue`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoplay_from: Option<usize>,
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
        // The artists are built again from the new library (in the background, `identity`).
        crate::identity::invalidate();
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

    /// For changes that can come many times a second (dragging the volume or an EQ band, clicking
    /// a switch): the settings are saved on a thread of their own once they've stopped changing
    /// for a moment (at most a few seconds later, and at exit: `save_settings_now`), never on the
    /// caller's thread.
    pub fn save_settings_later(self: &Arc<Self>) {
        static LATER: OnceLock<crossbeam_channel::Sender<Weak<AppState>>> = OnceLock::new();
        SETTINGS_LATER.store(true, Ordering::SeqCst);
        let tx = LATER.get_or_init(|| {
            let (tx, rx) = crossbeam_channel::unbounded::<Weak<AppState>>();
            let _ = std::thread::Builder::new().name("cadence-settings".into()).spawn(move || {
                while let Ok(first) = rx.recv() {
                    let Some(state) = settle(&rx, first, SETTINGS_QUIET, SETTINGS_LONGEST) else { return };
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
                unshuffled: if s.shuffle { crate::shuffle::positions(&s.queue, s.index, &self.engine.unshuffled()) } else { Vec::new() },
                autoplay_from: s.autoplay_from,
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
        // Places in the saved queue → places among the songs still in the library.
        let autoplay_from = session.autoplay_from.map(|from| {
            let lib = self.library.lock();
            session.queue.iter().take(from).filter(|id| lib.by_id.contains_key(*id)).count()
        });
        self.engine.send(crate::audio::Cmd::Restore {
            items,
            index,
            position: session.position,
            unshuffled: crate::shuffle::from_positions(&session.queue, &session.unshuffled),
            autoplay_from,
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

    /// The library cache (megabytes, saved every half minute while analysis runs) has no `.bak`:
    /// a rescan builds it again, so there's no need to keep (or, where the disk can't link files,
    /// copy) the previous one on every save.
    pub fn save_library(&self) {
        static OLD_BACKUP: std::sync::Once = std::sync::Once::new();
        let path = self.data_dir.join("library.json");
        OLD_BACKUP.call_once(|| {
            let _ = std::fs::remove_file(backup_path(&path));
        });
        let tracks = self.library.lock().snapshot.tracks.clone();
        write_json_with(&path, &crate::library::LibraryCache { tracks }, false);
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

    /// After a rescan: songs of the queue, the playlists and the favourites and ratings whose files
    /// went away (moved, renamed, replaced by a FLAC, deleted as a duplicate of another copy)
    /// follow the song to the copy the library has now, when it's certain which one that is
    /// (`Finder`). A song that can't be found for sure keeps its old id: its file may come back
    /// (a drive that isn't plugged in), and a wrong guess would put another song in its place.
    /// `queued`, `listed`: the queue's and the playlists' songs as the library had them.
    pub fn follow_moved(&self, queued: &[Track], listed: &[Track]) -> Followed {
        let folders = self.settings.lock().folders.clone();
        let mut out = Followed::default();
        let (queue_pairs, list_pairs) = {
            let lib = self.library.lock();
            let mut marks = self.marks.lock();
            let lookup = |id: &str| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]);
            out.marks_noted = marks.note_paths(lookup);
            let gone = |before: &[Track]| -> Vec<Track> { before.iter().filter(|t| !lib.by_id.contains_key(&t.id)).cloned().collect() };
            let (gone_queued, gone_listed) = (gone(queued), gone(listed));
            let marks_gone = marks.has_gone(|id| lib.by_id.contains_key(id));
            if gone_queued.is_empty() && gone_listed.is_empty() && !marks_gone {
                return out;
            }
            let finder = Finder::new(&lib.snapshot.tracks, &folders, crate::libfix::artist_aliases(&self.data_dir));
            let refs = |v: &[Track]| -> Vec<(String, String)> { moved_songs(&v.iter().collect::<Vec<_>>(), &finder) };
            let pairs = (refs(&gone_queued), refs(&gone_listed));
            if marks_gone {
                out.marks = marks.relink(&finder);
            }
            pairs
        };
        remember_moves(queue_pairs.iter().chain(&list_pairs).map(|(a, b)| (a.as_str(), b.as_str())));
        if !queue_pairs.is_empty() {
            let items = self.queue_items(&queue_pairs.iter().map(|(_, new)| new.clone()).collect::<Vec<_>>());
            let replace: Vec<(String, QueueItem)> = queue_pairs
                .into_iter()
                .filter_map(|(old, new)| items.iter().find(|q| q.id == new).map(|q| (old, q.clone())))
                .collect();
            out.queue = replace.len();
            self.engine.send(crate::audio::Cmd::Replace(replace));
        }
        if !list_pairs.is_empty() {
            out.playlists = self.playlists.lock().follow(&list_pairs.into_iter().collect());
            if out.playlists > 0 {
                self.save_playlists();
            }
        }
        if out.marks > 0 || out.marks_noted > 0 {
            self.save_marks();
        }
        out
    }

    /// A playlist saved by the window, with songs that moved since it was loaded (the window may
    /// still have their old ids) pointed at where they are now.
    pub fn follow_moves(&self, p: &mut crate::playlists::Playlist) {
        // The library first, then the moves: the order `follow_moved` (`Finder::known`) takes them.
        let lib = self.library.lock();
        let moves = MOVES.lock();
        let Some(moves) = moves.as_ref().filter(|m| !m.is_empty()) else { return };
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
        // For the balanced shuffle: when each song last played (one pass over the recent history,
        // its lock let go before the library's) and who it's by.
        let heard = crate::shuffle::recent_plays(&self.history.lock());
        let aliases = crate::shuffle::aliases(&self.data_dir);
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
                artist: crate::shuffle::artist_key(&t.artist, &aliases),
                last_played: heard.get(&t.id).copied().unwrap_or(0),
                auto: false,
            })
            .collect()
    }
}

/// Waits for a quiet moment after `first`: every message within `quiet` of the one before restarts
/// the wait, but it never goes on longer than `longest` (a slider held for long is still saved
/// every few seconds). The latest message; None when the senders are gone.
fn settle<T>(rx: &crossbeam_channel::Receiver<T>, first: T, quiet: Duration, longest: Duration) -> Option<T> {
    let started = std::time::Instant::now();
    let mut latest = first;
    loop {
        let wait = quiet.min(longest.saturating_sub(started.elapsed()));
        if wait.is_zero() {
            return Some(latest);
        }
        match rx.recv_timeout(wait) {
            Ok(newer) => latest = newer,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => return Some(latest),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// What `follow_moved` did.
#[derive(Default, Debug)]
pub struct Followed {
    /// Queue entries pointed at their song's new id.
    pub queue: usize,
    /// Playlist entries pointed at their song's new id.
    pub playlists: usize,
    /// Favourites and ratings that followed their song.
    pub marks: usize,
    /// Favourites and ratings that had their file noted (made before marks kept it).
    pub marks_noted: usize,
}

/// Two copies of a song this much apart in length can be the same recording (another encode).
const SAME_LENGTH: f64 = 2.5;

/// Which artist names are one artist (in the other script: 米津玄師 ↔ Kenshi Yonezu): the shared
/// `identity` snapshot.
pub(crate) type Aliases = crate::identity::Identity;

/// What a song whose file went away was: enough to know it again.
pub(crate) struct Was<'a> {
    pub id: &'a str,
    /// Its file (empty: not known).
    pub path: &'a str,
    /// A song of a single-file album (its file holds the whole album).
    pub cue: bool,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f64,
}

impl<'a> Was<'a> {
    pub fn of(t: &'a Track) -> Self {
        Was { id: &t.id, path: &t.path, cue: t.cue.is_some(), title: &t.title, artist: &t.artist, album: &t.album, duration: t.duration }
    }
}

/// How sure `Finder::find` is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Found {
    /// Cadence made the move itself (a copy it removed → the copy that stayed), or the file was
    /// replaced in its folder by one of another format ("Song.m4a" → "Song.flac").
    Exact,
    /// The only song in the library that is clearly the same one (`successor`).
    Same,
}

/// Finds songs whose files went away in the library as it is now. What Cadence knows for sure
/// comes first, then the one song that's clearly the same (same title, artist and length; its
/// album when there are several). Built once per rescan.
pub(crate) struct Finder<'a> {
    by_id: HashMap<&'a str, &'a Track>,
    by_path: HashMap<&'a str, &'a Track>,
    by_title: HashMap<String, Vec<&'a Track>>,
    aliases: Aliases,
    folders: Vec<PathBuf>,
}

impl<'a> Finder<'a> {
    pub fn new(tracks: &'a [Track], folders: &[String], aliases: Aliases) -> Self {
        let mut by_title: HashMap<String, Vec<&Track>> = HashMap::new();
        for t in tracks {
            by_title.entry(match_key(&t.title)).or_default().push(t);
        }
        Finder {
            by_id: tracks.iter().map(|t| (t.id.as_str(), t)).collect(),
            by_path: tracks.iter().filter(|t| t.cue.is_none()).map(|t| (t.path.as_str(), t)).collect(),
            by_title,
            aliases,
            folders: folders.iter().map(PathBuf::from).collect(),
        }
    }

    pub fn track(&self, id: &str) -> Option<&'a Track> {
        self.by_id.get(id).copied()
    }

    /// Where the song is now, and how sure that is; None when it isn't certain.
    pub fn find(&self, was: &Was) -> Option<(&'a Track, Found)> {
        if let Some(t) = self.known(was.id) {
            return Some((t, Found::Exact));
        }
        if let Some(t) = self.replaced_in_place(was)
            && really_gone(Path::new(was.path), &self.folders) {
                return Some((t, Found::Exact));
            }
        let t = successor(was, &self.by_title, &self.aliases)?;
        // A file that's only out of reach (an unplugged drive, a share that's down) isn't followed
        // to a look-alike elsewhere.
        (was.path.is_empty() || really_gone(Path::new(was.path), &self.folders)).then_some((t, Found::Same))
    }

    /// A move Cadence made or followed before (`MOVES`), to the song's latest id.
    fn known(&self, id: &str) -> Option<&'a Track> {
        let moves = MOVES.lock();
        let moves = moves.as_ref()?;
        let mut now = moves.get(id)?;
        // Moved more than once: followed to the end (a loop can't go on forever).
        for _ in 0..8 {
            match moves.get(now) {
                Some(next) => now = next,
                None => break,
            }
        }
        self.track(now).filter(|t| t.id != id)
    }

    /// The same file name in the same folder with another format: the downloader replaced a lossy
    /// copy with a FLAC ("Song.m4a" → "Song.flac"), whatever the new copy's tags say.
    fn replaced_in_place(&self, was: &Was) -> Option<&'a Track> {
        if was.cue || was.path.is_empty() {
            return None;
        }
        let old = Path::new(was.path);
        let ext = old.extension()?.to_string_lossy().to_ascii_lowercase();
        crate::library::AUDIO_EXTENSIONS
            .iter()
            .filter(|e| **e != ext)
            .flat_map(|e| [e.to_string(), e.to_ascii_uppercase()])
            .find_map(|e| self.by_path.get(old.with_extension(e).to_string_lossy().as_ref()).copied())
            .filter(|t| t.id != was.id && (t.duration - was.duration).abs() < SAME_LENGTH)
    }
}

/// The one song in the library that is clearly `was`: the same title, the same artist (across
/// scripts and "feat." credits, `same_artist`) and the same length within 2.5 s. With several,
/// the one on its album, then the closest in length; None when two are equally good: on other
/// albums only (another album's "Intro"), or as close as each other on its album.
pub(crate) fn successor<'a>(was: &Was, by_title: &HashMap<String, Vec<&'a Track>>, aliases: &Aliases) -> Option<&'a Track> {
    let candidates: Vec<&Track> = by_title
        .get(&match_key(was.title))?
        .iter()
        .copied()
        .filter(|t| t.id != was.id && (t.duration - was.duration).abs() < SAME_LENGTH && same_artist(was.artist, &t.artist, aliases))
        .collect();
    let album = match_key(was.album);
    let mut on_album: Vec<&Track> = candidates.iter().copied().filter(|t| match_key(&t.album) == album).collect();
    if on_album.is_empty() {
        // Only on other albums: one is the song (a FLAC of another edition); with two, either
        // could be, and a length within 2.5 s doesn't tell which.
        return match candidates.as_slice() {
            [one] => Some(one),
            _ => None,
        };
    }
    let gap = |t: &Track| (t.duration - was.duration).abs();
    on_album.sort_by(|a, b| gap(a).total_cmp(&gap(b)));
    match on_album.as_slice() {
        [one] => Some(one),
        [a, b, ..] if gap(b) - gap(a) >= 0.5 => Some(a),
        _ => None,
    }
}

/// Whether two artist credits name the same artist: a name in common ("Kenshi Yonezu" and
/// "Kenshi Yonezu feat. Hatsune Miku"), also across scripts — one artist in `identity`
/// (米津玄師 ↔ Kenshi Yonezu), or a kana name's reading in Latin letters (りりあ。 ↔ Riria.).
pub(crate) fn same_artist(a: &str, b: &str, aliases: &Aliases) -> bool {
    match_key(a) == match_key(b) || aliases.same_artist(a, b)
}

/// Whether a song whose file is no longer in the library really went away (rather than being out
/// of reach for now): its file isn't there, its music folder can be read, and no folder on the way
/// is a link to somewhere that can't be reached. A file that's still there, or that can't be
/// checked (no permission, a drive that isn't plugged in), didn't go anywhere.
pub(crate) fn really_gone(path: &Path, folders: &[PathBuf]) -> bool {
    match std::fs::metadata(path) {
        Ok(_) => return false,
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return false,
        Err(_) => {}
    }
    let root = folders
        .iter()
        .filter(|f| path.starts_with(f))
        .max_by_key(|f| f.as_os_str().len())
        .map(PathBuf::as_path)
        .or_else(|| path.ancestors().last());
    let Some(root) = root.filter(|r| !r.as_os_str().is_empty()) else { return false };
    if std::fs::read_dir(root).is_err() {
        return false;
    }
    for folder in path.ancestors().skip(1) {
        if folder == root || !folder.starts_with(root) {
            break;
        }
        match std::fs::metadata(folder) {
            Ok(_) => break,
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return false,
            // Not there: a folder that was moved or deleted, unless it's a link whose target is gone.
            Err(_) if std::fs::symlink_metadata(folder).is_ok() => return false,
            Err(_) => {}
        }
    }
    true
}

/// For songs whose files went away: (old id, new id) for each one found for sure (`Finder`).
/// Two songs of different albums can't both be one song: a guess that points at the same copy as
/// another song of another album is not followed.
pub(crate) fn moved_songs(gone: &[&Track], finder: &Finder) -> Vec<(String, String)> {
    let found: Vec<(&Track, &Track, Found)> = gone
        .iter()
        .filter_map(|old| finder.find(&Was::of(old)).map(|(new, how)| (*old, new, how)))
        .collect();
    found
        .iter()
        .filter(|(old, new, how)| {
            *how == Found::Exact
                || !found.iter().any(|(o, n, _)| n.id == new.id && o.id != old.id && match_key(&o.album) != match_key(&old.album))
        })
        .map(|(old, new, _)| (old.id.clone(), new.id.clone()))
        .collect()
}

/// Songs that moved while Cadence runs (old id → new id): moves Cadence made itself (a copy it
/// removed → the copy that stayed) and ones it followed, for playlists the window saves with ids
/// it loaded before the move, and for the rescan after a removal.
static MOVES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub(crate) fn remember_moves<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) {
    let mut moves = MOVES.lock();
    let moves = moves.get_or_insert_with(HashMap::new);
    for (old, new) in pairs {
        if old != new {
            moves.insert(old.to_string(), new.to_string());
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

/// Files that couldn't be read at startup although they're there (locked by another program,
/// not allowed), with no backup to use instead: what Cadence has of them this session is empty,
/// and saving it would replace the real file. They aren't saved until Cadence starts again.
static DONT_SAVE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Whether saves of `path` are held back this session (see `DONT_SAVE`).
pub fn held_back(path: &Path) -> bool {
    DONT_SAVE.lock().iter().any(|p| p == path)
}

/// Why a saved file couldn't be read.
enum Unread {
    /// It isn't there (never saved yet, or deleted).
    Missing,
    /// It's there but can't be opened or read: locked by another program, not allowed, a disk error.
    Locked(std::io::Error),
}

/// A file's bytes. Windows refuses to open a file another program (a virus scanner, a backup or
/// sync tool) holds open for a moment: tried a few times before giving up.
fn read_bytes(path: &Path) -> Result<Vec<u8>, Unread> {
    let mut attempt = 0;
    loop {
        match std::fs::read(path) {
            Ok(b) => return Ok(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Unread::Missing),
            Err(_) if attempt < 3 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50 << attempt));
            }
            Err(e) => return Err(Unread::Locked(e)),
        }
    }
}

/// Reads a saved file. The three ways that can fail are told apart:
/// - missing: the copy from before its last save (`.bak`) if there is one, else the default;
/// - there but unparseable (damaged): kept aside (`*.broken.json`, so nothing is lost for good),
///   then the `.bak`, else the default;
/// - there but unreadable (locked, not allowed): the `.bak`; with none that can be read, the
///   default — and the file isn't saved this session (`DONT_SAVE`), so an empty default can't be
///   written over the real file.
pub fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    let locked = match read_bytes(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(v) => return v,
            Err(e) => {
                let aside = aside_path(path);
                match std::fs::rename(path, &aside) {
                    Ok(()) => log::error!("can't read {}: {e}; kept as {}", path.display(), aside.display()),
                    Err(re) => log::error!("can't read {}: {e}; and can't keep it aside: {re}", path.display()),
                }
                false
            }
        },
        Err(Unread::Missing) => false,
        Err(Unread::Locked(e)) => {
            log::error!("can't open {}: {e}", path.display());
            true
        }
    };
    let bak = backup_path(path);
    let from_backup = match read_bytes(&bak) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!("can't read the backup {} either: {e}", bak.display());
                None
            }
        },
        Err(Unread::Missing) => None,
        Err(Unread::Locked(e)) => {
            log::error!("can't open the backup {} either: {e}", bak.display());
            None
        }
    };
    match from_backup {
        Some(v) => {
            log::warn!("{} is missing or unreadable; using its backup {}", path.display(), bak.display());
            v
        }
        None => {
            if locked {
                log::error!(
                    "{} is there but can't be read, and has no backup: NOT saving it this session (so it isn't replaced by an empty one); close what holds it and restart Cadence",
                    path.display()
                );
                DONT_SAVE.lock().push(path.to_path_buf());
            }
            T::default()
        }
    }
}

/// Saves `value` as JSON with `write_file`, keeping the previous save as `.bak`; a failure is logged.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    write_json_with(path, value, true);
}

/// `write_json`, with or without a `.bak` of the previous save.
pub fn write_json_with<T: Serialize>(path: &Path, value: &T, backup: bool) {
    match serde_json::to_vec(value) {
        Ok(bytes) => {
            if let Err(e) = write_file(path, &bytes, backup) {
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
/// beside it (`.<name>.XXXXXX.tmp`, its own name so two saves at once can't mix; left-overs of a
/// crash are cleared at startup by `clear_temp_files`), synced to the disk, then put in place of
/// the old one in one step. With `backup`, the old one stays as `<name>.bak`. A file held back
/// this session (`held_back`) isn't written.
pub fn write_file(path: &Path, bytes: &[u8], backup: bool) -> std::io::Result<()> {
    use std::io::Write;
    if held_back(path) {
        log::warn!("not saving {}: it couldn't be read when Cadence started", path.display());
        return Ok(());
    }
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

/// Set once making a second name for a file has failed (some phone storage can't): from then on
/// backups are copies.
static NO_LINKS: AtomicBool = AtomicBool::new(false);
/// A file at least this big whose backup is a copy (`NO_LINKS`)…
const BIG_FILE: u64 = 1 << 20;
/// …has its backup copied again at most this often, not on every save.
const BIG_BACKUP_EVERY: Duration = Duration::from_secs(10 * 60);

/// Keeps the file as it is now as `<name>.bak`: a second name for the same data (no copying),
/// or a copy where the disk can't do that — for a big file only every few minutes. Never fails
/// the save: without a backup the save still goes on (logged).
fn keep_previous(path: &Path) {
    let bak = backup_path(path);
    if NO_LINKS.load(Ordering::Relaxed) && !backup_due(path, &bak) {
        return;
    }
    let _ = std::fs::remove_file(&bak);
    if !NO_LINKS.load(Ordering::Relaxed) {
        match std::fs::hard_link(path, &bak) {
            Ok(()) => return,
            Err(e) => {
                log::info!("can't link {} as its backup ({e}); backups are copies from now on", path.display());
                NO_LINKS.store(true, Ordering::Relaxed);
            }
        }
    }
    if let Err(e) = std::fs::copy(path, &bak) {
        log::warn!("can't keep a backup of {}: {e}", path.display());
    }
}

/// Whether a backup copy is due: a small file's every time; a big one's once its backup is a few
/// minutes old (or there's none).
fn backup_due(path: &Path, bak: &Path) -> bool {
    let big = std::fs::metadata(path).is_ok_and(|m| m.len() >= BIG_FILE);
    let age = std::fs::metadata(bak).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    !big || age.is_none_or(|a| a >= BIG_BACKUP_EVERY)
}

/// Left-overs of saves that never finished (Cadence was killed or the power went between writing
/// the temporary file and putting it in place): `.<name>.XXXXXX.tmp` files (`write_file`'s own
/// naming) in the data folder and the folders in it, older than a few minutes (a save running now
/// is left alone). The album art folder, which `write_file` doesn't write, isn't looked through.
/// Returns how many were removed.
pub fn clear_temp_files(data_dir: &Path) -> usize {
    const STALE: Duration = Duration::from_secs(5 * 60);
    let mut removed = 0;
    let entries = walkdir::WalkDir::new(data_dir)
        .max_depth(2)
        .into_iter()
        // (Nor the waveforms' folder: tens of thousands of files `write_file` doesn't write either.)
        .filter_entry(|e| e.depth() != 1 || !e.file_type().is_dir() || (e.file_name() != "art" && e.file_name() != crate::waveform::DIR))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file());
    for e in entries {
        if !is_save_leftover(&e.file_name().to_string_lossy()) {
            continue;
        }
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|a| a >= STALE);
        if old && std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("removed {removed} files left by saves that didn't finish");
    }
    removed
}

/// `.settings.json.a1B2c3.tmp`: a temporary file of `write_file` (tempfile's 6 random letters
/// and digits between the name and ".tmp").
fn is_save_leftover(name: &str) -> bool {
    let Some(inner) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else { return false };
    let Some((file, random)) = inner.rsplit_once('.') else { return false };
    !file.is_empty() && random.len() == 6 && random.chars().all(|c| c.is_ascii_alphanumeric())
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
    use super::{BIG_BACKUP_EVERY, BIG_FILE, backup_due, backup_path, clear_temp_files, held_back, is_instrumental, is_save_leftover, read_json, settle, write_json};

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

    /// Holds `path` so nothing else can open it (as a virus scanner or sync tool can), or on Unix
    /// takes away the right to read it; None where that can't be done (running as root).
    fn lock(path: &std::path::Path) -> Option<Box<dyn std::any::Any>> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let f = std::fs::OpenOptions::new().read(true).share_mode(0).open(path).unwrap();
            Some(Box::new(f))
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
            if std::fs::read(path).is_ok() {
                return None;
            }
            Some(Box::new(()))
        }
    }

    #[test]
    fn a_locked_file_without_a_backup_is_never_overwritten() {
        let dir = temp_dir("locked");
        let path = dir.join("playlists.json");
        std::fs::write(&path, b"[1,2,3]").unwrap();
        let Some(held) = lock(&path) else { return };
        // There but unreadable, no backup: the default, and saves of it are held back.
        let read: Vec<u32> = read_json(&path);
        assert!(read.is_empty());
        assert!(held_back(&path));
        drop(held);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_json(&path, &Vec::<u32>::new());
        assert_eq!(std::fs::read(&path).unwrap(), b"[1,2,3]", "the real file is untouched");
        assert!(!dir.join("playlists.broken.json").exists(), "a locked file isn't a damaged one");
        // Missing and damaged files aren't held back: they're saved as usual.
        let missing = dir.join("history.json");
        let _: Vec<u32> = read_json(&missing);
        assert!(!held_back(&missing));
        write_json(&missing, &vec![7u32]);
        assert_eq!(std::fs::read(&missing).unwrap(), b"[7]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_locked_file_with_a_backup_uses_it() {
        let dir = temp_dir("locked-bak");
        let path = dir.join("marks.json");
        write_json(&path, &vec![1u32]);
        write_json(&path, &vec![1u32, 2]);
        let Some(held) = lock(&path) else { return };
        assert_eq!(read_json::<Vec<u32>>(&path), vec![1]);
        assert!(!held_back(&path), "the backup was read: saving goes on");
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn left_overs_of_interrupted_saves_are_cleared() {
        let dir = temp_dir("leftovers");
        std::fs::create_dir_all(dir.join("changes")).unwrap();
        std::fs::create_dir_all(dir.join("art")).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let make = |p: std::path::PathBuf, old_one: bool| {
            std::fs::write(&p, b"x").unwrap();
            if old_one {
                std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
            }
            p
        };
        let stale = make(dir.join(".settings.json.a1B2c3.tmp"), true);
        let nested = make(dir.join("changes").join(".changes.json.Zz9xY0.tmp"), true);
        let fresh = make(dir.join(".history.json.q1w2e3.tmp"), false);
        let other = make(dir.join(".something.tmp"), true);
        let mine = make(dir.join("notes.json.a1B2c3.tmp"), true);
        let art = make(dir.join("art").join(".a1.jpg.a1B2c3.tmp"), true);
        assert_eq!(clear_temp_files(&dir), 2);
        assert!(!stale.exists() && !nested.exists());
        assert!(fresh.exists(), "a save running now is left alone");
        assert!(other.exists() && mine.exists(), "only the save's own naming");
        assert!(art.exists(), "the art folder isn't looked through");
        assert!(is_save_leftover(".library.json.AbC123.tmp"));
        assert!(!is_save_leftover(".library.json.tmp") && !is_save_leftover(".x.toolong1.tmp"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_big_files_copied_backup_is_refreshed_only_now_and_then() {
        let dir = temp_dir("big-bak");
        let (small, big) = (dir.join("marks.json"), dir.join("sonic.json"));
        std::fs::write(&small, b"[1]").unwrap();
        std::fs::write(&big, vec![b' '; BIG_FILE as usize]).unwrap();
        // No backup yet: due.
        assert!(backup_due(&small, &backup_path(&small)) && backup_due(&big, &backup_path(&big)));
        std::fs::write(backup_path(&small), b"[0]").unwrap();
        std::fs::write(backup_path(&big), b"old").unwrap();
        assert!(backup_due(&small, &backup_path(&small)), "a small file's backup always");
        assert!(!backup_due(&big, &backup_path(&big)), "a big one's backup made just now isn't copied again");
        let long_ago = std::time::SystemTime::now() - BIG_BACKUP_EVERY - std::time::Duration::from_secs(1);
        std::fs::File::options().write(true).open(backup_path(&big)).unwrap().set_modified(long_ago).unwrap();
        assert!(backup_due(&big, &backup_path(&big)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_changes_are_saved_once_they_settle() {
        let (tx, rx) = crossbeam_channel::unbounded();
        // Changes 50 ms apart: still coming, so it waits; the last one is what's saved.
        let sender = std::thread::spawn(move || {
            for i in 1..=5 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                tx.send(i).unwrap();
            }
            tx
        });
        let started = std::time::Instant::now();
        let got = settle(&rx, 0, std::time::Duration::from_millis(200), std::time::Duration::from_secs(5));
        assert_eq!(got, Some(5));
        assert!(started.elapsed() >= std::time::Duration::from_millis(400));
        // Changes that never stop are still saved by the longest wait.
        let tx = sender.join().unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let flood = std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = tx.send(1);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        let started = std::time::Instant::now();
        assert!(settle(&rx, 0, std::time::Duration::from_millis(100), std::time::Duration::from_millis(300)).is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        flood.join().unwrap();
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
    use super::{Aliases, Finder, match_key, moved_songs, really_gone, remember_moves, same_artist};
    use crate::library::Track;

    fn song(id: &str, title: &str, artist: &str, album: &str, duration: f64) -> Track {
        Track { id: id.into(), title: title.into(), artist: artist.into(), album: album.into(), duration, ..Default::default() }
    }

    /// (old id, new id) for the `gone` songs in the library `now`.
    fn moved(gone: &[Track], now: &[Track], aliases: Aliases) -> Vec<(String, String)> {
        moved_in(gone, now, &[], aliases)
    }

    fn moved_in(gone: &[Track], now: &[Track], folders: &[String], aliases: Aliases) -> Vec<(String, String)> {
        let gone: Vec<&Track> = gone.iter().collect();
        moved_songs(&gone, &Finder::new(now, folders, aliases))
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
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
        assert_eq!(moved(&gone, &now, Aliases::new()), [pair("m4a", "flac")]);
        // A FLAC of another edition (its album is called something else): the only one, followed.
        let now = vec![song("single", "Lemon", "Kenshi Yonezu", "Lemon - Single", 255.5)];
        assert_eq!(moved(&gone[..1], &now, Aliases::new()), [pair("m4a", "single")]);
    }

    #[test]
    fn a_cover_by_another_artist_is_not_the_song() {
        // Same title, same length to the tenth of a second: someone else's, so not followed.
        let gone = [song("orig", "Lemon", "Kenshi Yonezu", "Lemon", 255.0)];
        let now = vec![song("cover", "Lemon", "Some Cover Band", "Lemon", 255.0)];
        assert!(moved(&gone, &now, Aliases::new()).is_empty());
        // Nor a song by someone whose name only contains the artist's.
        let now = vec![song("tribute", "Lemon", "Kenshi Yonezu Tribute Band", "Lemon", 255.0)];
        assert!(moved(&gone, &now, Aliases::new()).is_empty());
    }

    #[test]
    fn another_albums_intro_is_not_followed() {
        // The song's album went; the artist has an "Intro" on two other albums: either could be
        // the one, so neither.
        let gone = [song("intro", "Intro", "YOASOBI", "THE BOOK", 90.0)];
        let now = vec![song("i2", "Intro", "YOASOBI", "THE BOOK 2", 90.1), song("i3", "Intro", "YOASOBI", "THE BOOK 3", 91.5)];
        assert!(moved(&gone, &now, Aliases::new()).is_empty());
        // On its own album, the closest in length is it…
        let now = vec![song("i2", "Intro", "YOASOBI", "THE BOOK 2", 90.1), song("flac", "Intro", "YOASOBI", "THE BOOK", 90.2)];
        assert_eq!(moved(&gone, &now, Aliases::new()), [pair("intro", "flac")]);
        let now = vec![song("a", "Intro", "YOASOBI", "THE BOOK", 90.2), song("b", "Intro", "YOASOBI", "THE BOOK", 91.6)];
        assert_eq!(moved(&gone, &now, Aliases::new()), [pair("intro", "a")]);
        // …unless two are as close as each other.
        let now = vec![song("a", "Intro", "YOASOBI", "THE BOOK", 90.2), song("b", "Intro", "YOASOBI", "THE BOOK", 89.9)];
        assert!(moved(&gone, &now, Aliases::new()).is_empty());
        // Two songs of different albums can't both be the one copy left.
        let gone = [song("x1", "Intro", "YOASOBI", "THE BOOK", 90.0), song("x2", "Intro", "YOASOBI", "THE BOOK 2", 90.0)];
        let now = vec![song("only", "Intro", "YOASOBI", "THE BOOK 3", 90.0)];
        assert!(moved(&gone, &now, Aliases::new()).is_empty());
    }

    #[test]
    fn the_same_artist_in_another_script_or_credit_is_followed() {
        let aliases: Aliases = crate::identity::Identity::from_pairs(&[("米津玄師", "Kenshi Yonezu")]);
        let gone = [song("jp", "Lemon", "米津玄師", "Lemon", 255.0)];
        let now = vec![song("flac", "Lemon", "Kenshi Yonezu", "STRAY SHEEP", 255.3), song("cover", "Lemon", "Someone", "Lemon", 255.0)];
        assert_eq!(moved(&gone, &now, aliases.clone()), [pair("jp", "flac")]);
        // Without the catalogue's other name, a kanji name and a Latin one can't be told the same.
        assert!(moved(&gone, &now, Aliases::new()).is_empty());
        // And the other way round.
        let gone = [song("en", "Lemon", "Kenshi Yonezu", "Lemon", 255.0)];
        let now = vec![song("jp", "Lemon", "米津玄師", "Lemon", 255.0)];
        assert_eq!(moved(&gone, &now, aliases.clone()), [pair("en", "jp")]);
        // "feat." credits and spelling.
        assert!(same_artist("Kenshi Yonezu", "Kenshi Yonezu feat. Hatsune Miku", &Aliases::new()));
        assert!(same_artist("REOL", "Reol", &Aliases::new()));
        assert!(same_artist("Hikaru Utada & 米津玄師", "Kenshi Yonezu", &aliases));
        assert!(!same_artist("Kenshi Yonezu", "Someone Else", &aliases));
        // A name in kana, read in Latin letters.
        assert!(same_artist("りりあ。", "Riria.", &Aliases::new()));
    }

    #[test]
    fn what_cadence_knows_comes_first() {
        // A copy Cadence removed as a duplicate: to the copy that stayed, though its tags differ.
        remember_moves([("dedupe-removed", "dedupe-kept")]);
        let gone = [song("dedupe-removed", "Lemon (Remastered)", "米津玄師", "Lemon", 255.0)];
        let now = vec![song("dedupe-kept", "Lemon", "Kenshi Yonezu", "STRAY SHEEP", 254.0)];
        assert_eq!(moved(&gone, &now, Aliases::new()), [pair("dedupe-removed", "dedupe-kept")]);
        // The downloader put a FLAC where the m4a was (its title is spelt otherwise).
        let dir = std::env::temp_dir().join(format!("cadence-replaced-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut m4a = song("old-m4a", "lemon (official audio)", "Unknown Artist", "Downloads", 255.0);
        m4a.path = dir.join("Lemon.m4a").to_string_lossy().into_owned();
        let mut flac = song("new-flac", "Lemon", "Kenshi Yonezu", "Lemon", 255.4);
        flac.path = dir.join("Lemon.flac").to_string_lossy().into_owned();
        let folders = [dir.to_string_lossy().into_owned()];
        assert_eq!(moved_in(std::slice::from_ref(&m4a), &[flac.clone()], &folders, Aliases::new()), [pair("old-m4a", "new-flac")]);
        // The m4a is still there (it was only unreadable for a moment): it didn't move.
        std::fs::write(&m4a.path, b"x").unwrap();
        assert!(moved_in(&[m4a], &[flac], &folders, Aliases::new()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn songs_of_an_unplugged_drive_are_not_followed_to_look_alikes() {
        let base = std::env::temp_dir().join(format!("cadence-offline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Here")).unwrap();
        let unplugged = base.join("Unplugged");
        let mut old = song("off", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        old.path = unplugged.join("Lemon.flac").to_string_lossy().into_owned();
        let mut there = song("there", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        there.path = base.join("Here").join("Lemon.flac").to_string_lossy().into_owned();
        let folders = [unplugged.to_string_lossy().into_owned(), base.join("Here").to_string_lossy().into_owned()];
        assert!(moved_in(std::slice::from_ref(&old), std::slice::from_ref(&there), &folders, Aliases::new()).is_empty());
        assert!(!really_gone(std::path::Path::new(&old.path), &folders.iter().map(std::path::PathBuf::from).collect::<Vec<_>>()));
        // Plugged in again, the file really gone (moved to the other folder): followed.
        std::fs::create_dir_all(&unplugged).unwrap();
        assert_eq!(moved_in(&[old], &[there], &folders, Aliases::new()), [pair("off", "there")]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn names_match_across_spacing_case_and_punctuation() {
        assert_eq!(match_key("コノヨ Loading..."), match_key("コノヨLoading…"));
        assert_eq!(match_key("REOL"), match_key("Reol"));
        assert_ne!(match_key("RE"), match_key("REOL"));
    }
}
