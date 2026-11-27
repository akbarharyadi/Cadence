//! What Discover tells the AI about the listener, beyond the month's top artists and songs: the
//! library's artists in the order they're played (so the ones it must not suggest songs of
//! blindly come first, not the alphabet's first names), and their favourite songs (hearted or
//! rated 4 stars or more). Plain functions on the library, history counts and marks.

use std::collections::HashMap;

use crate::library::Track;
use crate::marks::Mark;

/// Library artists sent to the AI.
pub const OWNED_ARTISTS: usize = 150;
/// Favourite songs sent to the AI.
pub const FAVORITES: usize = 20;
/// Fewer picks than this left after the filters: the AI is asked once more.
pub const ENOUGH_PICKS: usize = 8;
/// Rounds of asking at most (the first and one more).
pub const ROUNDS: usize = 2;

/// Every artist in the library, the most played first (all-time plays), then by how many of
/// their songs the library has; each name once, ignoring case.
pub fn artists_by_plays(tracks: &[Track], plays: &HashMap<String, u32>) -> Vec<String> {
    let mut by: HashMap<String, (String, u32, u32)> = HashMap::new();
    for t in tracks.iter().filter(|t| !t.artist.trim().is_empty()) {
        let e = by.entry(t.artist.to_lowercase()).or_insert_with(|| (t.artist.clone(), 0, 0));
        e.1 = e.1.saturating_add(plays.get(&t.id).copied().unwrap_or(0));
        e.2 += 1;
    }
    let mut v: Vec<(String, u32, u32)> = by.into_values().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then_with(|| a.0.cmp(&b.0)));
    v.into_iter().map(|(name, _, _)| name).collect()
}

/// The listener's favourite songs ("Artist - Title"): hearted or rated 4 stars or more, the most
/// played first, at most `FAVORITES`.
pub fn favorites(tracks: &[Track], marks: &HashMap<String, Mark>, plays: &HashMap<String, u32>) -> Vec<String> {
    let mut v: Vec<(&Track, u32, u8)> = tracks
        .iter()
        .filter_map(|t| marks.get(&t.id).filter(|m| m.loved || m.rating >= 4).map(|m| (t, plays.get(&t.id).copied().unwrap_or(0), m.rating)))
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then_with(|| a.0.title.cmp(&b.0.title)));
    let mut out: Vec<String> = Vec::new();
    for (t, _, _) in v {
        let name = format!("{} - {}", t.artist, t.title);
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            out.push(name);
        }
        if out.len() >= FAVORITES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, artist: &str, title: &str) -> Track {
        Track { id: id.into(), artist: artist.into(), title: title.into(), ..Default::default() }
    }

    #[test]
    fn the_artists_played_most_come_first_not_the_alphabet() {
        let tracks = [song("1", "2 Unlimited", "a"), song("2", "Aimer", "b"), song("3", "Linkin Park", "c"), song("4", "aimer", "d"), song("5", "ABBA", "e"), song("6", "ABBA", "f")];
        let plays: HashMap<String, u32> = [("3".into(), 5), ("2".into(), 3), ("4".into(), 4)].into();
        assert_eq!(artists_by_plays(&tracks, &plays), ["Aimer", "Linkin Park", "ABBA", "2 Unlimited"]);
    }

    #[test]
    fn favorites_are_hearted_or_well_rated_songs_most_played_first() {
        let tracks = [song("1", "Aimer", "closer"), song("2", "LiSA", "Homura"), song("3", "X", "Meh"), song("4", "EGOIST", "Door")];
        let marks: HashMap<String, Mark> = [
            ("1".into(), Mark { rating: 0, loved: true }),
            ("2".into(), Mark { rating: 5, loved: false }),
            ("3".into(), Mark { rating: 3, loved: false }),
            ("4".into(), Mark { rating: 4, loved: false }),
        ]
        .into();
        let plays: HashMap<String, u32> = [("1".into(), 9), ("4".into(), 2)].into();
        assert_eq!(favorites(&tracks, &marks, &plays), ["Aimer - closer", "EGOIST - Door", "LiSA - Homura"]);
    }

    #[test]
    fn favorites_are_capped_and_listed_once() {
        let tracks: Vec<Track> = (0..40).map(|i| song(&i.to_string(), "A", if i % 2 == 0 { "Same" } else { "Other" })).chain((0..30).map(|i| song(&format!("x{i}"), "B", &format!("Song {i}")))).collect();
        let marks: HashMap<String, Mark> = tracks.iter().map(|t| (t.id.clone(), Mark { rating: 5, loved: true })).collect();
        let f = favorites(&tracks, &marks, &HashMap::new());
        assert_eq!(f.len(), FAVORITES);
        assert_eq!(f.iter().filter(|s| *s == "A - Same").count(), 1);
    }
}
