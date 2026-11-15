//! MusicBrainz ids written in a song's tags (Picard and others write them), read when they're
//! needed rather than for the whole library: a song finishing, a song being hearted, a match being
//! checked. One small read of the tags (no cover, no audio properties); the last few are kept.
//!
//! In every tag format `MusicBrainzRecordingId` is the *recording* (Vorbis `MUSICBRAINZ_TRACKID`,
//! ID3 `UFID` from musicbrainz.org, MP4 `MusicBrainz Track Id`).

use std::path::Path;
use std::time::SystemTime;

use lofty::config::ParseOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use parking_lot::Mutex;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ids {
    pub recording: Option<String>,
    pub release: Option<String>,
    pub artists: Vec<String>,
}

impl Ids {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.recording.is_none() && self.release.is_none() && self.artists.is_empty()
    }
}

/// Whether `s` is written like a MusicBrainz id (a UUID, 8-4-4-4-12 hex digits).
pub fn is_mbid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// The ids in one tag value (several may share it, separated by ";", "/" or spaces), lower case.
fn ids_in(value: &str) -> Vec<String> {
    value.split(|c: char| c == ';' || c == '/' || c == ',' || c.is_whitespace()).map(|p| p.trim().to_ascii_lowercase()).filter(|p| is_mbid(p)).collect()
}

pub fn from_tag(tag: &Tag) -> Ids {
    let first = |k: ItemKey| tag.get_strings(k).flat_map(ids_in).next();
    let mut artists: Vec<String> = Vec::new();
    for id in tag.get_strings(ItemKey::MusicBrainzArtistId).flat_map(ids_in) {
        if !artists.contains(&id) {
            artists.push(id);
        }
    }
    Ids { recording: first(ItemKey::MusicBrainzRecordingId), release: first(ItemKey::MusicBrainzReleaseId), artists }
}

/// The ids in a file's tags (empty when it has none or can't be read).
pub fn read(path: &Path) -> Ids {
    let opts = ParseOptions::new().read_cover_art(false).read_properties(false);
    let Ok(probe) = Probe::open(path) else { return Ids::default() };
    let Ok(tagged) = probe.options(opts).read() else { return Ids::default() };
    tagged.primary_tag().or_else(|| tagged.first_tag()).map(from_tag).unwrap_or_default()
}

const KEPT: usize = 32;
/// The last songs read, by file and as the file was (size, modified): a song is read once for its
/// "now playing" and again when it ends, and the file is read once.
static RECENT: Mutex<Vec<(String, u64, Option<SystemTime>, Ids)>> = Mutex::new(Vec::new());

/// The ids of a song. A song of a single-file album (CUE) has none: its file's tags are the album's.
pub fn of_track(t: &crate::library::Track) -> Ids {
    if t.cue.is_some() || t.path.is_empty() {
        return Ids::default();
    }
    let meta = std::fs::metadata(&t.path).ok();
    let stamp = (meta.as_ref().map(|m| m.len()).unwrap_or(0), meta.and_then(|m| m.modified().ok()));
    {
        let recent = RECENT.lock();
        if let Some((_, _, _, ids)) = recent.iter().find(|(p, n, m, _)| *p == t.path && (*n, *m) == stamp) {
            return ids.clone();
        }
    }
    let ids = read(Path::new(&t.path));
    let mut recent = RECENT.lock();
    recent.retain(|(p, ..)| *p != t.path);
    if recent.len() >= KEPT {
        recent.remove(0);
    }
    recent.push((t.path.clone(), stamp.0, stamp.1, ids.clone()));
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    const REC: &str = "98255A8C-017a-4bc7-8dd6-1fa36124572b";

    #[test]
    fn only_well_formed_ids_are_taken() {
        assert!(is_mbid(REC));
        assert!(!is_mbid("98255a8c-017a-4bc7-8dd6-1fa36124572"));
        assert!(!is_mbid("98255a8c017a4bc78dd61fa36124572bxxxx"));
        assert_eq!(ids_in(&format!("{REC}; db92a151-1ac2-438b-bc43-b82e149ddd50 / junk")), ["98255a8c-017a-4bc7-8dd6-1fa36124572b", "db92a151-1ac2-438b-bc43-b82e149ddd50"]);
    }

    #[test]
    fn a_tag_gives_its_recording_release_and_artists() {
        let mut tag = Tag::new(lofty::tag::TagType::VorbisComments);
        tag.insert_text(ItemKey::MusicBrainzRecordingId, REC.into());
        tag.insert_text(ItemKey::MusicBrainzReleaseId, "bf9e91ea-8029-4a04-a26a-224e00a83266".into());
        tag.push(lofty::tag::TagItem::new(ItemKey::MusicBrainzArtistId, lofty::tag::ItemValue::Text("db92a151-1ac2-438b-bc43-b82e149ddd50".into())));
        tag.push(lofty::tag::TagItem::new(ItemKey::MusicBrainzArtistId, lofty::tag::ItemValue::Text("not an id".into())));
        let ids = from_tag(&tag);
        assert_eq!(ids.recording.as_deref(), Some("98255a8c-017a-4bc7-8dd6-1fa36124572b"));
        assert_eq!(ids.release.as_deref(), Some("bf9e91ea-8029-4a04-a26a-224e00a83266"));
        assert_eq!(ids.artists, ["db92a151-1ac2-438b-bc43-b82e149ddd50"]);
        assert!(from_tag(&Tag::new(lofty::tag::TagType::VorbisComments)).is_empty());
    }

    #[test]
    fn a_cue_song_or_a_missing_file_has_no_ids() {
        let t = crate::library::Track { path: "Z:/nowhere/x.flac".into(), ..Default::default() };
        assert!(of_track(&t).is_empty());
    }
}
