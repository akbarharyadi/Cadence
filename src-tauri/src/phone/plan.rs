//! The phone's side, the part with no network or files: what a sync does, given the computer's
//! list (`Manifest`) and the phone's ledger of the files it copied itself.
//!
//! - A song already copied (its key in the ledger, its file still there at the size the ledger
//!   noted) stays. A song only moved on the list keeps its old name: nothing is renamed.
//! - A song not copied yet is downloaded.
//! - A copied song no longer on the list is removed. Only files in the ledger are ever removed, so
//!   music the listener put on the phone some other way is never touched.
//!
//! The list comes over the network, so every path in it is checked (`safe_rel`) before use.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest::{Entry, Manifest};

/// A file this phone copied from the computer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Held {
    /// Below the sync folder, with `/` between parts.
    pub path: String,
    pub size: u64,
}

/// The phone's record of what it copied (in its data folder; never in a backup: it describes
/// this phone's files).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Ledger {
    /// Song key → its file.
    pub files: BTreeMap<String, Held>,
    /// Unix seconds of the last sync that finished, and what it did, in words.
    pub last_sync: u64,
    pub last_result: String,
}

/// What a sync does.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub get: Vec<Entry>,
    /// Song keys whose files go (the paths are in the ledger).
    pub drop: Vec<String>,
    /// Bytes to download.
    pub bytes: u64,
}

/// The sync for `list`, given the `ledger` and whether a file (below the sync folder) is there at
/// a size.
pub fn plan(list: &Manifest, ledger: &Ledger, there: impl Fn(&str, u64) -> bool) -> Plan {
    let mut p = Plan::default();
    let wanted: HashSet<&str> = list.songs.iter().map(|e| e.key.as_str()).collect();
    for e in &list.songs {
        let kept = ledger.files.get(&e.key).is_some_and(|h| there(&h.path, h.size));
        if !kept {
            p.bytes = p.bytes.saturating_add(e.size);
            p.get.push(e.clone());
        }
    }
    p.drop = ledger.files.keys().filter(|k| !wanted.contains(k.as_str())).cloned().collect();
    p
}

/// The longest path part accepted, in bytes (Android's limit for a file name).
const MAX_PART: usize = 255;
/// Artist/Album/Song: deeper paths aren't from Cadence.
const MAX_DEPTH: usize = 4;

/// `rel` below `root`, when it's a plain relative path of a few safe parts: no `..`, no root, no
/// drive, no backslash, no control characters. None for anything else.
pub fn safe_rel(root: &Path, rel: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.is_empty() || parts.len() > MAX_DEPTH {
        return None;
    }
    let mut out = root.to_path_buf();
    for part in parts {
        let bad = part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > MAX_PART
            || part.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
            || part.starts_with(' ')
            || part.ends_with([' ', '.']);
        if bad {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// `path` with " (n)" before its extension: another name for a song whose own name is taken by a
/// file that isn't this song.
pub fn other_name(path: &Path, n: u32) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem} ({n}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({n})"),
    };
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, path: &str, size: u64) -> Entry {
        Entry { key: key.into(), path: path.into(), size, ..Default::default() }
    }

    fn held(path: &str, size: u64) -> Held {
        Held { path: path.into(), size }
    }

    #[test]
    fn new_songs_are_fetched_copied_ones_stay_and_unchosen_ones_go() {
        let list = Manifest { songs: vec![entry("k1", "A/B/01 One.flac", 10), entry("k2", "A/B/02 Two.flac", 20)], ..Default::default() };
        let mut ledger = Ledger::default();
        ledger.files.insert("k1".into(), held("A/B/01 One.flac", 10));
        ledger.files.insert("k9".into(), held("Old/Gone.flac", 5));
        let p = plan(&list, &ledger, |_, _| true);
        assert_eq!(p.get.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["k2"]);
        assert_eq!(p.drop, ["k9"]);
        assert_eq!(p.bytes, 20);
    }

    #[test]
    fn a_copied_song_whose_file_went_missing_or_changed_is_fetched_again() {
        let list = Manifest { songs: vec![entry("k1", "A/B/01 One.flac", 10)], ..Default::default() };
        let mut ledger = Ledger::default();
        ledger.files.insert("k1".into(), held("A/B/01 One.flac", 10));
        assert_eq!(plan(&list, &ledger, |_, _| false).get.len(), 1);
        assert!(plan(&list, &ledger, |_, _| false).drop.is_empty(), "still wanted: not removed");
    }

    #[test]
    fn a_song_moved_on_the_list_keeps_its_file() {
        let list = Manifest { songs: vec![entry("k1", "New Artist/B/01 One.flac", 10)], ..Default::default() };
        let mut ledger = Ledger::default();
        ledger.files.insert("k1".into(), held("Old Artist/B/01 One.flac", 10));
        let p = plan(&list, &ledger, |path, _| path == "Old Artist/B/01 One.flac");
        assert!(p.get.is_empty() && p.drop.is_empty());
    }

    #[test]
    fn an_empty_list_removes_only_what_this_phone_copied() {
        let mut ledger = Ledger::default();
        ledger.files.insert("k1".into(), held("A/1.flac", 1));
        let p = plan(&Manifest::default(), &ledger, |_, _| true);
        assert_eq!(p.drop, ["k1"]);
        assert!(plan(&Manifest::default(), &Ledger::default(), |_, _| true).drop.is_empty());
    }

    #[test]
    fn paths_from_the_network_stay_inside_the_sync_folder() {
        let root = Path::new("/sdcard/Music/Cadence");
        assert_eq!(safe_rel(root, "藍井エイル/IGNITE/01 IGNITE.flac"), Some(root.join("藍井エイル").join("IGNITE").join("01 IGNITE.flac")));
        for bad in ["../x.flac", "A/../../x.flac", "/etc/passwd", "C:/x.flac", "A\\..\\x.flac", "A//x.flac", "", "A/B/C/D/E.flac", "A/ x.flac", "A/x.", "A/\u{0}.flac"] {
            assert_eq!(safe_rel(root, bad), None, "{bad:?}");
        }
        assert_eq!(safe_rel(root, &format!("A/{}.flac", "x".repeat(300))), None);
    }

    #[test]
    fn another_name_keeps_the_extension() {
        assert_eq!(other_name(Path::new("/m/A/01 One.flac"), 2), Path::new("/m/A/01 One (2).flac"));
        assert_eq!(other_name(Path::new("/m/A/Two"), 3), Path::new("/m/A/Two (3)"));
    }

    #[test]
    fn an_old_or_partial_ledger_still_loads() {
        let l: Ledger = serde_json::from_str(r#"{"files":{"k":{"path":"A/1.flac"}}}"#).unwrap();
        assert_eq!(l.files["k"].size, 0);
        assert_eq!(l.last_sync, 0);
    }
}
