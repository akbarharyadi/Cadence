//! What a scan remembers between scans about files the library's own entries can't stand in for:
//! an audio file that a CUE sheet splits into songs (the library holds the songs, not the file's
//! entry) and a file whose tags can't be read (it has no entry at all). Without this, every rescan
//! read all of them again and logged each damaged file again; on a library with 58 CUE albums and
//! 9 damaged files that was 67 files read on every change in the music folders.
//!
//! Kept in memory only, keyed by path, size and modified time, and rebuilt on each scan, so it
//! holds no more than the files of that kind that are still there. The first scan after start
//! reads them once.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::Track;

/// A file as the last scan found it.
#[derive(Clone)]
pub struct Seen {
    pub size: u64,
    pub modified: u64,
    /// The file's own entry, or `None` when its tags couldn't be read.
    pub track: Option<Track>,
}

pub type Memo = HashMap<String, Seen>;

static MEMO: Mutex<Option<Memo>> = Mutex::new(None);

/// Takes what the last scan remembered (a scan running at the same time gets nothing and simply
/// reads its files).
pub fn take() -> Memo {
    MEMO.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default()
}

/// Stores what this scan learned, replacing the last scan's.
fn keep(memo: Memo) {
    *MEMO.lock().unwrap_or_else(|e| e.into_inner()) = Some(memo);
}

/// The songs a scan found; what it learned is kept for the next scan.
pub fn settle(checked: Vec<Checked>) -> Vec<Track> {
    let mut remembered = Memo::new();
    let tracks = checked
        .into_iter()
        .filter_map(|c| {
            remembered.extend(c.remember);
            c.track
        })
        .collect();
    keep(remembered);
    tracks
}

/// The result of checking one file.
pub struct Checked {
    pub track: Option<Track>,
    /// What to remember about the file for the next scan, if anything.
    pub remember: Option<(String, Seen)>,
    /// The file was read (not taken from the library or the memo).
    pub read: bool,
}

/// Decides whether a file must be read: the library's entry is used when the file is unchanged
/// and isn't split by a CUE sheet, then what the last scan remembered, and only then `read`.
pub fn check(
    path: &Path,
    (size, modified): (u64, u64),
    old: Option<Track>,
    memo: &Memo,
    read: impl Fn(&Path) -> Option<Track>,
) -> Checked {
    let unchanged = |s: u64, m: u64| s == size && m == modified;
    let old = match old {
        // (A cached song of a single-file album isn't the file's own entry.)
        Some(t) if t.cue.is_none() && unchanged(t.size, t.modified) => {
            return Checked { track: Some(t), remember: None, read: false };
        }
        old => old,
    };
    let key = path.to_string_lossy().into_owned();
    if let Some(seen) = memo.get(&key).filter(|s| unchanged(s.size, s.modified)) {
        return Checked { track: seen.track.clone(), remember: Some((key, seen.clone())), read: false };
    }
    let split = old.as_ref().is_some_and(|o| o.cue.is_some());
    let track = read(path).map(|mut t| {
        t.size = size;
        t.modified = modified;
        t.added = old.map(|o| o.added).unwrap_or_else(super::now);
        t
    });
    let remember = (track.is_none() || split).then(|| (key, Seen { size, modified, track: track.clone() }));
    Checked { track, remember, read: true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::Span;
    use std::cell::Cell;

    fn entry(path: &str, cue: bool) -> Track {
        Track {
            id: "x".into(),
            path: path.into(),
            size: 10,
            modified: 100,
            added: 7,
            cue: cue.then_some(Span { start: 0.0, end: None }),
            ..Default::default()
        }
    }

    /// Runs `check` twice on one file the way two scans in a row would, counting reads.
    fn two_scans(old: Option<Track>, read_ok: bool, stamp2: (u64, u64)) -> (usize, Checked) {
        let reads = Cell::new(0);
        let reader = |p: &Path| {
            reads.set(reads.get() + 1);
            read_ok.then(|| entry(&p.to_string_lossy(), false))
        };
        let path = Path::new("D:/Music/album.flac");
        let first = check(path, (10, 100), old.clone(), &Memo::new(), reader);
        let memo: Memo = first.remember.into_iter().collect();
        let second = check(path, stamp2, old, &memo, reader);
        (reads.get(), second)
    }

    #[test]
    fn a_file_split_by_a_cue_is_read_once_not_on_every_scan() {
        let (reads, second) = two_scans(Some(entry("D:/Music/album.flac", true)), true, (10, 100));
        assert_eq!(reads, 1);
        let t = second.track.expect("the file's entry comes from the memo");
        assert!(t.cue.is_none(), "the memo holds the file's own entry, not one of its songs");
        assert_eq!(t.added, 7, "the date it was added is kept");
        assert!(second.remember.is_some(), "and it's remembered for the scan after");
    }

    #[test]
    fn a_damaged_file_is_tried_once_until_it_changes() {
        let (reads, second) = two_scans(None, false, (10, 100));
        assert_eq!(reads, 1);
        assert!(second.track.is_none() && !second.read);
        // Replaced by a good copy: a new size or time means it's read again.
        let (reads, second) = two_scans(None, false, (11, 200));
        assert_eq!(reads, 2);
        assert!(second.read);
    }

    #[test]
    fn an_ordinary_unchanged_song_is_never_read_or_remembered() {
        let (reads, second) = two_scans(Some(entry("D:/Music/album.flac", false)), true, (10, 100));
        assert_eq!(reads, 0);
        assert!(second.remember.is_none());
    }
}
