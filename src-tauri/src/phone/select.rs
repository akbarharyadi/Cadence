//! Which songs go on the phone, from the listener's choices (`Choice`): the songs of kept
//! hand-made playlists and albums, of kept artists, and the most played songs.
//!
//! A playlist or album is an explicit choice, so its skip-marked songs go too; an artist or the
//! most played list leaves skip-marked songs out. Songs of single-file albums (CUE sheets) can't
//! be sent one by one yet and are counted instead.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;

use super::Choice;
use crate::history::PlayEvent;
use crate::identity::Identity;
use crate::library::Track;
use crate::playlists::Playlist;

/// A play counts toward "most played" when it lasted past the song's middle.
const PLAYED_SHARE: f64 = 0.5;
/// "Most played" looks this far back.
const TOP_WINDOW_SECS: u64 = 365 * 86_400;

/// The songs to keep on the phone.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Picked {
    /// Track ids, each once, in the library's order.
    pub songs: Vec<String>,
    /// Songs of single-file albums that were chosen but can't be sent yet.
    pub left_out: usize,
    /// Hand-made playlists among the choices that no longer exist, or are smart playlists
    /// (their rules are worked out by the window, not here).
    pub not_sent_playlists: usize,
}

/// What `pick` reads.
pub struct Input<'a> {
    pub tracks: &'a [Track],
    pub playlists: &'a [Playlist],
    pub events: &'a [PlayEvent],
    pub skipped: &'a BTreeSet<String>,
    pub ids: &'a Identity,
    pub now: u64,
}

/// The songs the choices name.
pub fn pick(c: &Choice, i: &Input) -> Picked {
    let mut chosen: HashSet<&str> = HashSet::new();
    let mut not_sent_playlists = 0;
    for id in &c.playlists {
        match i.playlists.iter().find(|p| &p.id == id) {
            Some(p) if p.smart.is_none() => chosen.extend(p.track_ids.iter().map(String::as_str)),
            _ => not_sent_playlists += 1,
        }
    }
    let skip = |t: &Track| i.skipped.contains(&t.id);
    for t in i.tracks {
        if c.albums.contains(&t.album_id) || (!skip(t) && by_kept_artist(c, i.ids, t)) {
            chosen.insert(&t.id);
        }
    }
    let present: HashMap<&str, &Track> = i.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    for id in most_played(i.events, i.now, c.top as usize, |id| present.get(id).is_some_and(|t| !skip(t))) {
        chosen.insert(id);
    }
    let mut out = Picked { not_sent_playlists, ..Default::default() };
    for t in i.tracks.iter().filter(|t| chosen.contains(t.id.as_str())) {
        if t.cue.is_some() {
            out.left_out += 1;
        } else {
            out.songs.push(t.id.clone());
        }
    }
    out
}

/// The song is by a kept artist (as the main credit, or as the album's artist).
fn by_kept_artist(c: &Choice, ids: &Identity, t: &Track) -> bool {
    !c.artists.is_empty()
        && (c.artists.contains(&ids.primary_key(&t.artist)) || (!t.album_artist.is_empty() && c.artists.contains(&ids.primary_key(&t.album_artist))))
}

/// The `n` songs played most (past their middle) in the last year, most first; ties go to the
/// one played last. Only songs `wanted` says yes to.
fn most_played(events: &[PlayEvent], now: u64, n: usize, wanted: impl Fn(&str) -> bool) -> Vec<&str> {
    if n == 0 {
        return Vec::new();
    }
    let since = now.saturating_sub(TOP_WINDOW_SECS);
    let mut count: HashMap<&str, (u32, u64)> = HashMap::new();
    for e in events.iter().filter(|e| e.at >= since && !e.skipped && e.duration > 0.0 && e.listened >= e.duration * PLAYED_SHARE) {
        let c = count.entry(e.track_id.as_str()).or_default();
        c.0 += 1;
        c.1 = c.1.max(e.at);
    }
    let mut list: Vec<(&str, (u32, u64))> = count.into_iter().filter(|(id, _)| wanted(id)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    list.into_iter().take(n).map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, artist: &str, album_id: &str) -> Track {
        Track { id: id.into(), artist: artist.into(), album_id: album_id.into(), path: format!("C:/m/{id}.flac"), duration: 200.0, ..Default::default() }
    }

    fn play(id: &str, at: u64, listened: f64) -> PlayEvent {
        PlayEvent { track_id: id.into(), album_id: String::new(), at, listened, duration: 200.0, skipped: false }
    }

    fn run(c: &Choice, tracks: &[Track], playlists: &[Playlist], events: &[PlayEvent], skipped: &[&str]) -> Picked {
        let skipped: BTreeSet<String> = skipped.iter().map(|s| s.to_string()).collect();
        let ids = Identity::default();
        pick(c, &Input { tracks, playlists, events, skipped: &skipped, ids: &ids, now: 1_000_000_000 })
    }

    #[test]
    fn a_kept_album_brings_all_its_songs_even_skipped_ones() {
        let tracks = [song("a", "X", "al1"), song("b", "X", "al1"), song("c", "Y", "al2")];
        let c = Choice { albums: ["al1".to_string()].into(), ..Default::default() };
        assert_eq!(run(&c, &tracks, &[], &[], &["b"]).songs, ["a", "b"]);
    }

    #[test]
    fn a_kept_artist_leaves_skipped_songs_out() {
        let tracks = [song("a", "Aimer", "al1"), song("b", "Aimer", "al2"), song("c", "LiSA", "al3")];
        let c = Choice { artists: [Identity::default().primary_key("Aimer")].into(), ..Default::default() };
        assert_eq!(run(&c, &tracks, &[], &[], &["b"]).songs, ["a"]);
    }

    #[test]
    fn a_hand_made_playlist_is_sent_and_a_smart_one_is_counted() {
        let tracks = [song("a", "X", "1"), song("b", "Y", "2")];
        let hand = Playlist { id: "p1".into(), track_ids: vec!["b".into(), "gone".into()], ..Default::default() };
        let smart = Playlist { id: "p2".into(), smart: Some(serde_json::json!({})), ..Default::default() };
        let c = Choice { playlists: ["p1".to_string(), "p2".to_string(), "p3".to_string()].into(), ..Default::default() };
        let p = run(&c, &tracks, &[hand, smart], &[], &[]);
        assert_eq!(p.songs, ["b"]);
        assert_eq!(p.not_sent_playlists, 2);
    }

    #[test]
    fn most_played_counts_plays_past_the_middle_in_the_last_year() {
        let tracks = [song("a", "X", "1"), song("b", "X", "1"), song("c", "X", "1"), song("d", "X", "1")];
        let now = 1_000_000_000;
        let events = [
            play("a", now - 10, 190.0),
            play("a", now - 20, 190.0),
            play("b", now - 10, 50.0),            // left early: doesn't count
            play("c", now - 400 * 86_400, 200.0), // over a year ago
            play("d", now - 30, 150.0),
            play("gone", now - 5, 200.0), // not in the library any more
        ];
        let c = Choice { top: 100, ..Default::default() };
        assert_eq!(run(&c, &tracks, &[], &events, &[]).songs, ["a", "d"]);
        let c = Choice { top: 1, ..Default::default() };
        assert_eq!(run(&c, &tracks, &[], &events, &[]).songs, ["a"]);
        assert!(run(&c, &tracks, &[], &events, &["a"]).songs == ["d"], "a skip-marked song isn't a top song");
    }

    #[test]
    fn songs_of_single_file_albums_are_counted_not_sent() {
        let mut cue = song("a", "X", "al1");
        cue.cue = Some(crate::cue::Span { start: 0.0, end: None });
        let tracks = [cue, song("b", "X", "al1")];
        let c = Choice { albums: ["al1".to_string()].into(), ..Default::default() };
        let p = run(&c, &tracks, &[], &[], &[]);
        assert_eq!((p.songs.len(), p.left_out), (1, 1));
    }

    #[test]
    fn nothing_chosen_sends_nothing() {
        let tracks = [song("a", "X", "1")];
        assert!(run(&Choice::default(), &tracks, &[], &[play("a", 999_999_990, 200.0)], &[]).songs.is_empty());
    }
}
