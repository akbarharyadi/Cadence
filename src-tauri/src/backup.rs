//! Backup and restore of what Cadence can't build again: settings (without secrets), playlists,
//! favourites and ratings, the whole listening history, the queue, artist pictures the listener
//! chose, lyrics they picked or pasted, lyrics timing nudges, and what they told Needs Attention.
//! Everything a rescan makes again (the library cache, sound analysis, loudness, album art, voice
//! models, logs) is left out.
//!
//! One zip file: the files as they are in the data folder, a `manifest.json` (format, app
//! version, date, every file with its size and hash) and `tracks.json` (what each song the
//! backup mentions was: title, artist, album, length, file), so that a backup restored where the
//! music lives elsewhere can find its songs again (`relink`).
//!
//! Rolling snapshots of the same kind are kept in `backups/` in the data folder: one when Cadence
//! starts if the last is a day old and something changed (7 daily and 4 weekly are kept), and one
//! before every restore (`safety-…`), optionally copied to a second folder.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::library::Track;
use crate::state::AppState;

/// The backup format this Cadence writes; it reads this one and every older one.
pub const FORMAT: u32 = 1;
const MANIFEST: &str = "manifest.json";
const LINKS: &str = "tracks.json";
/// Lyrics timing nudges (the window keeps them in its own storage and hands them over).
pub const OFFSETS: &str = "lyric-offsets.json";
/// Songs a restore couldn't find yet: tried again after every rescan.
const PENDING: &str = "relink-pending.json";

/// Limits: no backup of a music player's own data comes near these; an archive that does is
/// something else (or made to fill the disk).
const MAX_ARCHIVE: u64 = 1 << 30;
const MAX_TOTAL: u64 = 1 << 30;
const MAX_FILE: u64 = 256 << 20;
const MAX_PICTURE: u64 = 20 << 20;
const MAX_MANIFEST: u64 = 16 << 20;
const MAX_ENTRIES: usize = 200_000;

const DAILY: usize = 7;
const WEEKLY: usize = 4;
const SAFETY_KEPT: usize = 5;
const DAY: u64 = 86_400;

/// The data folder's own files, kept whole.
const WHOLE: &[&str] = &[
    "settings.json",
    "playlists.json",
    "marks.json",
    "history.json",
    "session.json",
    "attention.json",
    // (Also the user's "same artist" / "not the same artist" decisions. MusicBrainz's answers,
    // musicbrainz-artists.json, and search-readings.json are caches built again: not kept.)
    "artist-names.json",
    "artist-images.json",
    OFFSETS,
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// One of `WHOLE`.
    Whole,
    /// `history-archive/<year>.jsonl`.
    Archive,
    /// `lyrics/<song id>.json`: lyrics the listener picked, pasted or hid.
    Lyrics,
    /// `art/artists/<name>.jpg`: an artist picture the listener chose.
    Picture,
}

/// What an entry of a backup is, from its name; None for anything else (a path that leaves the
/// data folder, an absolute one, a drive letter, a file Cadence never writes).
fn kind(name: &str) -> Option<Kind> {
    if name.is_empty() || name.len() > 200 || name.contains(['\\', ':', '\0']) || name.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = name.split('/').collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
        return None;
    }
    let safe = |s: &str| !s.is_empty() && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    match parts.as_slice() {
        [f] if WHOLE.contains(f) => Some(Kind::Whole),
        ["history-archive", f] => f
            .strip_suffix(".jsonl")
            .filter(|y| (1..=6).contains(&y.len()) && y.chars().all(|c| c.is_ascii_digit()))
            .map(|_| Kind::Archive),
        ["lyrics", f] => f.strip_suffix(".json").filter(|id| safe(id) && !id.contains('.')).map(|_| Kind::Lyrics),
        ["art", "artists", f] => (safe(f) && (f.ends_with(".jpg") || f.ends_with(".png"))).then_some(Kind::Picture),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: u32,
    /// The Cadence version that made it.
    pub app: String,
    /// Unix seconds.
    pub created: u64,
    /// "manual", "auto" or "safety".
    #[serde(default)]
    pub kind: String,
    pub files: Vec<FileEntry>,
    /// The data folder's files (names, sizes, times) when it was made: an automatic snapshot is
    /// skipped when nothing changed since the last one.
    #[serde(default)]
    pub fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    /// BLAKE3, hex.
    pub hash: String,
}

/// What a song was when the backup was made: enough to find it again (`relink`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Link {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
}

impl Link {
    pub fn of(t: &Track) -> Self {
        Link {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration: t.duration,
            // A song of a single-file album is told by its tags, not the album's file.
            path: if t.cue.is_none() { t.path.clone() } else { String::new() },
        }
    }

    fn track(&self, id: &str) -> Track {
        Track {
            id: id.to_string(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration: self.duration,
            path: self.path.clone(),
            ..Default::default()
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn io(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn backups_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

// ---------- what goes in ----------

/// A file of the data folder that goes into a backup, by its name in the backup.
struct Source {
    name: String,
    path: PathBuf,
}

/// Everything a backup would hold now (lyrics: only those the listener chose).
fn sources(data_dir: &Path) -> Vec<Source> {
    let mut out: Vec<Source> = WHOLE
        .iter()
        .map(|n| Source { name: n.to_string(), path: data_dir.join(n) })
        .filter(|s| s.path.is_file())
        .collect();
    for p in crate::history::archive_files(data_dir) {
        let name = format!("history-archive/{}", p.file_name().unwrap_or_default().to_string_lossy());
        if kind(&name) == Some(Kind::Archive) {
            out.push(Source { name, path: p });
        }
    }
    for p in lyric_files(data_dir) {
        if is_picked(&p) {
            out.push(Source { name: format!("lyrics/{}", p.file_name().unwrap_or_default().to_string_lossy()), path: p });
        }
    }
    for p in pictures(data_dir) {
        let name = format!("art/artists/{}", p.file_name().unwrap_or_default().to_string_lossy());
        if kind(&name) == Some(Kind::Picture) && p.is_file() {
            out.push(Source { name, path: p });
        }
    }
    out
}

/// `lyrics/<id>.json` files (the saved lyrics; not the placed-on-the-singing ones).
fn lyric_files(data_dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(data_dir.join("lyrics"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy();
            kind(&format!("lyrics/{n}")) == Some(Kind::Lyrics)
        })
        .collect();
    v.sort();
    v
}

/// Lyrics the listener chose (searched, pasted, made by the AI, or hidden as wrong), not ones
/// found online on their own (those are found again).
fn is_picked(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| v.get("picked").and_then(|p| p.as_bool()) == Some(true))
}

/// The artist pictures `artist-images.json` points at (in the art folder's `artists/`).
fn pictures(data_dir: &Path) -> Vec<PathBuf> {
    let dir = data_dir.join("art").join("artists");
    let index: HashMap<String, crate::artists::ArtistImage> = std::fs::read(data_dir.join("artist-images.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut v: Vec<PathBuf> = index
        .values()
        .flat_map(|i| [&i.large, &i.small])
        .filter_map(|p| Path::new(p).file_name().map(|n| dir.join(n)))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The sources' names, sizes and times, hashed: changes when any of them does.
fn fingerprint(sources: &[Source]) -> String {
    let mut h = blake3::Hasher::new();
    for s in sources {
        let m = std::fs::metadata(&s.path).ok();
        let modified = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        h.update(s.name.as_bytes());
        h.update(&m.map_or(0, |m| m.len()).to_le_bytes());
        h.update(&modified.to_le_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// Settings keys that hold secrets (none do today: keys and tokens are in the system's credential
/// store; this keeps one that ever slips into settings out of backups).
fn secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k == "key" || ["token", "secret", "password", "passwd", "apikey", "api_key", "credential"].iter().any(|w| k.contains(w))
}

fn strip_secrets(v: &mut serde_json::Value) -> bool {
    let mut removed = false;
    match v {
        serde_json::Value::Object(map) => {
            let before = map.len();
            map.retain(|k, _| !secret_key(k));
            removed |= map.len() != before;
            for x in map.values_mut() {
                removed |= strip_secrets(x);
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                removed |= strip_secrets(x);
            }
        }
        _ => {}
    }
    removed
}

/// A file as it goes into a backup: checked (a damaged one is left out, so a backup can always
/// be restored), settings without secrets.
fn prepare(name: &str, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    check(name, &bytes)?;
    if name == "settings.json" {
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).map_err(io)?;
        if strip_secrets(&mut v) {
            return serde_json::to_vec(&v).map_err(io);
        }
    }
    Ok(bytes)
}

/// The song ids the data mentions: (playlists, favourites, the queue, skipped songs, lyrics), and
/// the listening history's.
fn mentioned(data_dir: &Path) -> (HashSet<String>, HashSet<String>) {
    let mut kept = HashSet::new();
    let read = |n: &str| std::fs::read(data_dir.join(n)).ok();
    if let Some(v) = read("marks.json").and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        && let Some(items) = v.get("items").and_then(|i| i.as_object()) {
            kept.extend(items.keys().cloned());
        }
    if let Some(p) = read("playlists.json").and_then(|b| serde_json::from_slice::<crate::playlists::Playlists>(&b).ok()) {
        kept.extend(p.items.into_iter().flat_map(|p| p.track_ids));
    }
    if let Some(s) = read("session.json").and_then(|b| serde_json::from_slice::<crate::state::Session>(&b).ok()) {
        kept.extend(s.queue);
    }
    if let Some(s) = read("settings.json").and_then(|b| serde_json::from_slice::<crate::state::Settings>(&b).ok()) {
        kept.extend(s.skipped);
    }
    if let Some(o) = read(OFFSETS).and_then(|b| serde_json::from_slice::<HashMap<String, f64>>(&b).ok()) {
        kept.extend(o.into_keys().map(|k| k.trim_end_matches(":voice").to_string()));
    }
    for p in lyric_files(data_dir) {
        if let Some(id) = p.file_stem() {
            kept.insert(id.to_string_lossy().into_owned());
        }
    }
    let mut heard = HashSet::new();
    if let Some(h) = read("history.json").and_then(|b| serde_json::from_slice::<crate::history::History>(&b).ok()) {
        heard.extend(h.events.into_iter().map(|e| e.track_id));
    }
    for f in crate::history::archive_files(data_dir) {
        let _ = crate::history::read_archive_from(&f, 0, |e| {
            heard.insert(e.track_id);
        });
    }
    (kept, heard)
}

/// What a backup was made of.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub path: String,
    pub files: usize,
    pub bytes: u64,
    /// Files left out because they're damaged.
    pub skipped: Vec<String>,
    /// Said when the backup isn't where the listener might expect (Android).
    pub note: String,
}

/// Writes a backup of `data_dir` to `out` (whole or not at all: a temporary file beside it, put in
/// place at the end). `lookup`: the library's song of an id. Each file is read, checked and
/// written into the zip one at a time, so memory holds one file at most.
pub fn create(data_dir: &Path, out: &Path, kind_label: &str, lookup: &dyn Fn(&str) -> Option<Link>) -> Result<Made, String> {
    let sources = sources(data_dir);
    let fp = fingerprint(&sources);
    let (kept, heard) = mentioned(data_dir);
    let links: std::collections::BTreeMap<String, Link> = kept.iter().chain(&heard).filter_map(|id| lookup(id).map(|l| (id.clone(), l))).collect();
    let dir = out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't make the folder {}: {e}", dir.display()))?;
    let tmp = tempfile::Builder::new()
        .prefix(".cadence-backup.")
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(|e| format!("Can't write in {}: {e}", dir.display()))?;
    let mut zip = zip::ZipWriter::new(tmp.as_file());
    let packed = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for s in &sources {
        let Ok(bytes) = std::fs::read(&s.path) else {
            skipped.push(s.name.clone());
            continue;
        };
        let bytes = match prepare(&s.name, bytes) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("backup: leaving out {} ({e})", s.name);
                skipped.push(s.name.clone());
                continue;
            }
        };
        let opts = if kind(&s.name) == Some(Kind::Picture) { stored } else { packed };
        zip.start_file(s.name.as_str(), opts).map_err(io)?;
        zip.write_all(&bytes).map_err(io)?;
        files.push(FileEntry { name: s.name.clone(), size: bytes.len() as u64, hash: hex(&bytes) });
    }
    let links = serde_json::to_vec(&links).map_err(io)?;
    zip.start_file(LINKS, packed).map_err(io)?;
    zip.write_all(&links).map_err(io)?;
    let manifest = Manifest {
        format: FORMAT,
        app: env!("CARGO_PKG_VERSION").to_string(),
        created: crate::history::now(),
        kind: kind_label.to_string(),
        files: files.clone(),
        fingerprint: fp,
    };
    zip.start_file(MANIFEST, packed).map_err(io)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(io)?).map_err(io)?;
    zip.finish().map_err(io)?;
    tmp.as_file().sync_all().map_err(io)?;
    tmp.persist(out).map_err(|e| format!("Can't save {}: {}", out.display(), e.error))?;
    let bytes = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    Ok(Made { path: out.to_string_lossy().into_owned(), files: files.len(), bytes, skipped, note: String::new() })
}

// ---------- reading one back ----------

/// A backup read and checked whole: nothing in it has been used yet.
pub struct Archive {
    pub manifest: Manifest,
    files: Vec<(String, Kind, Vec<u8>)>,
    links: HashMap<String, Link>,
}

/// "0.2.40" → [0, 2, 40].
fn version(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect()
}

/// Checks a file's contents for what its name says it is. JSON must parse as the data Cadence
/// keeps there; a history year may have a few damaged lines (a crash mid-write), not more.
fn check(name: &str, bytes: &[u8]) -> Result<(), String> {
    let bad = |e: String| format!("{name} is damaged ({e})");
    match kind(name) {
        Some(Kind::Whole) => {
            let r: Result<(), serde_json::Error> = match name {
                "settings.json" => serde_json::from_slice::<crate::state::Settings>(bytes).map(drop),
                "playlists.json" => serde_json::from_slice::<crate::playlists::Playlists>(bytes).map(drop),
                "marks.json" => serde_json::from_slice::<crate::marks::Marks>(bytes).map(drop),
                "history.json" => serde_json::from_slice::<crate::history::History>(bytes).map(drop),
                "session.json" => serde_json::from_slice::<crate::state::Session>(bytes).map(drop),
                "attention.json" => serde_json::from_slice::<crate::attention::Attention>(bytes).map(drop),
                "artist-images.json" => serde_json::from_slice::<HashMap<String, crate::artists::ArtistImage>>(bytes).map(drop),
                OFFSETS => serde_json::from_slice::<HashMap<String, f64>>(bytes).map(drop),
                _ => serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(bytes).map(drop),
            };
            r.map_err(|e| bad(e.to_string()))
        }
        Some(Kind::Lyrics) => serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(bytes).map(drop).map_err(|e| bad(e.to_string())),
        Some(Kind::Archive) => {
            let text = std::str::from_utf8(bytes).map_err(|e| bad(e.to_string()))?;
            let (mut lines, mut damaged) = (0usize, 0usize);
            for l in text.lines().filter(|l| !l.trim().is_empty()) {
                lines += 1;
                damaged += serde_json::from_str::<crate::history::PlayEvent>(l).is_err() as usize;
            }
            if damaged > 2 + lines / 100 {
                return Err(bad(format!("{damaged} of {lines} plays can't be read")));
            }
            Ok(())
        }
        Some(Kind::Picture) => {
            let image = bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(b"\x89PNG\r\n\x1a\n");
            if image { Ok(()) } else { Err(bad("not a JPEG or PNG picture".into())) }
        }
        None => Err(format!("{name} isn't something a Cadence backup holds")),
    }
}

/// Reads a backup and checks all of it before anything is used: its size, that it's a zip with
/// a manifest this Cadence can read (not made by a newer one), that every entry stays inside the
/// data folder and is one Cadence writes, sizes (also as really unpacked: a zip can lie), hashes,
/// and that every file parses.
pub fn open(path: &Path) -> Result<Archive, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("Can't open {}: {e}", path.display()))?.len();
    if size > MAX_ARCHIVE {
        return Err(format!("{} is too big to be a Cadence backup.", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("Can't open {}: {e}", path.display()))?;
    let not_ours = || format!("{} isn't a Cadence backup.", path.display());
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| not_ours())?;
    if zip.len() > MAX_ENTRIES {
        return Err(not_ours());
    }
    let manifest: Manifest = {
        let entry = zip.by_name(MANIFEST).map_err(|_| not_ours())?;
        let bytes = read_entry(entry, MAX_MANIFEST).map_err(|_| not_ours())?;
        serde_json::from_slice(&bytes).map_err(|_| format!("{} is a damaged backup (its list of files can't be read).", path.display()))?
    };
    let ours = env!("CARGO_PKG_VERSION");
    if manifest.format > FORMAT || version(&manifest.app) > version(ours) {
        return Err(format!(
            "This backup was made by Cadence {}, newer than this one ({ours}). Update Cadence first, then restore it.",
            manifest.app
        ));
    }
    let listed: HashMap<&str, &FileEntry> = manifest.files.iter().map(|f| (f.name.as_str(), f)).collect();
    if listed.len() != manifest.files.len() {
        return Err(format!("{} is a damaged backup (a file is listed twice).", path.display()));
    }
    let mut files = Vec::new();
    let mut links = HashMap::new();
    let mut total = 0u64;
    let mut seen = HashSet::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|_| not_ours())?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().map_err(|_| not_ours())?.into_owned();
        if name == MANIFEST {
            continue;
        }
        if entry.enclosed_name().is_none() || (name != LINKS && kind(&name).is_none()) {
            return Err(format!("The backup holds a file Cadence won't restore: {name}"));
        }
        if !seen.insert(name.clone()) {
            return Err(format!("The backup holds {name} twice."));
        }
        let limit = if kind(&name) == Some(Kind::Picture) { MAX_PICTURE } else { MAX_FILE };
        if entry.size() > limit || total + entry.size() > MAX_TOTAL {
            return Err(format!("{name} in the backup is too big."));
        }
        let bytes = read_entry(entry, limit).map_err(|_| format!("{name} in the backup is too big or damaged."))?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL {
            return Err("The backup is too big.".into());
        }
        if name == LINKS {
            links = serde_json::from_slice(&bytes).map_err(|e| format!("tracks.json is damaged ({e})"))?;
            continue;
        }
        let Some(f) = listed.get(name.as_str()) else {
            return Err(format!("{name} isn't in the backup's list of files."));
        };
        if f.size != bytes.len() as u64 || f.hash != hex(&bytes) {
            return Err(format!("{name} in the backup is damaged (it isn't what was saved)."));
        }
        check(&name, &bytes)?;
        files.push((name.clone(), kind(&name).unwrap_or(Kind::Whole), bytes));
    }
    if let Some(missing) = manifest.files.iter().find(|f| !seen.contains(&f.name)) {
        return Err(format!("The backup is incomplete: {} is missing.", missing.name));
    }
    Ok(Archive { manifest, files, links })
}

/// An entry's bytes, at most `limit` of them whatever its header says.
fn read_entry(entry: impl Read, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    entry.take(limit + 1).read_to_end(&mut out)?;
    if out.len() as u64 > limit {
        return Err(std::io::Error::other("too big"));
    }
    Ok(out)
}

/// What a backup holds, in words, for the question before a restore.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub path: String,
    pub app: String,
    pub created: u64,
    pub kind: String,
    /// What a restore replaces, one line each ("Playlists (4)").
    pub replaces: Vec<String>,
}

pub fn preview(path: &Path) -> Result<Preview, String> {
    let a = open(path)?;
    Ok(Preview {
        path: path.to_string_lossy().into_owned(),
        app: a.manifest.app.clone(),
        created: a.manifest.created,
        kind: a.manifest.kind.clone(),
        replaces: describe(&a),
    })
}

fn describe(a: &Archive) -> Vec<String> {
    let get = |n: &str| a.files.iter().find(|(name, _, _)| name == n).map(|(_, _, b)| b.as_slice());
    let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut out = Vec::new();
    if get("settings.json").is_some() {
        out.push("Settings (this device keeps its own music folders, volume and ListenBrainz account)".to_string());
    }
    if let Some(p) = get("playlists.json").and_then(|b| serde_json::from_slice::<crate::playlists::Playlists>(b).ok()) {
        out.push(format!("Playlists ({})", p.items.len()));
    }
    if let Some(m) = get("marks.json").and_then(|b| serde_json::from_slice::<crate::marks::Marks>(b).ok()) {
        out.push(format!("Favorites and ratings ({})", count(m.view().len(), "song", "songs")));
    }
    let live = get("history.json").and_then(|b| serde_json::from_slice::<crate::history::History>(b).ok()).map_or(0, |h| h.events.len());
    let archived: usize = a
        .files
        .iter()
        .filter(|(_, k, _)| *k == Kind::Archive)
        .map(|(_, _, b)| b.split(|c| *c == b'\n').filter(|l| serde_json::from_slice::<crate::history::PlayEvent>(l).is_ok()).count())
        .sum();
    if live + archived > 0 || get("history.json").is_some() {
        out.push(format!("Listening history ({})", count(live + archived, "play", "plays")));
    }
    if get("session.json").is_some() {
        out.push("The queue (what's playing now stops)".to_string());
    }
    if get("attention.json").is_some() {
        out.push("Problems you told Needs Attention to ignore".to_string());
    }
    if get("artist-names.json").is_some() {
        out.push("Artist names matched across scripts, and pairs you said are or aren't one artist".to_string());
    }
    let pictures = get("artist-images.json").and_then(|b| serde_json::from_slice::<HashMap<String, serde_json::Value>>(b).ok()).map_or(0, |m| m.len());
    if pictures > 0 {
        out.push(format!("Artist pictures you chose ({pictures})"));
    }
    let lyrics = a.files.iter().filter(|(_, k, _)| *k == Kind::Lyrics).count();
    if lyrics > 0 {
        out.push(format!("Lyrics you picked or pasted ({})", count(lyrics, "song", "songs")));
    }
    if let Some(o) = get(OFFSETS).and_then(|b| serde_json::from_slice::<HashMap<String, f64>>(b).ok())
        && !o.is_empty() {
            out.push(format!("Lyrics timing adjustments ({})", count(o.len(), "song", "songs")));
        }
    out
}

// ---------- restoring ----------

/// What a restore did.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    /// Files put back.
    pub files: usize,
    /// Songs found again under their new ids (their files moved, or the music is elsewhere).
    pub relinked: usize,
    /// Songs of playlists, favourites, the queue or skipped songs not in the library now: kept,
    /// and found again if they come back (tried after every rescan).
    pub unresolved: usize,
    /// The same for songs only in the listening history.
    pub unresolved_history: usize,
    /// The snapshot of the data before the restore.
    pub safety: String,
    /// The lyrics timing nudges, for the window to keep (null: the backup had none).
    pub lyric_offsets: Option<String>,
    pub app: String,
    pub created: u64,
}

/// Puts the data's latest state on disk (settings waiting to be saved, the queue's position).
fn flush(state: &AppState) {
    state.save_settings_now();
    let s = state.engine.status();
    if !s.queue.is_empty() {
        state.save_session(&s);
    }
}

/// Every song of the library as a `Link`, read under one lock (not one per song of a big history).
fn library_links(state: &AppState) -> HashMap<String, Link> {
    let lib = state.library.lock();
    lib.snapshot.tracks.iter().map(|t| (t.id.clone(), Link::of(t))).collect()
}

/// A snapshot of the data now into `backups/` (`kind`: "auto" or "safety").
pub(crate) fn snapshot(state: &AppState, kind_label: &str) -> Result<PathBuf, String> {
    let dir = backups_dir(&state.data_dir);
    let now = crate::history::now();
    let mut path = dir.join(format!("{kind_label}-{now}.zip"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{kind_label}-{now}-{n}.zip"));
        n += 1;
    }
    let links = library_links(state);
    create(&state.data_dir, &path, kind_label, &|id| links.get(id).cloned())?;
    Ok(path)
}

/// Restores a backup into the running app. In order, stopping at the first problem:
/// 1. the whole backup is read and checked (`open`); nothing has changed yet;
/// 2. a snapshot of the data as it is now goes into `backups/` (`safety-…`); no snapshot, no restore;
/// 3. each file is replaced through the usual atomic save (with its `.bak`), and what Cadence holds
///    in memory is replaced with it at the same moment (the file's lock held, so no save of the old
///    data can come between); if a file can't be written, the safety snapshot is put back;
/// 4. songs that aren't in the library under the backup's ids are found again (`relink`);
/// 5. the player gets the restored settings and queue (paused), and the window is told to reload.
pub fn restore(ctx: &Ctx, path: &Path) -> Result<Restored, String> {
    let archive = open(path)?;
    let state = &ctx.state;
    flush(state);
    let safety = snapshot(state, "safety").map_err(|e| format!("Nothing was restored: a copy of your data as it is now couldn't be made first ({e})."))?;
    rotate_safety(&backups_dir(&state.data_dir));
    let old_settings = state.settings.lock().clone();
    let files = match put_back(state, &archive) {
        Ok(n) => n,
        Err(e) => {
            // Back to how it was.
            if let Ok(before) = open(&safety) {
                let _ = put_back(state, &before);
            }
            crate::stats::invalidate(&state.data_dir);
            return Err(format!("The backup couldn't be restored ({e}); your data is as it was."));
        }
    };
    crate::stats::invalidate(&state.data_dir);
    let restored_ids: Vec<String> = archive.files.iter().filter(|(_, k, _)| *k == Kind::Lyrics).filter_map(|(n, _, _)| Path::new(n).file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
    let linked = relink(state, &archive.links, &restored_ids);
    crate::stats::invalidate(&state.data_dir);
    apply_to_player(ctx, &old_settings, archive.files.iter().any(|(n, _, _)| n == "session.json"));
    for e in ["library-changed", "playlists-changed", "history-changed", "attention-changed", "marks-changed"] {
        ctx.emit(e, ());
    }
    let lyric_offsets = archive.files.iter().any(|(n, _, _)| n == OFFSETS).then(|| std::fs::read_to_string(state.data_dir.join(OFFSETS)).unwrap_or_else(|_| "{}".into()));
    Ok(Restored {
        files,
        relinked: linked.relinked,
        unresolved: linked.unresolved,
        unresolved_history: linked.unresolved_history,
        safety: safety.to_string_lossy().into_owned(),
        lyric_offsets,
        app: archive.manifest.app.clone(),
        created: archive.manifest.created,
    })
}

/// The restored settings with what belongs to this device kept: its music folders (unless it has
/// none yet), its volume (a DAC's level), the ListenBrainz account whose token is in this device's
/// credential store, and whether a batch job was running here. Unchanged bytes when nothing
/// differs.
fn merged_settings(current: Option<&crate::state::Settings>, bytes: &[u8]) -> Result<(Vec<u8>, crate::state::Settings), String> {
    let original: serde_json::Value = serde_json::from_slice(bytes).map_err(io)?;
    let restored = serde_json::to_value(serde_json::from_value::<crate::state::Settings>(original.clone()).map_err(io)?).map_err(io)?;
    let mut v = original.clone();
    if let (Some(cur), Some(obj)) = (current, v.as_object_mut()) {
        let cur = serde_json::to_value(cur).map_err(io)?;
        let mut keep = vec!["volume", "listenbrainzUser", "voiceBatch"];
        if cur.get("folders").and_then(|f| f.as_array()).is_some_and(|f| !f.is_empty()) {
            keep.push("folders");
        }
        for k in keep {
            // Only where it differs: a backup whose values are this device's anyway stays as it was.
            if let Some(x) = cur.get(k).filter(|x| restored.get(k) != Some(*x)) {
                obj.insert(k.to_string(), x.clone());
            }
        }
    }
    let settings: crate::state::Settings = serde_json::from_value(v.clone()).map_err(io)?;
    let out = if v == original { bytes.to_vec() } else { serde_json::to_vec(&v).map_err(io)? };
    Ok((out, settings))
}

/// The artist pictures' index pointing at this data folder's art (the backup may come from
/// another computer or user).
fn local_pictures(data_dir: &Path, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut index: HashMap<String, crate::artists::ArtistImage> = serde_json::from_slice(bytes).map_err(io)?;
    let dir = data_dir.join("art").join("artists");
    let here = |p: &str| Path::new(p).file_name().map_or_else(|| p.to_string(), |n| dir.join(n).to_string_lossy().into_owned());
    let mut changed = false;
    for img in index.values_mut() {
        let (l, s) = (here(&img.large), here(&img.small));
        changed |= l != img.large || s != img.small;
        (img.large, img.small) = (l, s);
    }
    if changed { serde_json::to_vec(&index).map_err(io) } else { Ok(bytes.to_vec()) }
}

/// Replaces the data folder's files with the backup's (see `restore`, step 3). Returns how many.
fn put_back(state: &AppState, a: &Archive) -> Result<usize, String> {
    let dir = &state.data_dir;
    let write = |name: &str, bytes: &[u8], bak: bool| crate::state::write_file(&dir.join(name), bytes, bak).map_err(|e| format!("{name}: {e}"));
    let mut n = 0;
    for (name, kind, bytes) in &a.files {
        match (*kind, name.as_str()) {
            (Kind::Whole, "settings.json") => {
                let current = dir.join("settings.json").exists().then(|| state.settings.lock().clone());
                let (bytes, parsed) = merged_settings(current.as_ref(), bytes)?;
                let mut s = state.settings.lock();
                write(name, &bytes, true)?;
                *s = parsed;
            }
            (Kind::Whole, "playlists.json") => {
                let mut p = state.playlists.lock();
                write(name, bytes, true)?;
                *p = serde_json::from_slice(bytes).map_err(io)?;
            }
            (Kind::Whole, "marks.json") => {
                let mut m = state.marks.lock();
                write(name, bytes, true)?;
                *m = serde_json::from_slice(bytes).map_err(io)?;
            }
            (Kind::Whole, "history.json") => {
                // The archive with it, under the history's lock (plays are archived under it too).
                let mut h = state.history.lock();
                write(name, bytes, true)?;
                *h = serde_json::from_slice(bytes).map_err(io)?;
                let years: Vec<&(String, Kind, Vec<u8>)> = a.files.iter().filter(|(_, k, _)| *k == Kind::Archive).collect();
                for (y, _, b) in &years {
                    write(y, b, false)?;
                    n += 1;
                }
                // Years the backup doesn't have are gone from the restored history (they're in
                // the safety snapshot).
                for f in crate::history::archive_files(dir) {
                    let name = format!("history-archive/{}", f.file_name().unwrap_or_default().to_string_lossy());
                    if !years.iter().any(|(y, _, _)| *y == name) {
                        let _ = std::fs::remove_file(&f);
                    }
                }
            }
            (Kind::Whole, "artist-images.json") => write(name, &local_pictures(dir, bytes)?, true)?,
            (Kind::Whole, _) => write(name, bytes, true)?,
            // With history.json (a backup always has it when it has an archive).
            (Kind::Archive, _) if a.files.iter().any(|(n, _, _)| n == "history.json") => continue,
            (Kind::Archive | Kind::Lyrics | Kind::Picture, _) => write(name, bytes, false)?,
        }
        n += 1;
    }
    Ok(n)
}

/// The player and the rest of the app follow the restored settings (and queue).
fn apply_to_player(ctx: &Ctx, old: &crate::state::Settings, session: bool) {
    let state = &ctx.state;
    let new = state.settings.lock().clone();
    // Only what changed (turning shuffle "on" again would shuffle the queue anew).
    if new.dsp != old.dsp {
        state.engine.send(crate::audio::Cmd::SetDsp(new.dsp.clone()));
    }
    if serde_json::to_value(new.repeat).ok() != serde_json::to_value(old.repeat).ok() {
        state.engine.send(crate::audio::Cmd::SetRepeat(new.repeat));
    }
    if new.shuffle != old.shuffle {
        state.engine.send(crate::audio::Cmd::SetShuffle(new.shuffle));
    }
    if new.global_hotkeys != old.global_hotkeys {
        ctx.host().set_hotkeys(new.global_hotkeys);
    }
    if new.folders != old.folders {
        crate::jobs::watch_folders(ctx.clone());
        crate::jobs::start_scan(ctx.clone());
    }
    if session {
        state.restore_session();
    }
}

// ---------- finding songs again ----------

/// What `relink` did.
#[derive(Default, Debug)]
pub struct Linked {
    pub relinked: usize,
    pub unresolved: usize,
    pub unresolved_history: usize,
}

/// Points what was restored at the songs as the library has them now. A song whose id isn't in the
/// library is looked for with the same strict rules as a song whose file moved (`state::Finder`:
/// same title, same artist across scripts and credits, same length within 2.5 s; its album when
/// there are several; nothing when two are equally likely, nor when two songs of different albums
/// would land on the same one). A song whose file is still there (a rescan will find it) isn't
/// moved. Songs not found keep their ids and are tried again after every rescan (`retry_pending`).
/// `lyrics`: song ids whose picked lyrics were restored.
pub(crate) fn relink(state: &AppState, links: &HashMap<String, Link>, lyrics: &[String]) -> Linked {
    let dir = &state.data_dir;
    let offsets_file = dir.join(OFFSETS);
    let offsets: HashMap<String, f64> = std::fs::read(&offsets_file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let session: crate::state::Session = std::fs::read(dir.join("session.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    // The ids the data mentions.
    let mut kept: HashSet<String> = state.marks.lock().view().into_keys().collect();
    kept.extend(state.playlists.lock().items.iter().flat_map(|p| p.track_ids.iter().cloned()));
    kept.extend(state.settings.lock().skipped.iter().cloned());
    kept.extend(session.queue.iter().cloned());
    kept.extend(offsets.keys().map(|k| k.trim_end_matches(":voice").to_string()));
    kept.extend(lyrics.iter().cloned());
    let mut heard: HashSet<String> = state.history.lock().events.iter().map(|e| e.track_id.clone()).collect();
    for f in crate::history::archive_files(dir) {
        let _ = crate::history::read_archive_from(&f, 0, |e| {
            heard.insert(e.track_id);
        });
    }
    let marked: HashMap<String, Track> = state.marks.lock().as_tracks().into_iter().map(|t| (t.id.clone(), t)).collect();
    let folders = state.settings.lock().folders.clone();
    let pairs: Vec<(String, String)> = {
        let lib = state.library.lock();
        let gone: Vec<Track> = kept
            .iter()
            .chain(heard.iter())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|id| !lib.by_id.contains_key(id.as_str()))
            .filter_map(|id| links.get(id).map(|l| l.track(id)).or_else(|| marked.get(id).cloned()))
            // Still there: a rescan adds it under this very id.
            .filter(|t| t.path.is_empty() || !Path::new(&t.path).exists())
            .map(|mut t| {
                // Where the backup's music was may not exist here at all: told by its tags only.
                t.path.clear();
                t
            })
            .collect();
        if gone.is_empty() {
            Vec::new()
        } else {
            let finder = crate::state::Finder::new(&lib.snapshot.tracks, &folders, crate::libfix::artist_aliases(dir));
            crate::state::moved_songs(&gone.iter().collect::<Vec<_>>(), &finder)
        }
    };
    let moved: HashMap<String, String> = pairs.iter().cloned().collect();
    if !moved.is_empty() {
        crate::state::remember_moves(pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())));
        follow_everywhere(state, &moved, &session, &offsets);
    }
    let (unresolved, unresolved_history, pending) = {
        let lib = state.library.lock();
        let missing = |id: &String| !lib.by_id.contains_key(id.as_str()) && !moved.contains_key(id);
        let left = kept.iter().filter(|id| missing(id)).count();
        let left_heard = heard.iter().filter(|id| !kept.contains(*id) && missing(id)).count();
        let pending: HashMap<String, Link> = kept
            .iter()
            .chain(heard.iter())
            .filter(|id| missing(id))
            .filter_map(|id| links.get(id).map(|l| (id.clone(), l.clone())))
            .collect();
        (left, left_heard, pending)
    };
    let file = dir.join(PENDING);
    if pending.is_empty() {
        let _ = std::fs::remove_file(&file);
    } else {
        // Since when they're looked for (kept across the retries).
        let since = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Pending>(&b).ok()).map_or_else(crate::history::now, |p| p.since);
        crate::state::write_json_with(&file, &Pending { since, links: pending }, false);
    }
    Linked { relinked: moved.len(), unresolved, unresolved_history }
}

/// Songs found again (old id → new id): moved in the favourites, playlists, history (and its
/// archive), skipped songs, the saved queue, picked lyrics and timing nudges, each saved.
fn follow_everywhere(state: &AppState, moved: &HashMap<String, String>, session: &crate::state::Session, offsets: &HashMap<String, f64>) {
    let dir = &state.data_dir;
    let marks_moved = {
        let lib = state.library.lock();
        let mut marks = state.marks.lock();
        marks.follow(moved, |id| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]))
    };
    if marks_moved > 0 {
        state.save_marks();
    }
    if state.playlists.lock().follow(moved) > 0 {
        state.save_playlists();
    }
    // The new songs' albums, read before the history is locked (the library is never locked
    // while holding the history).
    let albums: HashMap<&str, String> = {
        let lib = state.library.lock();
        moved.values().filter_map(|id| lib.by_id.get(id).map(|&i| (id.as_str(), lib.snapshot.tracks[i].album_id.clone()))).collect()
    };
    let album_of = |id: &str| albums.get(id).cloned();
    {
        let mut h = state.history.lock();
        let mut changed = false;
        for e in h.events.iter_mut() {
            if let Some(new) = moved.get(&e.track_id) {
                e.track_id = new.clone();
                if let Some(a) = album_of(new) {
                    e.album_id = a;
                }
                changed = true;
            }
        }
        if changed {
            crate::state::write_json(&dir.join("history.json"), &*h);
        }
        // The archive's year files, rewritten whole where a play changed (under the history's
        // lock: nothing is archived meanwhile).
        for f in crate::history::archive_files(dir) {
            let mut out = Vec::new();
            let mut changed = false;
            let read = crate::history::read_archive_from(&f, 0, |mut e| {
                if let Some(new) = moved.get(&e.track_id) {
                    e.track_id = new.clone();
                    if let Some(a) = album_of(new) {
                        e.album_id = a;
                    }
                    changed = true;
                }
                if serde_json::to_writer(&mut out, &e).is_ok() {
                    out.push(b'\n');
                }
            });
            if read.is_ok() && changed
                && let Err(e) = crate::state::write_file(&f, &out, false) {
                    log::warn!("can't save {}: {e}", f.display());
                }
        }
    }
    {
        let mut s = state.settings.lock();
        let before = s.skipped.clone();
        s.skipped = before.iter().map(|id| moved.get(id).cloned().unwrap_or_else(|| id.clone())).collect();
        if s.skipped != before {
            crate::state::write_json(&dir.join("settings.json"), &*s);
        }
    }
    if session.queue.iter().any(|id| moved.contains_key(id)) {
        // Only the ids change; whatever else the session keeps stays as it is.
        let file = dir.join("session.json");
        if let Some(mut v) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            && let Some(q) = v.get_mut("queue").and_then(|q| q.as_array_mut()) {
                for id in q.iter_mut() {
                    if let Some(new) = id.as_str().and_then(|s| moved.get(s)) {
                        *id = serde_json::Value::String(new.clone());
                    }
                }
                crate::state::write_json(&file, &v);
            }
    }
    let lyrics = dir.join("lyrics");
    for (old, new) in moved {
        let (from, to) = (lyrics.join(format!("{old}.json")), lyrics.join(format!("{new}.json")));
        if from.exists() && (!to.exists() || !is_picked(&to)) {
            let _ = std::fs::rename(&from, &to);
        }
    }
    if offsets.keys().any(|k| moved.contains_key(k.trim_end_matches(":voice"))) {
        let renamed: HashMap<String, f64> = offsets
            .iter()
            .map(|(k, v)| {
                let (id, voice) = k.strip_suffix(":voice").map_or((k.as_str(), ""), |id| (id, ":voice"));
                (moved.get(id).map_or_else(|| k.clone(), |n| format!("{n}{voice}")), *v)
            })
            .collect();
        crate::state::write_json(&dir.join(OFFSETS), &renamed);
    }
}

/// Songs a restore couldn't find, looked for again after each rescan (`retry_pending`).
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Pending {
    /// Unix seconds of the restore.
    since: u64,
    links: HashMap<String, Link>,
}

/// How long `Pending` songs are looked for.
const PENDING_DAYS: u64 = 30;

/// After a rescan: songs a restore couldn't find are looked for again (a no-op without any).
pub fn retry_pending(ctx: &Ctx) {
    let file = ctx.state.data_dir.join(PENDING);
    if !file.exists() {
        return;
    }
    let pending: Pending = crate::state::read_json(&file);
    // Not for ever: each try reads the whole history, and after a month a song isn't coming back.
    if pending.links.is_empty() || crate::history::now().saturating_sub(pending.since) > PENDING_DAYS * DAY {
        let _ = std::fs::remove_file(&file);
        return;
    }
    let linked = relink(&ctx.state, &pending.links, &[]);
    if linked.relinked > 0 {
        log::info!("{} songs of a restored backup found again", linked.relinked);
        crate::stats::invalidate(&ctx.state.data_dir);
        for e in ["playlists-changed", "marks-changed", "history-changed"] {
            ctx.emit(e, ());
        }
    }
}

// ---------- snapshots ----------

/// Where automatic snapshots are copied too ("" for nowhere), kept in `backups/config.json` (it
/// belongs to this device, so it isn't in backups).
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Config {
    folder: String,
    /// The last automatic snapshot's fingerprint (`Manifest::fingerprint`).
    fingerprint: String,
    /// Why the last copy to `folder` failed ("" when it worked).
    folder_error: String,
}

fn config(data_dir: &Path) -> Config {
    std::fs::read(backups_dir(data_dir).join("config.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_config(data_dir: &Path, c: &Config) {
    crate::state::write_json_with(&backups_dir(data_dir).join("config.json"), c, false);
}

/// Snapshots in a folder: (unix seconds, path) of `<prefix>-<secs>.zip` files, newest first.
fn listed(dir: &Path, prefix: &str) -> Vec<(u64, PathBuf)> {
    let mut v: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            let secs = name.strip_prefix(prefix)?.strip_prefix('-')?.strip_suffix(".zip")?.split('-').next()?.parse().ok()?;
            Some((secs, p))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// Which snapshots to keep (by time): the newest of each of the last `DAILY` days that have one,
/// then the newest of each of the `WEEKLY` weeks before those.
fn kept(times: &[u64], utc_offset: i64) -> HashSet<u64> {
    let mut times = times.to_vec();
    times.sort_unstable_by(|a, b| b.cmp(a));
    let day = |t: u64| (t as i64 + utc_offset).div_euclid(DAY as i64);
    let mut keep = HashSet::new();
    let mut days = Vec::new();
    for &t in &times {
        let d = day(t);
        if days.len() < DAILY && !days.contains(&d) {
            days.push(d);
            keep.insert(t);
        }
    }
    let Some(&oldest) = days.last() else { return keep };
    let mut weeks = Vec::new();
    for &t in &times {
        let d = day(t);
        if keep.contains(&t) || d >= oldest {
            continue;
        }
        // Weeks from Monday (1970-01-01 was a Thursday).
        let w = (d + 3).div_euclid(7);
        if weeks.len() < WEEKLY && !weeks.contains(&w) {
            weeks.push(w);
            keep.insert(t);
        }
    }
    keep
}

/// Removes automatic snapshots `kept` doesn't keep.
fn rotate(dir: &Path) {
    let all = listed(dir, "auto");
    let keep = kept(&all.iter().map(|(t, _)| *t).collect::<Vec<_>>(), crate::stats::local_offset());
    for (t, p) in all {
        if !keep.contains(&t) {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub(crate) fn rotate_safety(dir: &Path) {
    for (_, p) in listed(dir, "safety").into_iter().skip(SAFETY_KEPT) {
        let _ = std::fs::remove_file(p);
    }
}

/// Whether an automatic snapshot is due: none in the last day, and the data changed since the
/// last one.
fn due(dir: &Path, now: u64, fingerprint: &str, last_fingerprint: &str) -> bool {
    let recent = listed(dir, "auto").first().is_some_and(|(t, _)| now.saturating_sub(*t) < DAY);
    !recent && fingerprint != last_fingerprint
}

/// At startup (on a background thread of its own, low priority): a snapshot when one is due,
/// copied to the second folder if there is one. Shown in the sidebar while it runs.
pub fn snapshot_if_due(ctx: &Ctx) -> Option<PathBuf> {
    let state = &ctx.state;
    let dir = backups_dir(&state.data_dir);
    let mut cfg = config(&state.data_dir);
    let fp = fingerprint(&sources(&state.data_dir));
    if !due(&dir, crate::history::now(), &fp, &cfg.fingerprint) {
        return None;
    }
    RUNNING.store(true, std::sync::atomic::Ordering::SeqCst);
    ctx.emit("backup-progress", serde_json::json!({ "running": true }));
    let made = snapshot(state, "auto");
    let out = match made {
        Ok(path) => {
            cfg.fingerprint = fp;
            rotate(&dir);
            if !cfg.folder.is_empty() {
                cfg.folder_error = copy_out(&path, Path::new(&cfg.folder)).err().unwrap_or_default();
            }
            save_config(&state.data_dir, &cfg);
            log::info!("backup: snapshot {}", path.display());
            Some(path)
        }
        Err(e) => {
            log::warn!("backup: no snapshot ({e})");
            None
        }
    };
    RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    ctx.emit("backup-progress", serde_json::json!({ "running": false }));
    out
}

/// An automatic snapshot is being made now (the window may open after its start was announced).
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Copies a snapshot into the second folder and rotates the snapshots there the same way.
fn copy_out(snapshot: &Path, folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("Can't reach {}: {e}", folder.display()))?;
    let name = snapshot.file_name().ok_or("no name")?;
    let bytes = std::fs::read(snapshot).map_err(io)?;
    crate::state::write_file(&folder.join(name), &bytes, false).map_err(|e| format!("Can't copy to {}: {e}", folder.display()))?;
    rotate(folder);
    Ok(())
}

// ---------- what the Settings page shows ----------

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInfo {
    pub path: String,
    pub name: String,
    /// "auto", "safety" or "manual".
    pub kind: String,
    pub created: u64,
    pub size: u64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// The newest automatic snapshot (unix seconds).
    pub last_auto: Option<u64>,
    /// Snapshots in `backups/` (and, on Android, backups in the export folder), newest first.
    pub snapshots: Vec<SnapshotInfo>,
    /// The second folder automatic snapshots are copied to ("" for none).
    pub folder: String,
    pub folder_error: String,
    /// Android: where "Back up now" saves (there's no save-file picker that can be written to).
    pub export_dir: String,
    /// The snapshots' folder.
    pub dir: String,
    /// An automatic snapshot is being made right now.
    pub running: bool,
}

pub fn info(data_dir: &Path) -> BackupInfo {
    let dir = backups_dir(data_dir);
    let cfg = config(data_dir);
    let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
    let mut snapshots: Vec<SnapshotInfo> = ["auto", "safety"]
        .iter()
        .flat_map(|k| listed(&dir, k).into_iter().map(move |(t, p)| (k, t, p)))
        .map(|(k, t, p)| SnapshotInfo { name: p.file_name().unwrap_or_default().to_string_lossy().into_owned(), size: size(&p), path: p.to_string_lossy().into_owned(), kind: k.to_string(), created: t })
        .collect();
    let export = export_dir(data_dir);
    if let Some(ex) = &export {
        for p in std::fs::read_dir(ex).into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())) {
            if p.extension().is_some_and(|x| x == "zip") {
                let created = std::fs::metadata(&p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                snapshots.push(SnapshotInfo { name: p.file_name().unwrap_or_default().to_string_lossy().into_owned(), size: size(&p), path: p.to_string_lossy().into_owned(), kind: "manual".into(), created });
            }
        }
    }
    snapshots.sort_by(|a, b| b.created.cmp(&a.created));
    BackupInfo {
        last_auto: listed(&dir, "auto").first().map(|(t, _)| *t),
        snapshots,
        folder: cfg.folder,
        folder_error: cfg.folder_error,
        export_dir: export.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        dir: dir.to_string_lossy().into_owned(),
        running: RUNNING.load(std::sync::atomic::Ordering::SeqCst),
    }
}

/// Android: a folder of the shared storage the listener can reach with a file manager or a computer
/// (the dialog's save picker hands back a content address the app can't write to). Elsewhere none:
/// the save picker gives a real path.
pub(crate) fn export_dir(_data_dir: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    {
        Some(PathBuf::from("/storage/emulated/0/Documents/Cadence"))
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// "Back up now": to `path` (the save picker's), or on Android into the export folder (falling back
/// to `backups/` when the shared storage can't be written).
pub fn create_now(state: &AppState, path: Option<String>, lyric_offsets: Option<String>) -> Result<Made, String> {
    if let Some(o) = lyric_offsets.filter(|o| !o.trim().is_empty()) {
        keep_offsets(&state.data_dir, &o)?;
    }
    flush(state);
    let links = library_links(state);
    let lookup = |id: &str| links.get(id).cloned();
    let day = {
        let (y, m, d) = crate::stats::civil((crate::history::now() as i64 + crate::stats::local_offset()).div_euclid(DAY as i64));
        format!("{y}-{m:02}-{d:02}")
    };
    match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            let mut out = PathBuf::from(p);
            if out.extension().is_none() {
                out.set_extension("zip");
            }
            create(&state.data_dir, &out, "manual", &lookup)
        }
        None => {
            let name = format!("Cadence backup {day}.zip");
            if let Some(ex) = export_dir(&state.data_dir) {
                match create(&state.data_dir, &ex.join(&name), "manual", &lookup) {
                    Ok(mut m) => {
                        m.note = format!("Saved in {} (Documents › Cadence in a file manager).", ex.display());
                        return Ok(m);
                    }
                    Err(e) => log::warn!("backup: can't write to {}: {e}", ex.display()),
                }
                let mut m = create(&state.data_dir, &backups_dir(&state.data_dir).join(format!("manual-{}.zip", crate::history::now())), "manual", &lookup)?;
                m.note = "The shared Documents folder can't be written, so it was saved in Cadence's own storage; it shows in the list below.".into();
                return Ok(m);
            }
            Err("Choose where to save the backup.".into())
        }
    }
}

/// Keeps the window's lyrics timing nudges in the data folder, so backups (automatic ones too)
/// have them.
pub fn keep_offsets(data_dir: &Path, json: &str) -> Result<(), String> {
    let parsed: HashMap<String, f64> = serde_json::from_str(json).map_err(|e| format!("Unexpected timing adjustments: {e}"))?;
    let path = data_dir.join(OFFSETS);
    let current: Option<HashMap<String, f64>> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if current.as_ref() == Some(&parsed) || (current.is_none() && parsed.is_empty()) {
        return Ok(());
    }
    crate::state::write_file(&path, &serde_json::to_vec(&parsed).map_err(io)?, true).map_err(io)
}

/// Sets (or with "" turns off) the second folder; a snapshot is copied there at once.
pub fn set_folder(data_dir: &Path, folder: Option<String>) -> Result<BackupInfo, String> {
    let folder = folder.map(|f| f.trim().to_string()).unwrap_or_default();
    let mut cfg = config(data_dir);
    cfg.folder_error.clear();
    if !folder.is_empty() {
        let p = Path::new(&folder);
        if !p.is_dir() {
            return Err(format!("{folder} isn't a folder Cadence can reach."));
        }
        if p.starts_with(data_dir) {
            return Err("Choose a folder outside Cadence's own data.".into());
        }
        if let Some((_, latest)) = listed(&backups_dir(data_dir), "auto").first() {
            cfg.folder_error = copy_out(latest, p).err().unwrap_or_default();
        }
    }
    cfg.folder = folder;
    save_config(data_dir, &cfg);
    Ok(info(data_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::PlayEvent;

    struct Quiet;
    impl crate::ctx::Host for Quiet {
        fn emit(&self, _: &str, _: serde_json::Value) {}
    }

    fn base(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("cadence-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// The app's core on a data folder of its own (no window, no device until a song plays).
    fn core(base: &Path) -> Ctx {
        crate::hub::open(base.join("data"), &base.join("local"), std::sync::Arc::new(Quiet)).0
    }

    fn song(id: &str, title: &str, artist: &str, album: &str, duration: f64, path: &str) -> Track {
        Track { id: id.into(), title: title.into(), artist: artist.into(), album: album.into(), album_id: format!("al-{album}"), duration, path: path.into(), ..Default::default() }
    }

    fn play(id: &str, at: u64) -> PlayEvent {
        PlayEvent { track_id: id.into(), album_id: "a".into(), at, listened: 200.0, duration: 200.0, skipped: false }
    }

    /// A data folder with something of everything.
    fn fill(dir: &Path) {
        std::fs::create_dir_all(dir.join("history-archive")).unwrap();
        std::fs::create_dir_all(dir.join("lyrics")).unwrap();
        std::fs::create_dir_all(dir.join("art").join("artists")).unwrap();
        let w = |n: &str, s: &str| std::fs::write(dir.join(n), s).unwrap();
        w("settings.json", r#"{"folders":[],"volume":0.5,"shuffle":true,"skipped":["s1"],"listenbrainzUser":""}"#);
        w("playlists.json", r#"{"items":[{"id":"p1","name":"Mine","trackIds":["s1","s2"],"smart":null,"created":1,"updated":2},{"id":"p2","name":"Smart","trackIds":[],"smart":{"match":"all","rules":[]},"created":1,"updated":2}]}"#);
        w("marks.json", r#"{"items":{"s1":{"rating":5,"loved":true,"title":"Lemon","artist":"Kenshi Yonezu","album":"Lemon","duration":255.0}}}"#);
        w("history.json", &serde_json::to_string(&crate::history::History { events: vec![play("s1", 1_800_000_000), play("s2", 1_800_000_500)] }).unwrap());
        w("session.json", r#"{"queue":["s1","s2"],"index":0,"position":12.5}"#);
        w("attention.json", r#"{"ignored":["copies|s1"],"hidden":["year"]}"#);
        w("artist-names.json", r#"{"latin":{},"different":[["A","B"]]}"#);
        let pic = dir.join("art").join("artists").join("abc-1.jpg");
        std::fs::write(&pic, [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
        let small = dir.join("art").join("artists").join("abc-1_s.jpg");
        std::fs::write(&small, [0xFF, 0xD8, 0xFF, 0xE0, 4]).unwrap();
        w("artist-images.json", &serde_json::json!({"kenshi yonezu": {"large": pic.to_string_lossy(), "small": small.to_string_lossy()}}).to_string());
        w(OFFSETS, r#"{"s1":0.5,"s2:voice":-0.25}"#);
        w("history-archive/2025.jsonl", &format!("{}\n{}\n", serde_json::to_string(&play("s1", 1_750_000_000)).unwrap(), serde_json::to_string(&play("s3", 1_750_000_100)).unwrap()));
        w("lyrics/s1.json", r#"{"lyrics":null,"checked":1,"picked":true,"v":1,"hidden":true}"#);
        w("lyrics/s2.json", r#"{"lyrics":null,"checked":1,"picked":false,"v":1}"#);
        w("lyrics/s1.voice.json", r#"{}"#);
        // Not user data: never in a backup.
        w("secrets.json", r#"{"ai-key":"sk-very-secret"}"#);
        w("library.json", r#"{"tracks":[]}"#);
        w("sonic.json", r#"{"tracks":{}}"#);
        w("health.json", r#"{"tracks":{}}"#);
        std::fs::write(dir.join("art").join("al1.jpg"), [0xFF, 0xD8, 0xFF]).unwrap();
    }

    fn names(path: &Path) -> Vec<String> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        (0..z.len()).map(|i| z.by_index(i).unwrap().name().unwrap().into_owned()).collect()
    }

    fn no_links(_: &str) -> Option<Link> {
        None
    }

    #[test]
    fn a_backup_restores_every_file_byte_for_byte() {
        let b = base("roundtrip");
        let data = b.join("data");
        fill(&data);
        let included = [
            "settings.json", "playlists.json", "marks.json", "history.json", "session.json", "attention.json", "artist-names.json",
            "artist-images.json", OFFSETS, "history-archive/2025.jsonl", "lyrics/s1.json", "art/artists/abc-1.jpg", "art/artists/abc-1_s.jpg",
        ];
        let before: HashMap<&str, Vec<u8>> = included.iter().map(|n| (*n, std::fs::read(data.join(n)).unwrap())).collect();
        let out = b.join("out").join("mine.zip");
        let made = create(&data, &out, "manual", &no_links).unwrap();
        assert_eq!(made.files, included.len(), "{:?}", names(&out));
        let ctx = core(&b);
        // Everything changes (and a year the backup doesn't have appears) …
        for n in ["playlists.json", "marks.json", "attention.json", OFFSETS, "history-archive/2025.jsonl", "lyrics/s1.json", "art/artists/abc-1.jpg"] {
            std::fs::write(data.join(n), b"{}").unwrap();
        }
        std::fs::write(data.join("history-archive/2026.jsonl"), "").unwrap();
        *ctx.state.marks.lock() = Default::default();
        // … and the restore puts it all back as it was.
        let r = restore(&ctx, &out).unwrap();
        assert_eq!(r.files, included.len());
        for n in included {
            assert_eq!(std::fs::read(data.join(n)).unwrap(), before[n], "{n}");
        }
        assert!(!data.join("history-archive/2026.jsonl").exists());
        // What Cadence holds in memory is the restored data too.
        assert_eq!(ctx.state.marks.lock().view()["s1"].rating, 5);
        assert_eq!(ctx.state.playlists.lock().items.len(), 2);
        assert!(ctx.state.settings.lock().shuffle);
        assert_eq!(r.lyric_offsets.as_deref().map(|s| s.contains("s2:voice")), Some(true));
        // The safety snapshot of the data as it was just before.
        assert!(Path::new(&r.safety).exists());
        let safety = open(Path::new(&r.safety)).unwrap();
        assert_eq!(safety.manifest.kind, "safety");
        assert!(safety.files.iter().any(|(n, _, b)| n == "attention.json" && b == b"{}"));
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn secrets_and_rebuildable_files_stay_out() {
        let b = base("secrets");
        let data = b.join("data");
        fill(&data);
        // A secret that slipped into settings is taken out too.
        std::fs::write(data.join("settings.json"), r#"{"volume":0.5,"ai":{"apiKey":"sk-very-secret","cloudUrl":"x"},"listenbrainzToken":"tok-secret"}"#).unwrap();
        // The seek bar's waveforms: a cache, drawn again from the music.
        std::fs::create_dir_all(data.join("waveforms")).unwrap();
        std::fs::write(data.join("waveforms").join("s1.wf"), b"CDWF").unwrap();
        let out = b.join("b.zip");
        create(&data, &out, "manual", &no_links).unwrap();
        let n = names(&out);
        for left_out in ["secrets.json", "library.json", "sonic.json", "health.json", "art/al1.jpg", "lyrics/s2.json", "lyrics/s1.voice.json", "waveforms/s1.wf"] {
            assert!(!n.iter().any(|x| x == left_out), "{left_out} in {n:?}");
        }
        let raw = std::fs::read(&out).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut all = String::new();
        for i in 0..z.len() {
            let mut s = String::new();
            let _ = z.by_index(i).unwrap().read_to_string(&mut s);
            all.push_str(&s);
        }
        assert!(!all.contains("very-secret") && !all.contains("tok-secret"), "{all}");
        assert!(!raw.windows(11).any(|w| w == b"very-secret"));
        assert!(all.contains("\"cloudUrl\""), "the rest of the settings stays");
        let _ = std::fs::remove_dir_all(&b);
    }

    /// Writes a zip of (name, bytes) and a manifest listing them.
    fn forge(path: &Path, entries: &[(&str, &[u8])], manifest: Option<Manifest>) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        for (n, b) in entries {
            z.start_file(*n, o).unwrap();
            z.write_all(b).unwrap();
        }
        let m = manifest.unwrap_or_else(|| Manifest {
            format: FORMAT,
            app: env!("CARGO_PKG_VERSION").into(),
            created: 1,
            kind: "manual".into(),
            files: entries.iter().map(|(n, b)| FileEntry { name: n.to_string(), size: b.len() as u64, hash: hex(b) }).collect(),
            fingerprint: String::new(),
        });
        z.start_file(MANIFEST, o).unwrap();
        z.write_all(&serde_json::to_vec(&m).unwrap()).unwrap();
        z.finish().unwrap();
    }

    /// Every file of the data folder, with its bytes.
    fn everything(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).map(|e| (e.path().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect()
    }

    #[test]
    fn bad_archives_are_refused_without_touching_anything() {
        let b = base("refused");
        let ctx = core(&b);
        let data = b.join("data");
        fill(&data);
        let before = everything(&data);
        let good_marks: &[u8] = br#"{"items":{}}"#;
        let cases: Vec<(&str, Box<dyn Fn(&Path)>)> = vec![
            ("zip-slip", Box::new(|p: &Path| forge(p, &[("../evil.json", b"{}")], None))),
            ("zip-slip-nested", Box::new(|p: &Path| forge(p, &[("lyrics/../../evil.json", b"{}")], None))),
            ("absolute", Box::new(|p: &Path| forge(p, &[("/etc/passwd", b"x")], None))),
            ("drive", Box::new(|p: &Path| forge(p, &[("C:/Windows/evil.json", b"{}")], None))),
            ("backslash", Box::new(|p: &Path| forge(p, &[("lyrics\\..\\..\\evil.json", b"{}")], None))),
            ("unknown", Box::new(|p: &Path| forge(p, &[("library.json", b"{}")], None))),
            ("garbage", Box::new(|p: &Path| std::fs::write(p, b"this is not a zip at all").unwrap())),
            ("no-manifest", Box::new(|p: &Path| {
                let mut z = zip::ZipWriter::new(std::fs::File::create(p).unwrap());
                z.start_file("marks.json", zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(b"{}").unwrap();
                z.finish().unwrap();
            })),
            ("damaged-json", Box::new(move |p: &Path| forge(p, &[("marks.json", good_marks), ("playlists.json", b"{\"items\": [")], None))),
            ("wrong-shape", Box::new(|p: &Path| forge(p, &[("playlists.json", b"[1,2,3]")], None))),
            ("hash-mismatch", Box::new(|p: &Path| {
                let mut m = Manifest { format: FORMAT, app: env!("CARGO_PKG_VERSION").into(), created: 1, kind: String::new(), files: vec![], fingerprint: String::new() };
                m.files.push(FileEntry { name: "marks.json".into(), size: 12, hash: hex(b"something else") });
                forge(p, &[("marks.json", br#"{"items":{}}"#)], Some(m));
            })),
            ("unlisted", Box::new(|p: &Path| {
                let m = Manifest { format: FORMAT, app: env!("CARGO_PKG_VERSION").into(), created: 1, kind: String::new(), files: vec![], fingerprint: String::new() };
                forge(p, &[("marks.json", br#"{"items":{}}"#)], Some(m));
            })),
            ("newer", Box::new(|p: &Path| {
                let m = Manifest { format: FORMAT, app: "999.0.0".into(), created: 1, kind: String::new(), files: vec![], fingerprint: String::new() };
                forge(p, &[], Some(m));
            })),
            ("newer-format", Box::new(|p: &Path| {
                let m = Manifest { format: FORMAT + 1, app: "0.0.1".into(), created: 1, kind: String::new(), files: vec![], fingerprint: String::new() };
                forge(p, &[], Some(m));
            })),
            ("not-a-picture", Box::new(|p: &Path| forge(p, &[("art/artists/x.jpg", b"MZ\x90\x00 an exe")], None))),
            ("oversized", Box::new(|p: &Path| {
                // A zip bomb: a tiny archive that unpacks past the limit for one file.
                let big = vec![b' '; (MAX_PICTURE + 10) as usize];
                let mut z = zip::ZipWriter::new(std::fs::File::create(p).unwrap());
                z.start_file("art/artists/x.jpg", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
                z.write_all(&big).unwrap();
                z.start_file(MANIFEST, zip::write::SimpleFileOptions::default()).unwrap();
                let m = Manifest { format: FORMAT, app: "0.0.1".into(), created: 1, kind: String::new(), files: vec![FileEntry { name: "art/artists/x.jpg".into(), size: big.len() as u64, hash: hex(&big) }], fingerprint: String::new() };
                z.write_all(&serde_json::to_vec(&m).unwrap()).unwrap();
                z.finish().unwrap();
            })),
        ];
        for (name, make) in cases {
            let p = b.join(format!("{name}.zip"));
            make(&p);
            let err = restore(&ctx, &p).expect_err(name);
            assert!(!err.is_empty(), "{name}");
            if name == "newer" {
                assert!(err.contains("newer"), "{err}");
            }
            assert_eq!(everything(&data), before, "{name} touched the data");
            assert!(!b.join("evil.json").exists() && !data.join("evil.json").exists());
        }
        assert!(listed(&backups_dir(&data), "safety").is_empty(), "refused before any snapshot");
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn older_formats_read_and_a_damaged_line_or_two_is_fine() {
        let b = base("older");
        let p = b.join("old.zip");
        let year = format!("{}\n{{\"trackId\": \"x\n{}\n", serde_json::to_string(&play("a", 1)).unwrap(), serde_json::to_string(&play("b", 2)).unwrap());
        let m = Manifest {
            format: 0,
            app: "0.1.0".into(),
            created: 1,
            kind: String::new(),
            files: vec![
                FileEntry { name: "history.json".into(), size: 12, hash: hex(br#"{"events":[]}"#) },
                FileEntry { name: "history-archive/2024.jsonl".into(), size: year.len() as u64, hash: hex(year.as_bytes()) },
            ],
            fingerprint: String::new(),
        };
        let mut m2 = m.clone();
        m2.files[0].size = 13;
        forge(&p, &[("history.json", br#"{"events":[]}"#), ("history-archive/2024.jsonl", year.as_bytes())], Some(m2));
        let a = open(&p).unwrap();
        assert_eq!(a.files.len(), 2);
        assert!(describe(&a).iter().any(|l| l.contains("2 plays")), "{:?}", describe(&a));
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn snapshots_keep_seven_days_and_four_weeks() {
        let now = 1_800_000_000u64;
        // A snapshot every day for 60 days, two on some days.
        let mut times: Vec<u64> = (0..60).map(|d| now - d * DAY).collect();
        times.push(now - 3 * DAY + 3600);
        let keep = kept(&times, 0);
        assert_eq!(keep.len(), DAILY + WEEKLY, "{keep:?}");
        // One for each of the last seven calendar days.
        let today = now / DAY;
        let days: HashSet<u64> = keep.iter().map(|t| t / DAY).filter(|d| today - d < 7).collect();
        assert_eq!(days.len(), 7, "{days:?}");
        assert!(keep.contains(&(now - 3 * DAY + 3600)), "the newest of a day");
        assert!(keep.iter().all(|t| now - t < 7 * DAY + 5 * 7 * DAY), "weekly ones are the four weeks before");
        // Files: rotate removes the rest.
        let b = base("rotate");
        for t in &times {
            std::fs::write(b.join(format!("auto-{t}.zip")), b"x").unwrap();
        }
        std::fs::write(b.join("safety-1.zip"), b"x").unwrap();
        rotate(&b);
        assert_eq!(listed(&b, "auto").len(), DAILY + WEEKLY);
        assert_eq!(listed(&b, "safety").len(), 1, "only automatic ones rotate");
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn a_snapshot_is_skipped_when_nothing_changed() {
        let b = base("due");
        let ctx = core(&b);
        let data = b.join("data");
        fill(&data);
        let first = snapshot_if_due(&ctx).expect("first snapshot");
        assert!(first.exists());
        // Within a day: not again.
        assert!(snapshot_if_due(&ctx).is_none());
        // A day later, nothing changed: still not.
        let dir = backups_dir(&data);
        let old = dir.join(format!("auto-{}.zip", crate::history::now() - 2 * DAY));
        std::fs::rename(&first, &old).unwrap();
        assert!(snapshot_if_due(&ctx).is_none(), "nothing changed");
        // Something changed: a new one.
        std::fs::write(data.join("attention.json"), r#"{"ignored":["x"],"hidden":[]}"#).unwrap();
        assert!(snapshot_if_due(&ctx).is_some());
        assert_eq!(listed(&dir, "auto").len(), 2);
        // A second folder gets a copy.
        let second = b.join("second");
        std::fs::create_dir_all(&second).unwrap();
        let info = set_folder(&data, Some(second.to_string_lossy().into_owned())).unwrap();
        assert_eq!(info.folder, second.to_string_lossy());
        assert_eq!(listed(&second, "auto").len(), 1);
        assert!(set_folder(&data, Some(data.join("backups").to_string_lossy().into_owned())).is_err());
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn restored_songs_are_found_again_where_it_is_certain() {
        let b = base("relink");
        let ctx = core(&b);
        let data = b.join("data");
        std::fs::create_dir_all(&data).unwrap();
        // The backup's library: four songs under the old music folder.
        let old = [
            song("o-lemon", "Lemon", "Kenshi Yonezu", "Lemon", 255.0, "Z:/Old/Lemon.flac"),
            song("o-intro", "Intro", "YOASOBI", "THE BOOK", 90.0, "Z:/Old/Intro.flac"),
            song("o-gone", "Gone Song", "Nobody", "Nowhere", 100.0, "Z:/Old/Gone.flac"),
            song("o-heard", "Heard", "Someone", "Played", 180.0, "Z:/Old/Heard.flac"),
        ];
        let links: HashMap<String, Link> = old.iter().map(|t| (t.id.clone(), Link::of(t))).collect();
        std::fs::write(data.join("marks.json"), r#"{"items":{"o-lemon":{"rating":5,"loved":true,"title":"Lemon","artist":"Kenshi Yonezu","album":"Lemon","duration":255.0}}}"#).unwrap();
        std::fs::write(data.join("playlists.json"), r#"{"items":[{"id":"p","name":"P","trackIds":["o-lemon","o-intro","o-gone"],"created":1,"updated":1}]}"#).unwrap();
        std::fs::write(data.join("history.json"), serde_json::to_string(&crate::history::History { events: vec![play("o-heard", 10), play("o-lemon", 20)] }).unwrap()).unwrap();
        std::fs::create_dir_all(data.join("history-archive")).unwrap();
        std::fs::write(data.join("history-archive/1970.jsonl"), format!("{}\n", serde_json::to_string(&play("o-lemon", 5)).unwrap())).unwrap();
        std::fs::write(data.join(OFFSETS), r#"{"o-lemon":0.5,"o-lemon:voice":1.0}"#).unwrap();
        let out = b.join("b.zip");
        create(&data, &out, "manual", &|id| links.get(id).cloned()).unwrap();
        // The library now: the same songs at new paths (new ids); Intro twice on other albums.
        {
            let now = vec![
                song("n-lemon", "Lemon", "Kenshi Yonezu", "Lemon", 255.3, "Y:/New/Lemon.flac"),
                song("n-intro2", "Intro", "YOASOBI", "THE BOOK 2", 90.1, "Y:/New/Intro2.flac"),
                song("n-intro3", "Intro", "YOASOBI", "THE BOOK 3", 90.4, "Y:/New/Intro3.flac"),
                song("n-heard", "Heard", "Someone", "Played", 180.5, "Y:/New/Heard.flac"),
            ];
            let albums = Vec::new();
            ctx.state.library.lock().set_tracks(now, albums);
        }
        let r = restore(&ctx, &out).unwrap();
        assert_eq!(r.relinked, 2, "{r:?}");
        assert_eq!(r.unresolved, 2, "the ambiguous Intro and the song that's gone: {r:?}");
        assert_eq!(r.unresolved_history, 0);
        let p = ctx.state.playlists.lock().items[0].track_ids.clone();
        assert_eq!(p, ["n-lemon", "o-intro", "o-gone"], "ambiguous and missing ones are kept, not dropped");
        assert_eq!(ctx.state.marks.lock().view()["n-lemon"].rating, 5);
        let h: Vec<String> = ctx.state.history.lock().events.iter().map(|e| e.track_id.clone()).collect();
        assert_eq!(h, ["n-heard", "n-lemon"]);
        assert!(std::fs::read_to_string(data.join("history-archive/1970.jsonl")).unwrap().contains("n-lemon"));
        let o: HashMap<String, f64> = serde_json::from_slice(&std::fs::read(data.join(OFFSETS)).unwrap()).unwrap();
        assert_eq!(o.get("n-lemon:voice"), Some(&1.0));
        // Saved too.
        assert!(std::fs::read_to_string(data.join("playlists.json")).unwrap().contains("n-lemon"));
        // The two not found are tried again after a rescan; the Intro's album comes back.
        assert!(data.join(PENDING).exists());
        {
            let mut lib = ctx.state.library.lock();
            let mut tracks = lib.snapshot.tracks.clone();
            tracks.push(song("n-intro", "Intro", "YOASOBI", "THE BOOK", 90.0, "Y:/New/Intro.flac"));
            lib.set_tracks(tracks, Vec::new());
        }
        retry_pending(&ctx);
        assert_eq!(ctx.state.playlists.lock().items[0].track_ids, ["n-lemon", "n-intro", "o-gone"]);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn names_that_leave_the_data_folder_are_not_ours() {
        for bad in ["../x.json", "a/../../x", "/abs", "C:/x", "lyrics\\x.json", "lyrics/.hidden.json", "lyrics/a.voice.json", "history-archive/../settings.json", "art/artists/x.exe", "", "./settings.json", "history-archive/20x5.jsonl"] {
            assert!(kind(bad).is_none(), "{bad}");
        }
        assert_eq!(kind("settings.json"), Some(Kind::Whole));
        assert_eq!(kind("history-archive/2025.jsonl"), Some(Kind::Archive));
        assert_eq!(kind("lyrics/0123abcd.json"), Some(Kind::Lyrics));
        assert_eq!(kind("art/artists/ab12-1700.jpg"), Some(Kind::Picture));
        assert!(version("0.2.40") < version("0.2.41") && version("0.10.0") > version("0.9.9"));
    }

    #[test]
    fn decisions_are_backed_up_and_rebuildable_caches_are_not() {
        assert!(WHOLE.contains(&"artist-names.json"));
        assert!(!WHOLE.contains(&"musicbrainz-artists.json"));
        assert!(!WHOLE.contains(&"search-readings.json"));
    }

}
