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

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Entry {
    /// 0 = not rated, else 1–5 stars.
    rating: u8,
    loved: bool,
    title: String,
    artist: String,
    album: String,
    duration: f64,
    /// The song's file (empty: a song of a single-file album, or a mark made before this was
    /// kept): a song whose file is only out of reach isn't followed to a look-alike.
    #[serde(skip_serializing_if = "String::is_empty")]
    path: String,
}

/// The same mark of the same song (where its file is doesn't matter: `take_back` compares marks
/// saved with a change before files were kept).
impl PartialEq for Entry {
    fn eq(&self, o: &Self) -> bool {
        (self.rating, self.loved, &self.title, &self.artist, &self.album) == (o.rating, o.loved, &o.title, &o.artist, &o.album) && self.duration == o.duration
    }
}

impl Entry {
    fn of(t: &Track) -> Self {
        Self {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration: t.duration,
            path: if t.cue.is_none() { t.path.clone() } else { String::new() },
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

    /// Notes the file of entries made before marks kept it (`lookup`: the library's song of an id).
    /// Returns how many.
    pub fn note_paths<'a>(&mut self, lookup: impl Fn(&str) -> Option<&'a Track>) -> usize {
        let mut noted = 0;
        for (id, e) in self.items.iter_mut().filter(|(_, e)| e.path.is_empty()) {
            if let Some(t) = lookup(id).filter(|t| t.cue.is_none()) {
                e.path = t.path.clone();
                noted += 1;
            }
        }
        noted
    }

    /// Each marked song as the mark remembers it (id, title, artist, album, length, file), for
    /// finding it again after a restore (`backup::relink`).
    pub(crate) fn as_tracks(&self) -> Vec<Track> {
        self.items
            .iter()
            .map(|(id, e)| Track {
                id: id.clone(),
                title: e.title.clone(),
                artist: e.artist.clone(),
                album: e.album.clone(),
                duration: e.duration,
                path: e.path.clone(),
                ..Default::default()
            })
            .collect()
    }

    /// Moves entries to their song's new id (old → new, `lookup`: the library's song of an id),
    /// unless the new id has a mark of its own. Returns how many moved.
    pub(crate) fn follow<'a>(&mut self, moved: &HashMap<String, String>, lookup: impl Fn(&str) -> Option<&'a Track>) -> usize {
        let mut n = 0;
        for (old, new) in moved {
            if old == new || self.items.contains_key(new) || !self.items.contains_key(old) {
                continue;
            }
            let Some(t) = lookup(new) else { continue };
            let Some(mut e) = self.items.remove(old) else { continue };
            let fresh = Entry::of(t);
            (e.title, e.artist, e.album, e.duration, e.path) = (fresh.title, fresh.artist, fresh.album, fresh.duration, fresh.path);
            self.items.insert(new.clone(), e);
            n += 1;
        }
        n
    }

    /// Whether some entry's song isn't in the library (`known`: whether an id is).
    pub fn has_gone(&self, known: impl Fn(&str) -> bool) -> bool {
        self.items.keys().any(|id| !known(id))
    }

    /// Entries whose song id is gone from the library follow the song to its new id when it's
    /// certain which song that is (`state::Finder`: a move Cadence made, a file replaced in its
    /// folder, or the one song with the same title, artist and length), unless that song already
    /// has its own mark. A cover by another artist, or one of several look-alikes, is not it: the
    /// entry stays under its old id. Returns how many moved.
    pub(crate) fn relink(&mut self, finder: &crate::state::Finder) -> usize {
        use crate::state::Found;
        // (old id, where it is now, how sure, its album)
        let found: Vec<(String, &Track, Found, String)> = self
            .items
            .iter()
            .filter(|(id, _)| finder.track(id).is_none())
            .filter_map(|(id, e)| {
                let was = crate::state::Was {
                    id,
                    path: &e.path,
                    cue: e.path.is_empty(),
                    title: &e.title,
                    artist: &e.artist,
                    album: &e.album,
                    duration: e.duration,
                };
                finder.find(&was).map(|(new, how)| (id.clone(), new, how, match_key(&e.album)))
            })
            .collect();
        let mut moved = 0;
        for (old_id, new, how, album) in &found {
            // Two songs of different albums can't both be this one: a guess isn't followed then.
            if *how == Found::Same && found.iter().any(|(o, n, _, a)| n.id == new.id && o != old_id && a != album) {
                continue;
            }
            if self.items.contains_key(&new.id) {
                continue;
            }
            let Some(mut entry) = self.items.remove(old_id.as_str()) else { continue };
            entry.title = new.title.clone();
            entry.artist = new.artist.clone();
            entry.album = new.album.clone();
            entry.duration = new.duration;
            entry.path = if new.cue.is_none() { new.path.clone() } else { String::new() };
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
        assert_eq!(relink(&mut m, &library), 1);
        let v = m.view();
        assert!(!v.contains_key("old"));
        assert_eq!((v["new"].rating, v["new"].loved), (5, true));
    }

    fn relink(m: &mut Marks, library: &[Track]) -> usize {
        m.relink(&crate::state::Finder::new(library, &[], Default::default()))
    }

    #[test]
    fn marks_never_go_to_another_artists_song_or_a_look_alike() {
        // A cover by someone else, of the same title and length: not the song.
        let old = song("m-old", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        let mut m = Marks::default();
        m.set_loved(&old, true);
        assert_eq!(relink(&mut m, &[song("m-cover", "Lemon", "Someone Else", "Covers", 255.0)]), 0);
        assert!(m.view()["m-old"].loved);
        // Another album's "Intro" by the same artist, two of them: either could be it, so neither.
        let intro = song("m-intro", "Intro", "YOASOBI", "THE BOOK", 90.0);
        let mut m = Marks::default();
        m.set_rating(&intro, 4);
        let others = [song("m-i2", "Intro", "YOASOBI", "THE BOOK 2", 90.4), song("m-i3", "Intro", "YOASOBI", "THE BOOK 3", 91.0)];
        assert_eq!(relink(&mut m, &others), 0);
        assert_eq!(m.view()["m-intro"].rating, 4);
        // The same artist under a credit with a guest: followed.
        assert_eq!(relink(&mut m, &[song("m-i4", "Intro", "YOASOBI feat. Ayase", "THE BOOK", 90.2)]), 1);
        assert_eq!(m.view()["m-i4"].rating, 4);
    }

    #[test]
    fn a_mark_whose_drive_is_unplugged_stays() {
        // Its music folder can't be reached: the song isn't gone, only out of reach.
        let base = std::env::temp_dir().join(format!("cadence-marks-offline-{}", std::process::id()));
        let folder = base.join("Unplugged");
        let mut old = song("m-off", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        old.path = folder.join("Lemon.flac").to_string_lossy().into_owned();
        let mut m = Marks::default();
        m.set_loved(&old, true);
        let elsewhere = [song("m-elsewhere", "Lemon", "Kenshi Yonezu", "Lemon", 255.0)];
        let finder = crate::state::Finder::new(&elsewhere, &[folder.to_string_lossy().into_owned()], Default::default());
        assert_eq!(m.relink(&finder), 0);
        assert!(m.view()["m-off"].loved);
        // A mark saved before files were kept still reads, and learns its file.
        let mut m: Marks = serde_json::from_str(r#"{"items":{"m-n":{"rating":3,"loved":false,"title":"T","artist":"A","album":"B","duration":100.0}}}"#).unwrap();
        let mut t = song("m-n", "T", "A", "B", 100.0);
        t.path = "D:/Music/T.flac".into();
        assert_eq!(m.note_paths(|id| (id == "m-n").then_some(&t)), 1);
        assert_eq!(m.items["m-n"].path, "D:/Music/T.flac");
        assert_eq!(m.note_paths(|id| (id == "m-n").then_some(&t)), 0, "once");
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
        assert_eq!(relink(&mut m, &[song("x", "Other", "A", "B", 100.0)]), 0);
        assert!(m.view()["old"].loved);
        // A song that already has its own mark isn't overwritten.
        let mut m = Marks::default();
        m.set_loved(&old, true);
        let twin = song("twin", "Lemon", "Kenshi Yonezu", "Lemon", 255.0);
        m.set_rating(&twin, 2);
        assert_eq!(relink(&mut m, &[twin]), 0);
        assert_eq!(m.view()["twin"].rating, 2);
    }
}
