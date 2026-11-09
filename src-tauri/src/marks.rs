//! Favourites (hearts) and star ratings.
//!
//! Kept by song id like everything else, but each entry also remembers what its song is (title,
//! artist, album, length). When a file is moved or renamed its id changes; the entry then follows
//! the song to its new id instead of being lost. An entry whose song can't be found is kept, never
//! dropped (a drive that isn't plugged in must not wipe your ratings).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::library::Track;
use crate::state::match_key;

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Entry {
    /// 0 = not rated, else 1–5 stars.
    rating: u8,
    loved: bool,
    title: String,
    artist: String,
    album: String,
    duration: f64,
}

impl Entry {
    fn of(t: &Track) -> Self {
        Self {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration: t.duration,
            ..Default::default()
        }
    }

    fn is_empty(&self) -> bool {
        self.rating == 0 && !self.loved
    }
}

/// What the UI needs per song.
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub rating: u8,
    pub loved: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Marks {
    items: HashMap<String, Entry>,
}

impl Marks {
    pub fn view(&self) -> HashMap<String, Mark> {
        self.items
            .iter()
            .map(|(id, e)| (id.clone(), Mark { rating: e.rating, loved: e.loved }))
            .collect()
    }

    pub fn set_rating(&mut self, t: &Track, rating: u8) {
        let e = self.items.entry(t.id.clone()).or_insert_with(|| Entry::of(t));
        e.rating = rating.min(5);
        self.tidy(&t.id);
    }

    pub fn set_loved(&mut self, t: &Track, loved: bool) {
        let e = self.items.entry(t.id.clone()).or_insert_with(|| Entry::of(t));
        e.loved = loved;
        self.tidy(&t.id);
    }

    /// Entries with nothing set are dropped.
    fn tidy(&mut self, id: &str) {
        if self.items.get(id).is_some_and(Entry::is_empty) {
            self.items.remove(id);
        }
    }

    /// A removed copy's favourite and rating go to the copy that stays (the higher rating, a
    /// favourite if either was). True when anything changed.
    pub fn pass_on(&mut self, from: &str, to: &Track) -> bool {
        let Some(old) = self.items.remove(from) else { return false };
        let e = self.items.entry(to.id.clone()).or_insert_with(|| Entry::of(to));
        e.rating = e.rating.max(old.rating);
        e.loved |= old.loved;
        true
    }

    /// Entries whose song id is gone from the library follow the same song under its new id (same
    /// title and length, the same artist and album preferred), unless that song already has its
    /// own mark. Returns how many moved.
    pub fn relink(&mut self, tracks: &[Track]) -> usize {
        let known: std::collections::HashSet<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        let gone: Vec<String> = self.items.keys().filter(|id| !known.contains(id.as_str())).cloned().collect();
        if gone.is_empty() {
            return 0;
        }
        let mut by_title: HashMap<String, Vec<&Track>> = HashMap::new();
        for t in tracks {
            by_title.entry(match_key(&t.title)).or_default().push(t);
        }
        let mut moved = 0;
        for old_id in gone {
            let Some(entry) = self.items.get(&old_id) else { continue };
            let best = by_title
                .get(&match_key(&entry.title))
                .into_iter()
                .flatten()
                .filter(|t| (t.duration - entry.duration).abs() < 2.5 && !self.items.contains_key(&t.id))
                .min_by_key(|t| {
                    (
                        match_key(&t.artist) != match_key(&entry.artist),
                        match_key(&t.album) != match_key(&entry.album),
                        ((t.duration - entry.duration).abs() * 1000.0) as u64,
                    )
                });
            let Some(new) = best else { continue };
            let mut entry = self.items.remove(&old_id).unwrap();
            entry.title = new.title.clone();
            entry.artist = new.artist.clone();
            entry.album = new.album.clone();
            entry.duration = new.duration;
            self.items.insert(new.id.clone(), entry);
            moved += 1;
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, title: &str, artist: &str, album: &str, duration: f64) -> Track {
        Track { id: id.into(), title: title.into(), artist: artist.into(), album: album.into(), duration, ..Default::default() }
    }

    #[test]
    fn empty_entries_are_dropped() {
        let t = song("a", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        let mut m = Marks::default();
        m.set_rating(&t, 4);
        m.set_loved(&t, true);
        assert_eq!(m.view()["a"].rating, 4);
        m.set_rating(&t, 0);
        assert!(m.view()["a"].loved);
        m.set_loved(&t, false);
        assert!(m.view().is_empty());
        m.set_rating(&t, 9);
        assert_eq!(m.view()["a"].rating, 5);
    }

    #[test]
    fn marks_follow_a_song_that_moved() {
        let old = song("old", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        let mut m = Marks::default();
        m.set_rating(&old, 5);
        m.set_loved(&old, true);
        let library = vec![
            song("other", "Lemon", "Someone Else", "Cover", 300.0),
            song("new", "Lemon", "Kenshi Yonezu", "Lemon", 255.4),
        ];
        assert_eq!(m.relink(&library), 1);
        let v = m.view();
        assert!(!v.contains_key("old"));
        assert_eq!((v["new"].rating, v["new"].loved), (5, true));
    }

    #[test]
    fn marks_of_missing_songs_are_kept() {
        let old = song("old", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        let mut m = Marks::default();
        m.set_loved(&old, true);
        // The drive is unplugged: nothing like it in the library.
        assert_eq!(m.relink(&[song("x", "Other", "A", "B", 100.0)]), 0);
        assert!(m.view()["old"].loved);
        // A song that already has its own mark isn't overwritten.
        let mut m = Marks::default();
        m.set_loved(&old, true);
        let twin = song("twin", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        m.set_rating(&twin, 2);
        assert_eq!(m.relink(&[twin]), 0);
        assert_eq!(m.view()["twin"].rating, 2);
    }
}
