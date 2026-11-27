//! The music list the phone reads: every song to keep there, with its key, size, where it goes on
//! the phone (under `Music/Cadence/`), and the tags needed to match it to listening data.
//! Songs whose key isn't worked out yet are counted as pending (the key job is started for them),
//! so the phone gets them on its next sync.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::library::Track;

/// The longest file or folder name written on the phone, in characters.
const MAX_NAME: usize = 80;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Manifest {
    /// The computer's name, for the phone to show.
    pub computer: String,
    /// Unix seconds.
    pub made: u64,
    pub songs: Vec<Entry>,
    /// Chosen songs whose key is still being worked out.
    pub pending: usize,
    /// Chosen songs that can't be sent yet (single-file albums).
    pub left_out: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    /// The file's key (see `keys`): also the name to ask the computer for it by.
    pub key: String,
    pub size: u64,
    /// Where it goes on the phone, below the sync folder, with `/` between parts.
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
}

/// The list for `songs` (in order), given the keys known so far (id → key).
pub fn build(computer: &str, made: u64, songs: &[&Track], keys: &HashMap<String, String>, left_out: usize) -> Manifest {
    let mut m = Manifest { computer: computer.to_string(), made, left_out, ..Default::default() };
    let mut taken: HashSet<String> = HashSet::new();
    let mut seen_keys: HashSet<&str> = HashSet::new();
    for t in songs {
        let Some(key) = keys.get(&t.id) else {
            m.pending += 1;
            continue;
        };
        // The same file twice in the library (a copy): once on the phone.
        if !seen_keys.insert(key.as_str()) {
            continue;
        }
        let path = unique(phone_path(t), &mut taken);
        m.songs.push(Entry {
            key: key.clone(),
            size: t.size,
            path,
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration: t.duration,
        });
    }
    m
}

/// "Artist/Album/01 Title.flac" for a song, every part safe for Android's file names.
pub fn phone_path(t: &Track) -> String {
    let artist = if t.album_artist.trim().is_empty() { &t.artist } else { &t.album_artist };
    let number = match (t.disc_no, t.track_no) {
        (Some(d), Some(n)) if d > 1 => format!("{d}-{n:02} "),
        (_, Some(n)) => format!("{n:02} "),
        _ => String::new(),
    };
    let ext = std::path::Path::new(&t.path).extension().and_then(|e| e.to_str()).map(str::to_lowercase).filter(|e| !e.is_empty() && e.len() <= 5);
    let file = safe_name(&format!("{number}{}", t.title), "Song");
    let file = match ext {
        Some(e) => format!("{file}.{e}"),
        None => file,
    };
    format!("{}/{}/{}", safe_name(artist, "Unknown artist"), safe_name(&t.album, "Unknown album"), file)
}

/// A name with the characters Android (and FAT cards) refuse taken out, no leading or trailing
/// dots and spaces, at most `MAX_NAME` characters (cut at a character, never inside one).
fn safe_name(s: &str, fallback: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { ' ' } else { c })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = joined.chars().take(MAX_NAME).collect();
    let trimmed = cut.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() { fallback.to_string() } else { trimmed.to_string() }
}

/// `path`, or "name (2).ext" and so on when another song already has it (compared without case:
/// phone storage ignores it).
fn unique(path: String, taken: &mut HashSet<String>) -> String {
    if taken.insert(path.to_lowercase()) {
        return path;
    }
    let (stem, ext) = match path.rfind('.').filter(|&i| path.rfind('/').is_none_or(|s| i > s)) {
        Some(i) => (path.get(..i).unwrap_or(&path), path.get(i..).unwrap_or("")),
        None => (path.as_str(), ""),
    };
    let mut n = 2;
    loop {
        let p = format!("{stem} ({n}){ext}");
        if taken.insert(p.to_lowercase()) {
            return p;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str, no: Option<u32>) -> Track {
        Track {
            id: id.into(),
            title: title.into(),
            artist: "Eir Aoi".into(),
            album: "IGNITE".into(),
            track_no: no,
            path: format!("D:/Music/{id}.FLAC"),
            size: 100,
            ..Default::default()
        }
    }

    #[test]
    fn a_song_goes_to_artist_album_and_numbered_file() {
        assert_eq!(phone_path(&song("a", "IGNITE", Some(1))), "Eir Aoi/IGNITE/01 IGNITE.flac");
        let mut t = song("b", "Brainwash", Some(2));
        t.disc_no = Some(2);
        t.album_artist = "藍井エイル".into();
        assert_eq!(phone_path(&t), "藍井エイル/IGNITE/2-02 Brainwash.flac");
    }

    #[test]
    fn names_android_refuses_are_made_safe() {
        assert_eq!(safe_name("What? / Why: \"now\"*", "x"), "What Why now");
        assert_eq!(safe_name("...", "Unknown album"), "Unknown album");
        assert_eq!(safe_name(" .hidden. ", "x"), "hidden");
        let long = "夜".repeat(200);
        assert_eq!(safe_name(&long, "x").chars().count(), MAX_NAME, "cut at a character");
    }

    #[test]
    fn two_songs_with_one_name_get_two_files() {
        let mut taken = HashSet::new();
        assert_eq!(unique("A/B/01 X.flac".into(), &mut taken), "A/B/01 X.flac");
        assert_eq!(unique("a/b/01 x.flac".into(), &mut taken), "a/b/01 x (2).flac");
        assert_eq!(unique("A/B/01 X.flac".into(), &mut taken), "A/B/01 X (3).flac");
    }

    #[test]
    fn songs_without_a_key_are_pending_and_copies_go_once() {
        let (a, b, c) = (song("a", "One", Some(1)), song("b", "Two", Some(2)), song("c", "One copy", Some(1)));
        let keys: HashMap<String, String> = [("a".into(), "k1".into()), ("c".into(), "k1".into())].into();
        let m = build("PC", 5, &[&a, &b, &c], &keys, 3);
        assert_eq!(m.songs.len(), 1);
        assert_eq!((m.songs[0].key.as_str(), m.songs[0].path.as_str()), ("k1", "Eir Aoi/IGNITE/01 One.flac"));
        assert_eq!((m.pending, m.left_out), (1, 3));
    }
}
