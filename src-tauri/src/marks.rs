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

/// A removed copy's mark handed to the copy that stayed (`Marks::pass_on`), kept with the change so
/// Undo can give it back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassedOn {
    /// The removed copy's song id, and its mark.
    pub from: String,
    entry: Entry,
    /// The copy that stayed, its mark before and after.
    pub to: String,
    to_before: Option<Entry>,
    to_after: Entry,
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
    /// favourite if either was). Returns what was done, to undo it (`take_back`); None when the
    /// removed copy had no mark.
    pub fn pass_on(&mut self, from: &str, to: &Track) -> Option<PassedOn> {
        let old = self.items.remove(from)?;
        let to_before = self.items.get(&to.id).cloned();
        let e = self.items.entry(to.id.clone()).or_insert_with(|| Entry::of(to));
        e.rating = e.rating.max(old.rating);
        e.loved |= old.loved;
        Some(PassedOn { from: from.to_string(), entry: old, to: to.id.clone(), to_before, to_after: e.clone() })
    }

    /// Undoes a `pass_on` (the removed copy is back): its mark returns, and the copy that stayed
    /// gets its own back, unless it has been changed since (then the listener's change stays).
    pub fn take_back(&mut self, p: &PassedOn) {
        self.items.entry(p.from.clone()).or_insert_with(|| p.entry.clone());
        if p.to != p.from && self.items.get(&p.to) == Some(&p.to_after) {
            match &p.to_before {
                Some(e) => {
                    self.items.insert(p.to.clone(), e.clone());
                }
                None => {
                    self.items.remove(&p.to);
                }
            }
        }
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
    fn undoing_a_removal_gives_both_copies_their_own_marks_back() {
        let gone = song("lossy", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        let kept = song("flac", "Lemon", "Kenshi Yonezu", "Lemon", 255.2);
        let mut m = Marks::default();
        m.set_rating(&gone, 5);
        m.set_loved(&gone, true);
        m.set_rating(&kept, 2);
        let p = m.pass_on("lossy", &kept).expect("it had a mark");
        assert_eq!((m.view()["flac"].rating, m.view()["flac"].loved), (5, true));
        assert!(!m.view().contains_key("lossy"));
        // Kept with the change on disk and read back.
        let p: PassedOn = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        m.take_back(&p);
        let v = m.view();
        assert_eq!((v["lossy"].rating, v["lossy"].loved), (5, true));
        assert_eq!((v["flac"].rating, v["flac"].loved), (2, false));

        // The kept copy had no mark of its own: it has none again. One changed since keeps the change.
        let mut m = Marks::default();
        m.set_rating(&gone, 4);
        let p = m.pass_on("lossy", &kept).unwrap();
        let mut changed = Marks { items: m.items.clone() };
        m.take_back(&p);
        assert!(!m.view().contains_key("flac"));
        changed.set_loved(&kept, true);
        changed.take_back(&p);
        assert_eq!((changed.view()["flac"].rating, changed.view()["flac"].loved), (4, true));
        assert_eq!(changed.view()["lossy"].rating, 4);
        // Nothing to hand on.
        assert!(Marks::default().pass_on("lossy", &kept).is_none());
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
