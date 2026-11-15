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
    /// The whole date when the file has more than a year ("2019-05-01"), so Undo keeps it.
    #[serde(default)]
    pub date: Option<String>,
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
    /// Favourites and ratings of removed files that went to the copy that stayed, given back
    /// with the file.
    #[serde(default)]
    marks: Vec<RemovedMark>,
}

/// A removed file's mark, handed to the copy that stayed.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemovedMark {
    path: String,
    mark: crate::marks::PassedOn,
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
    crate::state::read_json(&list_file(dir))
}
fn save(dir: &Path, list: &[Batch]) {
    crate::state::write_json(&list_file(dir), &list);
}

/// One change to the list at a time (load, change, save): a change kept while another is undone
/// can't be lost.
static LIST: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
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
    marks: Vec<RemovedMark>,
}

impl Recorder {
    pub fn new(ctx: &Ctx, label: impl Into<String>) -> Self {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Recorder { id: format!("c{at}"), label: label.into(), dir: dir(ctx), songs: Vec::new(), removed: Vec::new(), marks: Vec::new() }
    }

    /// Notes a file the change moved to the Recycle Bin.
    pub fn removed(&mut self, path: &str) {
        self.removed.push(path.to_string());
    }

    /// Notes that a removed file's mark went to the copy that stayed (given back by Undo).
    pub fn passed_on(&mut self, path: &str, mark: crate::marks::PassedOn) {
        self.marks.push(RemovedMark { path: path.to_string(), mark });
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
        {
            let _one = LIST.lock();
            let mut list = load(&self.dir);
            list.push(Batch { id: self.id.clone(), at: now(), label: self.label, songs: self.songs, removed: self.removed, marks: self.marks });
            while list.len() > KEEP {
                forget(&self.dir, &list.remove(0).songs);
            }
            save(&self.dir, &list);
        }
        ctx.emit("changes-changed", ());
        Some(self.id)
    }
}

/// Lets go of the saved covers of a change's songs.
fn forget(dir: &Path, songs: &[Before]) {
    for b in songs {
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
    b.date = tag.date().filter(|d| d.month.is_some()).map(|d| match (d.month, d.day) {
        (Some(m), Some(day)) => format!("{:04}-{m:02}-{day:02}", d.year),
        (Some(m), None) => format!("{:04}-{m:02}", d.year),
        _ => d.year.to_string(),
    });
    b.track_no = tag.track();
    b.disc_no = tag.disk();
    if let Some(front) = tag.pictures().iter().find(|p| p.pic_type() == PictureType::CoverFront) {
        b.no_cover = false;
        if let Some((dir, name)) = cover_to {
            let _ = std::fs::create_dir_all(dir);
            if crate::state::write_file(&dir.join(&name), front.data(), false).is_ok() {
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
            year: b.date.clone().or_else(|| num(b.year)),
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

/// A change that can be undone here: one that only removed files can't be where nothing comes back
/// from a Recycle Bin (`CAN_BRING_BACK`; such changes aren't kept there, but a list can still have one).
fn can_undo(b: &Batch) -> bool {
    CAN_BRING_BACK || !b.songs.is_empty()
}

/// The changes that can be undone, newest first.
pub fn recent(ctx: &Ctx) -> Vec<ChangeInfo> {
    load(&dir(ctx))
        .into_iter()
        .rev()
        .filter(can_undo)
        .map(|b| ChangeInfo { id: b.id, at: b.at, label: b.label, songs: b.songs.len() + b.removed.len() })
        .collect()
}

/// What's left of a change after an undo that didn't all work: the songs not written back, the
/// files that could come back but didn't yet (`retry`), and their marks. Files that can never come
/// back (not in the Recycle Bin any more, or no Recycle Bin to bring them back from) aren't kept:
/// trying again could only fail again. None when nothing is left to try.
fn left_over(batch: &Batch, failed_songs: &[String], retry: &[String]) -> Option<Batch> {
    let left = Batch {
        id: batch.id.clone(),
        at: batch.at,
        label: batch.label.clone(),
        songs: batch.songs.iter().filter(|b| failed_songs.contains(&b.path)).cloned().collect(),
        removed: batch.removed.iter().filter(|p| retry.contains(p)).cloned().collect(),
        marks: batch.marks.iter().filter(|m| retry.contains(&m.path)).cloned().collect(),
    };
    (!left.songs.is_empty() || !left.removed.is_empty()).then_some(left)
}

/// What to tell the listener when part of an undo failed (`done` of `total` undone): files that
/// didn't come back but may (`retry`), ones that never can (`lost`), songs not written back.
fn partly_undone(done: usize, total: usize, failed_songs: &[String], retry: usize, lost: usize) -> String {
    let files = |n: usize| if n == 1 { "1 file".to_string() } else { format!("{n} files") };
    let mut why = Vec::new();
    if lost > 0 {
        let is = if lost == 1 { "is" } else { "are" };
        if CAN_BRING_BACK {
            why.push(format!("{} {is} not in the Recycle Bin any more", files(lost)));
        } else {
            why.push(format!("{} can't be brought back here (no Recycle Bin)", files(lost)));
        }
    }
    if retry > 0 {
        why.push(format!("{} couldn't be brought back from the Recycle Bin", files(retry)));
    }
    if !failed_songs.is_empty() {
        let names: Vec<String> = failed_songs.iter().take(3).map(|p| file_name(p)).collect();
        let more = if failed_songs.len() > 3 { format!(" and {} more", failed_songs.len() - 3) } else { String::new() };
        why.push(format!("couldn't write {}{more} back (open in another app?)", names.join(", ")));
    }
    let start = if done == 0 { "Nothing was undone".to_string() } else { format!("Undid {done} of {total}") };
    let again = if retry > 0 || !failed_songs.is_empty() { " Try Undo again to retry the rest." } else { "" };
    format!("{start}: {}.{again}", why.join("; "))
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string())
}

/// Undoes a change: its files get their earlier info back (the playing song's when it ends),
/// removed files come back from the Recycle Bin with their favourites and ratings. Returns how
/// many songs. When part of it fails, what failed stays in the list to try again, and the error
/// says what was and wasn't undone.
pub fn undo(ctx: &Ctx, id: &str) -> Result<usize, String> {
    undo_with(ctx, id, bring_back)
}

/// `undo`, bringing removed files back with `bring_back` (the Recycle Bin's, or a test's).
fn undo_with(ctx: &Ctx, id: &str, bring_back: impl Fn(&[String]) -> BroughtBack) -> Result<usize, String> {
    // One undo at a time (two clicks mustn't both bring the same files back).
    static UNDOING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _one = UNDOING.lock();
    let dir = dir(ctx);
    let batch = load(&dir).into_iter().find(|b| b.id == id).ok_or("This change can't be undone any more.")?;
    if !can_undo(&batch) {
        // Kept by mistake where nothing comes back from a Recycle Bin: let go, not offered again.
        forget_batch(&dir, &batch.id);
        ctx.emit("changes-changed", ());
        return Err("Removed files can't be brought back on this device.".into());
    }
    let (playing_id, playing_path) = {
        let s = ctx.state.engine.status();
        let id = s.index.and_then(|i| s.queue.get(i).cloned());
        let lib = ctx.state.library.lock();
        let path = id.as_ref().and_then(|id| lib.by_id.get(id).map(|&i| lib.snapshot.tracks[i].path.clone()));
        (id, path)
    };
    let art_dir = ctx.state.art_dir();
    let mut written = 0;
    let mut failed: Vec<String> = Vec::new();
    // Removed files that aren't back (checked on the disk: one already put back by hand counts),
    // and of those, the ones that never can be (not in the Recycle Bin, or no Recycle Bin here).
    let mut missing: Vec<String> = Vec::new();
    let mut lost: Vec<String> = Vec::new();
    if !batch.removed.is_empty() {
        let back = bring_back(&batch.removed);
        missing = batch.removed.iter().filter(|p| !Path::new(p.as_str()).exists()).cloned().collect();
        lost = missing.iter().filter(|p| !CAN_BRING_BACK || back.not_in_bin.contains(p)).cloned().collect();
        written += batch.removed.len() - missing.len();
        // Their favourites and ratings come back with them.
        let back: Vec<&RemovedMark> = batch.marks.iter().filter(|m| !missing.contains(&m.path)).collect();
        if !back.is_empty() {
            {
                let mut marks = ctx.state.marks.lock();
                for m in back {
                    marks.take_back(&m.mark);
                }
            }
            ctx.state.save_marks();
            ctx.emit("marks-changed", ());
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
    // Only now is the change taken off the list (or cut down to what failed and can work when
    // tried again).
    let retry: Vec<String> = missing.iter().filter(|p| !lost.contains(p)).cloned().collect();
    let left = left_over(&batch, &failed, &retry);
    {
        let _one = LIST.lock();
        let mut list = load(&dir);
        if let Some(i) = list.iter().position(|b| b.id == batch.id) {
            match &left {
                Some(l) => list[i] = l.clone(),
                None => {
                    list.remove(i);
                }
            }
            save(&dir, &list);
        }
    }
    // Covers of songs written back are let go; the playing song's is still needed until it ends,
    // and those of songs to try again stay with them.
    let done_songs: Vec<Before> = batch
        .songs
        .iter()
        .filter(|b| !failed.contains(&b.path) && playing_path.as_deref() != Some(b.path.as_str()))
        .cloned()
        .collect();
    forget(&dir, &done_songs);
    ctx.emit("changes-changed", ());
    if written > 0 {
        crate::jobs::start_scan(ctx.clone());
    }
    let total = batch.songs.len() + batch.removed.len();
    if !failed.is_empty() || !missing.is_empty() {
        return Err(partly_undone(total - failed.len() - missing.len(), total, &failed, retry.len(), lost.len()));
    }
    Ok(total)
}

/// Takes a change off the list.
fn forget_batch(dir: &Path, id: &str) {
    let _one = LIST.lock();
    let mut list = load(dir);
    if let Some(i) = list.iter().position(|b| b.id == id) {
        let b = list.remove(i);
        forget(dir, &b.songs);
        save(dir, &list);
    }
}

/// What `bring_back` found.
#[derive(Default)]
struct BroughtBack {
    /// Files that aren't in the Recycle Bin at all (emptied, or never there): they can never come
    /// back, so Undo doesn't keep them to try again.
    not_in_bin: Vec<String>,
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
/// the place is free), and says which aren't in it at all. (A Recycle Bin that can't be read
/// counts as not knowing: those files may come back on another try.)
fn bring_back(paths: &[String]) -> BroughtBack {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let mut out = BroughtBack::default();
        let items = match trash::os_limited::list() {
            Ok(items) => items,
            Err(e) => {
                log::warn!("couldn't read the Recycle Bin: {e}");
                return out;
            }
        };
        let mut picked: Vec<trash::TrashItem> = Vec::new();
        for path in paths {
            let p = Path::new(path);
            if p.exists() {
                continue;
            }
            let latest = items
                .iter()
                .filter(|i| i.original_path() == p)
                .max_by_key(|i| i.time_deleted);
            match latest {
                Some(i) => picked.push(i.clone()),
                None => out.not_in_bin.push(path.clone()),
            }
        }
        if !picked.is_empty()
            && let Err(e) = trash::os_limited::restore_all(picked) {
                log::warn!("couldn't bring files back from the Recycle Bin: {e}");
            }
        out
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        // No Recycle Bin to bring files back from here (`CAN_BRING_BACK`).
        BroughtBack { not_in_bin: paths.to_vec() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(path: &str) -> Before {
        Before { path: path.into(), ..Default::default() }
    }

    fn mark(from: &str) -> crate::marks::PassedOn {
        let mut m = crate::marks::Marks::default();
        let t = |id: &str| crate::library::Track { id: id.into(), title: "T".into(), duration: 100.0, ..Default::default() };
        m.set_loved(&t(from), true);
        m.pass_on(from, &t("kept")).unwrap()
    }

    #[test]
    fn what_failed_stays_to_try_again() {
        let batch = Batch {
            id: "c1".into(),
            at: 5,
            label: "Removed 2 copies".into(),
            songs: vec![song("a.flac"), song("b.flac")],
            removed: vec!["x.mp3".into(), "y.mp3".into()],
            marks: vec![RemovedMark { path: "x.mp3".into(), mark: mark("x") }, RemovedMark { path: "y.mp3".into(), mark: mark("y") }],
        };
        assert!(left_over(&batch, &[], &[]).is_none(), "all undone: the change goes");
        let left = left_over(&batch, &["b.flac".into()], &["y.mp3".into()]).expect("some failed");
        assert_eq!((left.id.as_str(), left.label.as_str()), ("c1", "Removed 2 copies"));
        assert_eq!(left.songs.iter().map(|b| b.path.as_str()).collect::<Vec<_>>(), ["b.flac"]);
        assert_eq!(left.removed, ["y.mp3"]);
        assert_eq!(left.marks.len(), 1);
        assert_eq!(left.marks[0].path, "y.mp3");
        // Saved and read back by an older list that had no marks: still read.
        let old: Vec<Batch> = serde_json::from_str(r#"[{"id":"c0","at":1,"label":"Edited song info","songs":[]}]"#).unwrap();
        assert!(old[0].marks.is_empty() && old[0].removed.is_empty());
    }

    #[test]
    fn says_what_was_and_wasnt_undone() {
        let msg = partly_undone(3, 5, &["D:/Music/a.flac".into()], 0, 1);
        assert!(msg.starts_with("Undid 3 of 5"), "{msg}");
        assert!(msg.contains("1 file") && msg.contains("a.flac") && msg.contains("Try Undo again"), "{msg}");
        // Only files that can never come back: nothing to try again.
        let msg = partly_undone(0, 2, &[], 0, 2);
        assert!(msg.starts_with("Nothing was undone: 2 files") && !msg.contains("Try Undo again"), "{msg}");
        assert!(partly_undone(1, 2, &[], 1, 0).contains("couldn't be brought back from the Recycle Bin. Try Undo again"));
    }

    struct Quiet;
    impl crate::ctx::Host for Quiet {
        fn emit(&self, _: &str, _: serde_json::Value) {}
    }

    /// The app's core on a data folder of its own (no window; the player opens no device until a
    /// song plays).
    fn core(name: &str) -> (Ctx, PathBuf) {
        let base = std::env::temp_dir().join(format!("cadence-undo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (ctx, _status) = crate::hub::open(base.join("data"), &base.join("local"), std::sync::Arc::new(Quiet));
        (ctx, base)
    }

    #[test]
    fn undo_keeps_only_what_can_work_on_another_try() {
        let (ctx, base) = core("retry");
        let dir = dir(&ctx);
        let file = |n: &str| base.join(n).to_string_lossy().into_owned();
        let (back, emptied, stuck) = (file("back.mp3"), file("emptied.mp3"), file("stuck.mp3"));
        // A song whose file can't be written (it isn't there), and three removed copies.
        let batch = Batch {
            id: "c1".into(),
            at: 5,
            label: "Removed 3 copies".into(),
            songs: vec![song(&file("locked.flac"))],
            removed: vec![back.clone(), emptied.clone(), stuck.clone()],
            marks: vec![RemovedMark { path: stuck.clone(), mark: mark("stuck") }, RemovedMark { path: emptied.clone(), mark: mark("emptied") }],
        };
        save(&dir, &[batch]);
        // The Recycle Bin: "back" comes back; "emptied" isn't in it any more; "stuck" is, but
        // couldn't be put back this time.
        let (b, e) = (back.clone(), emptied.clone());
        let bin = move |paths: &[String]| {
            assert_eq!(paths.len(), 3);
            std::fs::write(&b, b"x").unwrap();
            BroughtBack { not_in_bin: vec![e.clone()] }
        };
        let err = undo_with(&ctx, "c1", bin).unwrap_err();
        assert!(err.starts_with("Undid 1 of 4"), "{err}");
        let list = load(&dir);
        assert_eq!(list.len(), 1, "something can still work: the change stays");
        let left = &list[0];
        assert_eq!(left.songs.len(), 1, "the song to write back is tried again");
        if CAN_BRING_BACK {
            assert_eq!(left.removed, [stuck.clone()], "the emptied one can never come back: not kept");
            assert_eq!(left.marks.len(), 1);
            assert_eq!(left.marks[0].path, stuck);
        } else {
            assert!(left.removed.is_empty(), "nothing ever comes back here: no removed file is kept");
        }
        // The second try brings "stuck" back; the song still can't be written.
        let s = stuck.clone();
        let err = undo_with(&ctx, "c1", move |_| {
            std::fs::write(&s, b"x").unwrap();
            BroughtBack::default()
        })
        .unwrap_err();
        assert!(err.contains("locked.flac"), "{err}");
        let list = load(&dir);
        assert!(list[0].removed.is_empty() && list[0].songs.len() == 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_removal_that_can_never_come_back_isnt_kept() {
        let (ctx, base) = core("lost");
        let dir = dir(&ctx);
        let gone = base.join("gone.mp3").to_string_lossy().into_owned();
        save(&dir, &[Batch { id: "c2".into(), at: 1, label: "Removed 1 copy".into(), songs: vec![], removed: vec![gone.clone()], marks: vec![] }]);
        let shown = recent(&ctx).len();
        let not_in_bin = gone.clone();
        let err = undo_with(&ctx, "c2", move |_| BroughtBack { not_in_bin: vec![not_in_bin.clone()] }).unwrap_err();
        assert!(!err.contains("Try Undo again"), "{err}");
        // Gone from the list: Undo isn't offered for it again, where it could only fail.
        assert!(load(&dir).is_empty());
        assert!(recent(&ctx).is_empty());
        // Where removed files can't come back (Android, macOS), such a change isn't even offered.
        assert_eq!(shown, CAN_BRING_BACK as usize);
        let _ = std::fs::remove_dir_all(&base);
    }

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
