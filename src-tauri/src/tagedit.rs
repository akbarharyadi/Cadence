//! Editing song details (tags) in the files themselves.
//!
//! A song that's playing can't have its file rewritten (the decoder is reading it, and new tags
//! can move the audio inside the file). Its changes show in the library at once and are *held*:
//! written when the song stops, or at the next start if Cadence closes first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{Tag, items::Timestamp};
use serde::{Deserialize, Serialize};

use crate::library::Track;

/// Fields to change. `None` leaves a field as it is; an empty string removes it.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TagChanges {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<String>,
    pub disc_no: Option<String>,
}

impl TagChanges {
    /// Puts later changes on top of these.
    fn merge(&mut self, later: &TagChanges) {
        for (mine, newer) in [
            (&mut self.title, &later.title),
            (&mut self.artist, &later.artist),
            (&mut self.album, &later.album),
            (&mut self.album_artist, &later.album_artist),
            (&mut self.genre, &later.genre),
            (&mut self.year, &later.year),
            (&mut self.track_no, &later.track_no),
            (&mut self.disc_no, &later.disc_no),
        ] {
            if newer.is_some() {
                mine.clone_from(newer);
            }
        }
    }
}

/// `None`: unchanged; `Some(None)`: removed; `Some(Some(n))`: set.
fn number(v: &Option<String>) -> Result<Option<Option<u32>>, String> {
    match v.as_deref().map(str::trim) {
        None => Ok(None),
        Some("") => Ok(Some(None)),
        Some(s) => s
            .parse::<u32>()
            .map(|n| Some(Some(n)))
            .map_err(|_| format!("“{s}” isn’t a number")),
    }
}

/// Fails on numbers that can't be written (before anything is written or held).
pub fn check(c: &TagChanges) -> Result<(), String> {
    number(&c.year)?;
    number(&c.track_no)?;
    number(&c.disc_no)?;
    Ok(())
}

pub fn write(path: &Path, c: &TagChanges) -> Result<(), String> {
    let mut tagged = lofty::read_from_path(path)
        .map_err(|e| format!("Couldn't read {}: {e}", file_name(path)))?;
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tagged.primary_tag_type()));
    }
    let tag = tagged.primary_tag_mut().expect("tag was just inserted");

    let text = |tag: &mut Tag, key: ItemKey, v: &Option<String>| {
        if let Some(v) = v {
            let v = v.trim();
            if v.is_empty() {
                tag.remove_key(key);
            } else {
                tag.insert_text(key, v.to_string());
            }
        }
    };
    text(tag, ItemKey::TrackTitle, &c.title);
    text(tag, ItemKey::TrackArtist, &c.artist);
    text(tag, ItemKey::AlbumTitle, &c.album);
    text(tag, ItemKey::AlbumArtist, &c.album_artist);
    text(tag, ItemKey::Genre, &c.genre);

    match number(&c.year)? {
        Some(Some(y)) => tag.set_date(Timestamp {
            year: y as u16,
            ..Default::default()
        }),
        Some(None) => tag.remove_date(),
        None => {}
    }
    match number(&c.track_no)? {
        Some(Some(n)) => tag.set_track(n),
        Some(None) => tag.remove_track(),
        None => {}
    }
    match number(&c.disc_no)? {
        Some(Some(n)) => tag.set_disk(n),
        Some(None) => tag.remove_disk(),
        None => {}
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("Couldn't save {}: {e}", file_name(path)))
}

/// Shows written changes in the library at once (song id → changes), with the albums rebuilt once:
/// the rescan that reads them back can take a while in a big library, and until then the change
/// would seem not to have happened. Saves the library; the caller says "library-changed".
pub fn show_now(state: &crate::state::AppState, changes: &[(String, TagChanges)]) {
    if changes.is_empty() {
        return;
    }
    {
        let mut lib = state.library.lock();
        let mut tracks = lib.snapshot.tracks.clone();
        for (id, c) in changes {
            if let Some(&i) = lib.by_id.get(id) {
                show(&mut tracks[i], c);
            }
        }
        let albums = crate::library::build_albums(&mut tracks, &state.art_dir());
        lib.set_tracks(tracks, albums);
    }
    state.save_library();
}

/// Puts the changes into a song's library entry the way a rescan would read them from the file
/// (an emptied artist is "Unknown Artist", and so on). The album id follows; rebuild the albums.
pub fn show(t: &mut Track, c: &TagChanges) {
    let path = PathBuf::from(&t.path);
    let given = |v: &Option<String>| v.as_deref().map(|s| s.trim().to_string());
    if let Some(v) = given(&c.title) {
        t.title = if v.is_empty() {
            crate::library::title_from_file(&path)
        } else {
            v
        };
    }
    if let Some(v) = given(&c.artist) {
        t.artist = if v.is_empty() {
            "Unknown Artist".into()
        } else {
            v
        };
    }
    if let Some(v) = given(&c.album) {
        t.album = if v.is_empty() {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            v
        };
    }
    if let Some(v) = given(&c.album_artist) {
        t.album_artist = if v.is_empty() { t.artist.clone() } else { v };
    }
    if let Some(v) = given(&c.genre) {
        t.genre = v;
    }
    if let Ok(Some(y)) = number(&c.year) {
        t.year = y;
    }
    if let Ok(Some(n)) = number(&c.track_no) {
        t.track_no = n;
    }
    if let Ok(Some(n)) = number(&c.disc_no) {
        t.disc_no = n;
    }
    t.album_id = crate::library::album_id_for(&t.album_artist, &t.album);
}

// ---------- changes held for a playing song ----------

/// File path → changes not yet written (kept in held_tags.json so a restart doesn't lose them).
static HELD: Mutex<Option<HashMap<String, TagChanges>>> = Mutex::new(None);

fn held_file(dir: &Path) -> PathBuf {
    dir.join("held_tags.json")
}

fn with_held<R>(dir: &Path, f: impl FnOnce(&mut HashMap<String, TagChanges>) -> R) -> R {
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let map = held.get_or_insert_with(|| {
        std::fs::read(held_file(dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    });
    f(map)
}

fn save_held(dir: &Path, map: &HashMap<String, TagChanges>) {
    let file = held_file(dir);
    if map.is_empty() {
        let _ = std::fs::remove_file(file);
    } else if let Ok(b) = serde_json::to_vec(map) {
        let _ = std::fs::write(file, b);
    }
}

/// Holds a playing song's changes until `write_held` (on top of any held already).
pub fn hold(dir: &Path, path: &str, c: &TagChanges) {
    with_held(dir, |map| {
        map.entry(path.to_string()).or_default().merge(c);
        save_held(dir, map);
    });
}

/// Writes a file's held changes (its song stopped). False when nothing was held.
pub fn write_held(dir: &Path, path: &str) -> Result<bool, String> {
    let held = with_held(dir, |map| {
        let c = map.remove(path);
        if c.is_some() {
            save_held(dir, map);
        }
        c
    });
    match held {
        Some(c) => write(Path::new(path), &c).map(|()| true),
        None => Ok(false),
    }
}

/// At start, before anything plays: writes changes held when Cadence closed during their song.
pub fn write_leftovers(dir: &Path) {
    with_held(dir, |map| {
        if map.is_empty() {
            return;
        }
        map.retain(|path, c| {
            let p = Path::new(path);
            // On a drive that isn't plugged in right now: try again next time.
            if !p.exists() {
                return true;
            }
            match write(p, c) {
                Ok(()) => log::info!("wrote the held song info into {path}"),
                Err(e) => log::warn!("{e}"),
            }
            false
        });
        save_held(dir, map);
    });
}

/// A rescan reads the old tags from files whose changes are held: show the changes instead.
pub fn show_held(dir: &Path, tracks: &mut [Track]) {
    with_held(dir, |map| {
        if map.is_empty() {
            return;
        }
        for t in tracks.iter_mut() {
            if let Some(c) = map.get(&t.path) {
                show(t, c);
            }
        }
    });
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn later_changes_win_and_untouched_fields_stay() {
        let mut held = TagChanges {
            title: some("A"),
            year: some("2013"),
            ..Default::default()
        };
        held.merge(&TagChanges {
            title: some("B"),
            artist: some("SPYAIR"),
            ..Default::default()
        });
        assert_eq!(held.title.as_deref(), Some("B"));
        assert_eq!(held.artist.as_deref(), Some("SPYAIR"));
        assert_eq!(held.year.as_deref(), Some("2013"));
        assert_eq!(held.album, None);
    }

    #[test]
    fn shown_like_a_rescan_reads_it() {
        let mut t = Track {
            path: r"D:\Music\YouTube\SPYAIR - Sakuramitsutsuki.m4a".into(),
            title: "SPYAIR - Sakuramitsutsuki".into(),
            artist: "Unknown Artist".into(),
            album: "YouTube".into(),
            album_artist: "Unknown Artist".into(),
            ..Default::default()
        };
        show(
            &mut t,
            &TagChanges {
                title: some(" サクラミツツキ "),
                artist: some("SPYAIR"),
                album_artist: some(""),
                year: some("2013"),
                track_no: some(""),
                ..Default::default()
            },
        );
        assert_eq!(t.title, "サクラミツツキ");
        assert_eq!(t.album_artist, "SPYAIR");
        assert_eq!(t.album, "YouTube");
        assert_eq!(t.year, Some(2013));
        assert_eq!(t.track_no, None);
        assert_eq!(
            t.album_id,
            crate::library::album_id_for("SPYAIR", "YouTube")
        );
    }

    #[test]
    fn bad_numbers_fail_before_anything_is_held() {
        assert!(
            check(&TagChanges {
                year: some("20x3"),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            check(&TagChanges {
                year: some(" "),
                ..Default::default()
            })
            .is_ok()
        );
    }
}
