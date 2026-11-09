//! Undo for the changes Cadence makes to songs' files: Fix song info, Fix Album Info, Fix all,
//! renaming an artist, merging a split album and editing info by hand.
//!
//! Before a change writes a file, the file's info as it is (title, artist, album, album artist,
//! genre, year, track and disc numbers, subtitle, and its cover when the change replaces it) is
//! kept, grouped by change, in `changes/changes.json`. Undo writes it back. The last 30 changes are
//! kept; older ones (and their covers) are let go.
//!
//! Removing copies (Needs Attention, Library Health) is a change too: the files go to the Recycle
//! Bin (Trash), and Undo brings them back from it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lofty::config::WriteOptions;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;

/// How many changes are kept to undo.
const KEEP: usize = 30;

/// A file's info before a change.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Before {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub subtitle: Option<String>,
    /// The front cover, saved in the changes folder (when the change replaced it).
    pub cover: Option<String>,
    /// The file had no front cover: one the change added is taken out again.
    pub no_cover: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Batch {
    id: String,
    /// Unix seconds.
    at: u64,
    label: String,
    songs: Vec<Before>,
    /// Files the change moved to the Recycle Bin (their paths), brought back by Undo.
    #[serde(default)]
    removed: Vec<String>,
}

/// A change that can be undone, for the list in Needs Attention.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeInfo {
    id: String,
    at: u64,
    label: String,
    songs: usize,
}

fn dir(ctx: &Ctx) -> PathBuf {
    ctx.state.data_dir.join("changes")
}
fn list_file(dir: &Path) -> PathBuf {
    dir.join("changes.json")
}
fn load(dir: &Path) -> Vec<Batch> {
    std::fs::read(list_file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn save(dir: &Path, list: &[Batch]) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(b) = serde_json::to_vec(list) {
        let _ = std::fs::write(list_file(dir), b);
    }
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Keeps what a change's files were like before it, as it goes; `finish` keeps the change.
pub struct Recorder {
    id: String,
    label: String,
    dir: PathBuf,
    songs: Vec<Before>,
    removed: Vec<String>,
}

impl Recorder {
    pub fn new(ctx: &Ctx, label: impl Into<String>) -> Self {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Recorder { id: format!("c{at}"), label: label.into(), dir: dir(ctx), songs: Vec::new(), removed: Vec::new() }
    }

    /// Notes a file the change moved to the Recycle Bin.
    pub fn removed(&mut self, path: &str) {
        self.removed.push(path.to_string());
    }

    /// Notes the file's info before it's written (`cover`: the change replaces its cover).
    /// A file noted already in this change keeps its first state.
    pub fn add(&mut self, path: &str, cover: bool) {
        if self.songs.iter().any(|b| b.path == path) {
            return;
        }
        let n = self.songs.len();
        if let Some(b) = snapshot(Path::new(path), cover.then(|| (self.dir.as_path(), format!("{}-{n}", self.id)))) {
            self.songs.push(b);
        }
    }

    /// Keeps the change to undo; returns its id (none when nothing was noted).
    pub fn finish(self, ctx: &Ctx) -> Option<String> {
        if self.songs.is_empty() && self.removed.is_empty() {
            return None;
        }
        let mut list = load(&self.dir);
        list.push(Batch { id: self.id.clone(), at: now(), label: self.label, songs: self.songs, removed: self.removed });
        while list.len() > KEEP {
            forget(&self.dir, &list.remove(0));
        }
        save(&self.dir, &list);
        ctx.emit("changes-changed", ());
        Some(self.id)
    }
}

/// Lets go of a change's saved covers.
fn forget(dir: &Path, batch: &Batch) {
    for b in &batch.songs {
        if let Some(c) = &b.cover {
            let _ = std::fs::remove_file(dir.join(c));
        }
    }
}

/// The file's info as it is now; with `cover_to` (folder, name), its front cover is saved there.
fn snapshot(path: &Path, cover_to: Option<(&Path, String)>) -> Option<Before> {
    let tagged = lofty::read_from_path(path).ok()?;
    let mut b = Before { path: path.to_string_lossy().into_owned(), no_cover: true, ..Default::default() };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else { return Some(b) };
    let text = |k: ItemKey| tag.get_string(k).map(str::to_string);
    b.title = text(ItemKey::TrackTitle);
    b.artist = text(ItemKey::TrackArtist);
    b.album = text(ItemKey::AlbumTitle);
    b.album_artist = text(ItemKey::AlbumArtist);
    b.genre = text(ItemKey::Genre);
    b.subtitle = text(ItemKey::TrackSubtitle);
    b.year = tag.date().map(|d| d.year as u32);
    b.track_no = tag.track();
    b.disc_no = tag.disk();
    if let Some(front) = tag.pictures().iter().find(|p| p.pic_type() == PictureType::CoverFront) {
        b.no_cover = false;
        if let Some((dir, name)) = cover_to {
            let _ = std::fs::create_dir_all(dir);
            if std::fs::write(dir.join(&name), front.data()).is_ok() {
                b.cover = Some(name);
            }
        }
    }
    Some(b)
}

/// Writes a file's earlier info back.
fn restore(dir: &Path, b: &Before) -> Result<(), String> {
    let path = Path::new(&b.path);
    let text = |v: &Option<String>| Some(v.clone().unwrap_or_default());
    let num = |v: Option<u32>| Some(v.map(|n| n.to_string()).unwrap_or_default());
    crate::tagedit::write(
        path,
        &crate::tagedit::TagChanges {
            title: text(&b.title),
            artist: text(&b.artist),
            album: text(&b.album),
            album_artist: text(&b.album_artist),
            genre: text(&b.genre),
            year: num(b.year),
            track_no: num(b.track_no),
            disc_no: num(b.disc_no),
        },
    )?;
    if let Some(c) = &b.cover {
        let bytes = std::fs::read(dir.join(c)).map_err(|e| e.to_string())?;
        crate::covers::embed(path, &bytes)?;
    }
    let mut tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tagged.primary_tag_type()));
    }
    let tag = tagged.primary_tag_mut().expect("tag was just inserted");
    if let Some(s) = &b.subtitle {
        tag.insert_text(ItemKey::TrackSubtitle, s.clone());
    } else {
        tag.remove_key(ItemKey::TrackSubtitle);
    }
    if b.no_cover {
        tag.remove_picture_type(PictureType::CoverFront);
    }
    tag.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

/// The changes that can be undone, newest first.
pub fn recent(ctx: &Ctx) -> Vec<ChangeInfo> {
    load(&dir(ctx))
        .into_iter()
        .rev()
        .map(|b| ChangeInfo { id: b.id, at: b.at, label: b.label, songs: b.songs.len() + b.removed.len() })
        .collect()
}

/// Undoes a change: its files get their earlier info back (the playing song's when it ends).
/// Returns how many songs.
pub fn undo(ctx: &Ctx, id: &str) -> Result<usize, String> {
    let dir = dir(ctx);
    let mut list = load(&dir);
    let i = list.iter().position(|b| b.id == id).ok_or("This change can't be undone any more.")?;
    let batch = list.remove(i);
    let (playing_id, playing_path) = {
        let s = ctx.state.engine.status();
        let id = s.index.and_then(|i| s.queue.get(i).cloned());
        let lib = ctx.state.library.lock();
        let path = id.as_ref().and_then(|id| lib.by_id.get(id).map(|&i| lib.snapshot.tracks[i].path.clone()));
        (id, path)
    };
    let art_dir = ctx.state.art_dir();
    let mut written = 0;
    let mut failed = Vec::new();
    if !batch.removed.is_empty() {
        let back = bring_back(&batch.removed);
        written += back;
        if back < batch.removed.len() {
            failed.push(format!("{} files not in the Recycle Bin any more", batch.removed.len() - back));
        }
    }
    for b in &batch.songs {
        // The album's artwork as Cadence shows it goes back too (Fix song info had replaced it).
        if let Some(c) = &b.cover
            && let Ok(bytes) = std::fs::read(dir.join(c)) {
                let by = b.album_artist.clone().or_else(|| b.artist.clone()).unwrap_or_else(|| "Unknown Artist".into());
                let album = b.album.clone().unwrap_or_else(|| {
                    Path::new(&b.path).parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                });
                crate::library::write_art(&crate::library::album_id_for(&by, &album), &bytes, &art_dir);
            }
        // The album had no cover before: the artwork Cadence shows goes too.
        if b.no_cover && b.cover.is_none() {
            let by = b.album_artist.clone().or_else(|| b.artist.clone()).unwrap_or_else(|| "Unknown Artist".into());
            let album = b.album.clone().unwrap_or_else(|| {
                Path::new(&b.path).parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            });
            let id = crate::library::album_id_for(&by, &album);
            for name in [format!("{id}.jpg"), format!("{id}_s.jpg"), format!("{id}.colors")] {
                let _ = std::fs::remove_file(art_dir.join(name));
            }
        }
        if playing_path.as_deref() == Some(b.path.as_str()) {
            let (ctx2, dir2, b2) = (ctx.clone(), dir.clone(), b.clone());
            if let Some(pid) = &playing_id {
                crate::autotag::after_song(
                    pid,
                    Box::new(move || match restore(&dir2, &b2) {
                        Ok(()) => crate::jobs::start_scan(ctx2),
                        Err(e) => log::warn!("couldn't undo {}: {e}", b2.path),
                    }),
                );
            }
            continue;
        }
        match restore(&dir, b) {
            Ok(()) => written += 1,
            Err(e) => {
                log::warn!("couldn't undo {}: {e}", b.path);
                failed.push(b.path.clone());
            }
        }
    }
    // The playing song's cover file is still needed until it ends.
    if playing_path.is_none_or(|p| !batch.songs.iter().any(|b| b.path == p)) {
        forget(&dir, &batch);
    }
    save(&dir, &list);
    ctx.emit("changes-changed", ());
    if written > 0 {
        crate::jobs::start_scan(ctx.clone());
    }
    if !failed.is_empty() && written == 0 {
        if batch.songs.is_empty() {
            return Err("Those files aren't in the Recycle Bin any more.".into());
        }
        return Err(format!("Couldn't write {} of the songs back (are they open in another app?).", failed.len()));
    }
    Ok(batch.songs.len() + batch.removed.len())
}

/// Moves a file to the Recycle Bin (Trash). Phones have none: there it's deleted.
pub fn to_recycle_bin(path: &Path) -> Result<(), String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return trash::delete(path).map_err(|e| e.to_string());
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return std::fs::remove_file(path).map_err(|e| e.to_string());
}

/// Whether removed files can be brought back (from the Recycle Bin).
pub const CAN_BRING_BACK: bool = cfg!(any(windows, target_os = "linux"));

/// Brings files back from the Recycle Bin to where they were (the latest copy of each, if
/// the place is free). Returns how many.
fn bring_back(paths: &[String]) -> usize {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let Ok(items) = trash::os_limited::list() else { return 0 };
        let mut picked: Vec<trash::TrashItem> = Vec::new();
        for p in paths {
            let p = Path::new(p);
            if p.exists() {
                continue;
            }
            let latest = items
                .iter()
                .filter(|i| i.original_path() == p)
                .max_by_key(|i| i.time_deleted);
            if let Some(i) = latest {
                picked.push(i.clone());
            }
        }
        let n = picked.len();
        if n == 0 {
            return 0;
        }
        match trash::os_limited::restore_all(picked) {
            Ok(()) => n,
            Err(e) => {
                log::warn!("couldn't bring files back from the Recycle Bin: {e}");
                0
            }
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = paths;
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a copy of a tagged song with a cover: a change and its undo leave the file as it was.
    /// `CADENCE_TAGGED_FILE=path cargo test --lib changes::tests -- --ignored`.
    #[test]
    #[ignore]
    fn undo_puts_the_file_back() {
        let Ok(src) = std::env::var("CADENCE_TAGGED_FILE") else { return };
        let dir = std::env::temp_dir().join("cadence-changes-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join(Path::new(&src).file_name().unwrap());
        std::fs::copy(&src, &copy).unwrap();
        let before = snapshot(&copy, Some((dir.as_path(), "cover".into()))).unwrap();
        assert!(before.cover.is_some() && before.title.is_some());
        // The change: other info, a new cover and a subtitle.
        crate::tagedit::write(
            &copy,
            &crate::tagedit::TagChanges { title: Some("Changed".into()), album: Some("Other".into()), year: Some("1999".into()), ..Default::default() },
        )
        .unwrap();
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([0, 255, 0])))
            .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .unwrap();
        crate::covers::embed(&copy, &jpeg).unwrap();
        restore(&dir, &before).unwrap();
        let after = snapshot(&copy, Some((dir.as_path(), "after".into()))).unwrap();
        assert_eq!((after.title, after.album, after.year), (before.title, before.album, before.year));
        assert_eq!(after.subtitle, before.subtitle);
        assert_eq!(std::fs::read(dir.join("after")).unwrap(), std::fs::read(dir.join("cover")).unwrap(), "the old cover is back");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
