//! Shuffle orders.
//!
//! A plain Fisher–Yates shuffle is fair, but it doesn't feel random: one artist comes up three
//! times in a row, or a song heard an hour ago comes straight back. The balanced shuffle (after
//! Martin Fiedler's "balanced shuffle", and Spotify's "fewer repeats") instead:
//!
//! 1. groups the songs by artist (one artist across scripts and "feat." credits, `artist_key`);
//! 2. spreads each artist's songs evenly over the whole order, from a random offset and with a
//!    little jitter (within an artist, its albums are spread the same way);
//! 3. pushes songs played recently towards the end (by how recently: the play history);
//! 4. moves a song that would follow one by the same artist (or from the same album) a few places
//!    on, where the mix allows it;
//! 5. makes a few such orders and keeps the one that does best on 3 and 4.
//!
//! Everything is deterministic for a seed, and takes a few milliseconds for 30 000 songs.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::state::Aliases;

/// Which shuffle plays (a setting).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShuffleMode {
    /// Artists spread out, recently played songs later (see the module).
    #[default]
    Balanced,
    /// Every order equally likely (Fisher–Yates).
    Random,
}

/// Small, fast generator (SplitMix64): the same seed gives the same order.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// In 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() >> 32) * n as u64 >> 32) as usize
    }
}

/// A seed from the clock, for orders that should differ every time.
pub fn clock_seed() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

/// What the balanced shuffle needs to know about a song.
#[derive(Clone, Copy)]
pub struct Entry<'a> {
    /// `artist_key` of the song's artist.
    pub artist: &'a str,
    pub album: &'a str,
    /// Unix seconds of its last play (0: not in the recent history).
    pub last_played: u64,
}

/// How strongly a song counts as recently played: 1 just now, half after a day and a bit, close to
/// nothing after a week.
pub fn recency(last_played: u64, now: u64) -> f32 {
    if last_played == 0 {
        return 0.0;
    }
    let hours = now.saturating_sub(last_played) as f32 / 3600.0;
    (-hours / 40.0).exp()
}

/// Every order equally likely: `order[k]` is the index of the song that plays `k`th.
pub fn random(n: usize, seed: u64) -> Vec<usize> {
    let mut rng = Rng::new(seed);
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        order.swap(i, rng.below(i + 1));
    }
    order
}

/// How many orders are tried (the best is kept).
const CANDIDATES: usize = 3;
/// How far on a song is looked for to separate two by the same artist.
const WINDOW: usize = 48;
/// How far a song played just now is pushed towards the end (0..1 of the way left).
const FRESH_PUSH: f32 = 0.55;

/// A song's artist and album as small numbers (comparing and sorting those is quick).
#[derive(Clone, Copy, PartialEq)]
struct Ids {
    artist: u32,
    album: u32,
}

/// Not anyone's: for a `lead` song whose artist or album none of the songs has.
const NOBODY: u32 = u32::MAX;

/// A quick hasher for the names (FxHash's step): they come from this computer, not an attacker.
#[derive(Default)]
struct Quick(u64);

impl std::hash::Hasher for Quick {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(0x517C_C1B7_2722_0A95);
        }
    }
}

type QuickMap<'a> = HashMap<&'a str, u32, std::hash::BuildHasherDefault<Quick>>;

/// Numbers for names, in the order they first come (so the seed alone decides the order).
fn number<'a>(map: &mut QuickMap<'a>, name: &'a str) -> u32 {
    let next = map.len() as u32;
    *map.entry(name).or_insert(next)
}

/// The balanced order of `entries` (see the module): `order[k]` is the index of the song that
/// plays `k`th. `lead`: the song playing before them (artist, album), which the first one
/// shouldn't share an artist with.
pub fn balanced(entries: &[Entry], lead: Option<(&str, &str)>, now: u64, seed: u64) -> Vec<usize> {
    let n = entries.len();
    if n < 2 {
        return (0..n).collect();
    }
    let mut rng = Rng::new(seed);
    let (mut artists, mut albums) = (QuickMap::default(), QuickMap::default());
    let ids: Vec<Ids> = entries
        .iter()
        .map(|e| Ids { artist: number(&mut artists, e.artist), album: number(&mut albums, e.album) })
        .collect();
    let lead = lead.map(|(artist, album)| Ids {
        artist: artists.get(artist).copied().unwrap_or(NOBODY),
        album: albums.get(album).copied().unwrap_or(NOBODY),
    });
    // Each artist's songs, in the order they'll be spread in (its albums spread among them).
    let mut groups: Vec<Vec<u32>> = vec![Vec::new(); artists.len()];
    for (i, id) in ids.iter().enumerate() {
        groups[id.artist as usize].push(i as u32);
    }
    for g in &mut groups {
        spread_albums(g, &ids, &mut rng);
    }
    let fresh: Vec<f32> = entries.iter().map(|e| recency(e.last_played, now)).collect();

    // Sorted as integers: the place (a non-negative float's bits keep its order), then the index.
    let mut keyed: Vec<u64> = Vec::with_capacity(n);
    let mut best: Option<(f32, Vec<u32>)> = None;
    for _ in 0..CANDIDATES {
        keyed.clear();
        for g in &groups {
            let m = g.len() as f32;
            let offset = rng.unit();
            for (k, &i) in g.iter().enumerate() {
                let jitter = (rng.unit() - 0.5) * 0.2;
                let mut p = (k as f32 + offset + jitter) / m;
                let w = fresh[i as usize];
                if w > 0.0 {
                    p += w * FRESH_PUSH * (1.0 - p.min(1.0));
                }
                keyed.push(u64::from(p.max(0.0).to_bits()) << 32 | u64::from(i));
            }
        }
        keyed.sort_unstable();
        let mut order: Vec<u32> = keyed.iter().map(|&k| k as u32).collect();
        separate(&mut order, &ids, lead);
        let score = penalty(&order, &ids, &fresh, lead);
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            best = Some((score, order));
        }
    }
    best.map(|(_, o)| o.into_iter().map(|i| i as usize).collect()).unwrap_or_default()
}

/// Orders one artist's songs: each album's songs in a random order, the albums spread among each
/// other the way artists are spread over the whole order.
fn spread_albums(g: &mut [u32], ids: &[Ids], rng: &mut Rng) {
    let album = |i: u32| ids[i as usize].album;
    let one_album = g.iter().all(|&i| album(i) == album(g[0]));
    if !one_album {
        g.sort_unstable_by_key(|&i| (album(i), i));
    }
    let mut keyed: Vec<u64> = Vec::with_capacity(g.len());
    let mut start = 0;
    while start < g.len() {
        let this = album(g[start]);
        let end = start + g[start..].iter().take_while(|&&i| album(i) == this).count();
        let run = &mut g[start..end];
        for i in (1..run.len()).rev() {
            run.swap(i, rng.below(i + 1));
        }
        let m = run.len() as f32;
        let offset = rng.unit();
        for (k, &i) in run.iter().enumerate() {
            let p = (k as f32 + offset + (rng.unit() - 0.5) * 0.2) / m;
            keyed.push(u64::from(p.max(0.0).to_bits()) << 32 | u64::from(i));
        }
        start = end;
    }
    if !one_album {
        keyed.sort_unstable();
    }
    for (slot, k) in g.iter_mut().zip(keyed) {
        *slot = k as u32;
    }
}

/// How badly two songs follow each other: 2 the same artist, 1 the same album, 0 neither.
fn clash(a: Ids, b: Ids) -> u8 {
    if a.artist == b.artist {
        2
    } else if a.album == b.album {
        1
    } else {
        0
    }
}

/// Where a song follows one by the same artist (or from the same album), the nearest song a little
/// further on that fits better moves up in front of it.
fn separate(order: &mut [u32], ids: &[Ids], lead: Option<Ids>) {
    let at = |order: &[u32], k: usize| ids[order[k] as usize];
    for i in 0..order.len() {
        let prev = match i {
            0 => match lead {
                Some(l) => l,
                None => continue,
            },
            _ => at(order, i - 1),
        };
        let cur = at(order, i);
        let c = clash(prev, cur);
        if c == 0 {
            continue;
        }
        let end = (i + 1 + WINDOW).min(order.len());
        let mut fallback = None;
        let mut found = None;
        for j in i + 1..end {
            let x = at(order, j);
            let with_prev = clash(prev, x);
            if with_prev == 0 && clash(x, cur) == 0 {
                found = Some(j);
                break;
            }
            if with_prev < c && fallback.is_none() {
                fallback = Some(j);
            }
        }
        if let Some(j) = found.or(fallback) {
            order[i..=j].rotate_right(1);
            continue;
        }
        // Nothing further on fits (the end of the order): the song moves back instead, to the
        // nearest gap between two songs it doesn't clash with.
        let back = (1..i.saturating_sub(1))
            .rev()
            .take(WINDOW)
            .find(|&k| clash(at(order, k - 1), cur) == 0 && clash(at(order, k), cur) == 0);
        if let Some(k) = back {
            order[k..=i].rotate_right(1);
        }
    }
}

/// Lower is better: recently played songs early, and songs by the same artist (or from the same
/// album) back to back.
fn penalty(order: &[u32], ids: &[Ids], fresh: &[f32], lead: Option<Ids>) -> f32 {
    let n = order.len() as f32;
    let mut score = 0.0;
    let mut prev = lead;
    for (k, &i) in order.iter().enumerate() {
        score += fresh[i as usize] * (1.0 - k as f32 / n);
        let id = ids[i as usize];
        if let Some(p) = prev {
            score += match clash(p, id) {
                2 => 4.0,
                1 => 1.0,
                _ => 0.0,
            };
        }
        prev = Some(id);
    }
    score
}

/// A list kept in its order except where a song follows one by the same artist (or from the same
/// album): the balanced shuffle's spacing step alone, for lists already put in order (the DJ's).
/// `order[k]` is the index of the song that plays `k`th.
pub fn spaced(artists: &[&str], albums: &[&str]) -> Vec<usize> {
    let (mut a, mut b) = (QuickMap::default(), QuickMap::default());
    let ids: Vec<Ids> = artists.iter().zip(albums).map(|(x, y)| Ids { artist: number(&mut a, x), album: number(&mut b, y) }).collect();
    let mut order: Vec<u32> = (0..ids.len() as u32).collect();
    separate(&mut order, &ids, None);
    order.into_iter().map(|i| i as usize).collect()
}

/// Puts `items` in `order` (indexes into it, as `balanced` and `random` give).
pub fn apply<T>(items: &mut Vec<T>, order: &[usize]) {
    let mut slots: Vec<Option<T>> = items.drain(..).map(Some).collect();
    items.extend(order.iter().filter_map(|&i| slots.get_mut(i).and_then(Option::take)));
    // Anything the order left out keeps its place at the end (never lost).
    items.extend(slots.into_iter().flatten());
}

/// The order that puts shuffled `ids` back the way `original` had them: indexes into `ids`, the
/// songs `original` names first (in its order), then the others where they are (songs added since).
pub fn unshuffle_order(ids: &[&str], original: &[String]) -> Vec<usize> {
    let mut at: HashMap<&str, Vec<usize>> = HashMap::with_capacity(ids.len());
    // Reversed, so a song queued twice comes back in the order of its places.
    for (i, id) in ids.iter().enumerate().rev() {
        at.entry(id).or_default().push(i);
    }
    let mut used = vec![false; ids.len()];
    let mut out = Vec::with_capacity(ids.len());
    for id in original {
        if let Some(i) = at.get_mut(id.as_str()).and_then(Vec::pop) {
            used[i] = true;
            out.push(i);
        }
    }
    out.extend((0..ids.len()).filter(|&i| !used[i]));
    out
}

/// The order to put back (`original`, song ids) as places in `queue` after `index`, for the saved
/// session: a few digits per song rather than its id again. Songs no longer queued are left out.
pub fn positions(queue: &[String], index: Option<usize>, original: &[String]) -> Vec<u32> {
    if original.is_empty() {
        return Vec::new();
    }
    let from = index.map_or(0, |i| i + 1);
    let mut at: HashMap<&str, Vec<u32>> = HashMap::with_capacity(queue.len().saturating_sub(from));
    for (i, id) in queue.iter().enumerate().skip(from).rev() {
        at.entry(id.as_str()).or_default().push(i as u32);
    }
    original.iter().filter_map(|id| at.get_mut(id.as_str()).and_then(Vec::pop)).collect()
}

/// The ids `positions` stands for in `queue`.
pub fn from_positions(queue: &[String], positions: &[u32]) -> Vec<String> {
    positions.iter().filter_map(|&p| queue.get(p as usize).cloned()).collect()
}

/// Who a song is by, as one name for the balanced shuffle: the artist leading the credit
/// ("A feat. B", "A & B", "A × B" are A's; a whole credit that is an artist, "Simon & Garfunkel",
/// stays whole), the same for every spelling of one artist (`identity`: 米津玄師 and Kenshi
/// Yonezu, "Beyoncé" and "BEYONCE").
pub fn artist_key(credit: &str, aliases: &Aliases) -> String {
    aliases.primary_key(credit)
}

/// The artists (`identity`), as last built. `data_dir` is the app's data folder, which they're
/// built from.
pub fn aliases(data_dir: &Path) -> Arc<Aliases> {
    let _ = data_dir;
    Arc::new(crate::identity::current())
}

/// How far back the history is read for "played recently".
const RECENT_PLAYS: usize = 5_000;

/// When each song last played, from the most recent plays (one pass, newest last).
pub fn recent_plays(history: &crate::history::History) -> HashMap<String, u64> {
    let events = &history.events;
    let from = events.len().saturating_sub(RECENT_PLAYS);
    let mut out = HashMap::with_capacity(events.len() - from);
    for e in &events[from..] {
        out.insert(e.track_id.clone(), e.at);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(artist: &'a str, album: &'a str, last_played: u64) -> Entry<'a> {
        Entry { artist, album, last_played }
    }

    /// A library: `spec` is (artist, songs); each artist's songs on two albums.
    fn library(spec: &[(&str, usize)]) -> (Vec<String>, Vec<String>) {
        let (mut artists, mut albums) = (Vec::new(), Vec::new());
        for (a, n) in spec {
            for k in 0..*n {
                artists.push(a.to_string());
                albums.push(format!("{a}-{}", k % 2));
            }
        }
        (artists, albums)
    }

    fn entries<'a>(artists: &'a [String], albums: &'a [String]) -> Vec<Entry<'a>> {
        artists.iter().zip(albums).map(|(a, b)| entry(a, b, 0)).collect()
    }

    fn longest_run(order: &[usize], artist: impl Fn(usize) -> String) -> usize {
        let (mut best, mut run) = (0, 0);
        for k in 0..order.len() {
            run = if k > 0 && artist(order[k]) == artist(order[k - 1]) { run + 1 } else { 1 };
            best = best.max(run);
        }
        best
    }

    #[test]
    fn same_seed_same_order_and_every_song_once() {
        let (artists, albums) = library(&[("a", 30), ("b", 20), ("c", 7), ("d", 1)]);
        let e = entries(&artists, &albums);
        let one = balanced(&e, None, 1_000, 42);
        assert_eq!(one, balanced(&e, None, 1_000, 42), "deterministic for a seed");
        assert_ne!(one, balanced(&e, None, 1_000, 43), "another seed, another order");
        let mut sorted = one.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..e.len()).collect::<Vec<_>>(), "a permutation: complete, no duplicates");
        let mut r = random(57, 9);
        assert_eq!(r, random(57, 9));
        r.sort_unstable();
        assert_eq!(r, (0..57).collect::<Vec<_>>());
    }

    #[test]
    fn an_artist_with_40_percent_never_comes_three_times_in_a_row() {
        // 40 of 100 songs by one artist: spreading them leaves room between every two.
        let mut spec = vec![("big", 40)];
        let names: Vec<String> = (0..30).map(|i| format!("small{i}")).collect();
        spec.extend(names.iter().map(|n| (n.as_str(), 2)));
        let (artists, albums) = library(&spec);
        let e = entries(&artists, &albums);
        for seed in 0..200 {
            let order = balanced(&e, None, 0, seed);
            assert!(longest_run(&order, |i| artists[i].clone()) <= 2, "seed {seed}");
            let backs = order.windows(2).filter(|w| artists[w[0]] == artists[w[1]]).count();
            assert!(backs <= 1, "seed {seed}: {backs} pairs back to back");
        }
        // A plain shuffle does it often (what this replaces).
        let runs = (0..200).filter(|&s| longest_run(&random(e.len(), s), |i| artists[i].clone()) >= 3).count();
        assert!(runs > 50, "{runs}");
    }

    #[test]
    fn the_same_album_is_kept_apart_where_it_can_be() {
        // Two artists, each on one album only: alternating is possible, so nothing repeats.
        let artists: Vec<String> = (0..20).map(|i| if i % 2 == 0 { "a" } else { "b" }.to_string()).collect();
        let albums: Vec<String> = artists.iter().map(|a| format!("{a}-album")).collect();
        let e = entries(&artists, &albums);
        for seed in 0..50 {
            let order = balanced(&e, None, 0, seed);
            assert!(order.windows(2).all(|w| albums[w[0]] != albums[w[1]]), "seed {seed}");
        }
    }

    #[test]
    fn the_song_playing_before_is_not_followed_by_its_artist() {
        let (artists, albums) = library(&[("a", 5), ("b", 5), ("c", 5)]);
        let e = entries(&artists, &albums);
        for seed in 0..50 {
            let order = balanced(&e, Some(("a", "a-0")), 0, seed);
            assert_ne!(artists[order[0]], "a", "seed {seed}");
        }
    }

    #[test]
    fn songs_played_recently_come_later() {
        let now = 1_800_000_000;
        let names: Vec<String> = (0..100).map(|i| format!("artist{i}")).collect();
        let albums: Vec<String> = names.iter().map(|n| format!("{n}-album")).collect();
        // Every tenth song played within the last couple of hours.
        let e: Vec<Entry> = (0..200)
            .map(|i| entry(&names[i % 100], &albums[i % 100], if i % 10 == 0 { now - 3600 * (i as u64 % 3) } else { 0 }))
            .collect();
        for seed in 0..20 {
            let order = balanced(&e, None, now, seed);
            let places: Vec<usize> = order.iter().enumerate().filter(|(_, i)| *i % 10 == 0).map(|(k, _)| k).collect();
            let late = places.iter().filter(|&&k| k >= 100).count();
            assert!(late >= 17, "seed {seed}: only {late} of 20 in the second half");
            let mean = places.iter().sum::<usize>() as f32 / places.len() as f32;
            assert!(mean > 120.0, "seed {seed}: {mean}");
        }
    }

    #[test]
    fn one_artist_in_two_scripts_is_one_group() {
        let aliases = crate::identity::Identity::from_pairs(&[("米津玄師", "Kenshi Yonezu")]);
        let a = artist_key("米津玄師", &aliases);
        assert_eq!(a, artist_key("Kenshi Yonezu", &aliases));
        assert_eq!(a, artist_key("Kenshi Yonezu feat. Hatsune Miku", &aliases));
        assert_eq!(a, artist_key("米津玄師 & 菅田将暉", &aliases));
        assert_eq!(artist_key("Beyoncé", &aliases), artist_key("BEYONCE", &aliases));
        assert_eq!(artist_key("REOL × Giga", &aliases), artist_key("Reol", &aliases));
        assert_ne!(artist_key("YOASOBI", &aliases), a);
        // Without the pair the names stay apart (nothing is guessed).
        assert_ne!(artist_key("米津玄師", &Aliases::new()), artist_key("Kenshi Yonezu", &Aliases::new()));

        // A third of the songs, under either name: as one artist they're spread like one.
        let keys: Vec<String> = (0..60)
            .map(|i| match i % 6 {
                0 => artist_key("米津玄師", &aliases),
                1 => artist_key("Kenshi Yonezu", &aliases),
                _ => format!("other{}", i % 12),
            })
            .collect();
        let albums: Vec<String> = (0..60).map(|i| format!("al{i}")).collect();
        let e = entries(&keys, &albums);
        for seed in 0..50 {
            let order = balanced(&e, None, 0, seed);
            assert!(order.windows(2).all(|w| keys[w[0]] != keys[w[1]]), "seed {seed}");
        }
        // Kept apart as two artists, they'd often meet.
        let apart: Vec<String> = (0..60)
            .map(|i| match i % 6 {
                0 => "米津玄師".to_string(),
                1 => "kenshiyonezu".to_string(),
                _ => format!("other{}", i % 12),
            })
            .collect();
        let e = entries(&apart, &albums);
        let met = (0..50)
            .filter(|&seed| balanced(&e, None, 0, seed).windows(2).any(|w| keys[w[0]] == keys[w[1]]))
            .count();
        assert!(met > 10, "{met}");
    }

    #[test]
    fn an_artist_with_a_quarter_of_the_songs_is_never_back_to_back() {
        let aliases = Aliases::new();
        let keys: Vec<String> = (0..40)
            .map(|i| match i % 4 {
                0 => artist_key("A", &aliases),
                _ => format!("other{i}"),
            })
            .collect();
        let albums: Vec<String> = (0..40).map(|i| format!("al{i}")).collect();
        let e = entries(&keys, &albums);
        for seed in 0..50 {
            let order = balanced(&e, None, 0, seed);
            assert!(order.windows(2).all(|w| keys[w[0]] != keys[w[1]]), "seed {seed}");
        }
    }

    #[test]
    fn putting_the_order_back() {
        let ids = ["c", "a", "x", "b", "a"];
        let original: Vec<String> = ["a", "b", "c", "a", "gone"].iter().map(|s| s.to_string()).collect();
        let order = unshuffle_order(&ids, &original);
        let back: Vec<&str> = order.iter().map(|&i| ids[i]).collect();
        assert_eq!(back, ["a", "b", "c", "a", "x"], "the original order, the song added since at the end");
        let mut v = vec![1, 2, 3, 4];
        apply(&mut v, &[3, 1]);
        assert_eq!(v, [4, 2, 1, 3], "nothing lost");
    }

    #[test]
    fn the_saved_order_is_places_in_the_queue() {
        let queue: Vec<String> = ["p", "c", "a", "b", "a"].iter().map(|s| s.to_string()).collect();
        let original: Vec<String> = ["a", "b", "c", "a", "gone"].iter().map(|s| s.to_string()).collect();
        let saved = positions(&queue, Some(0), &original);
        assert_eq!(saved, [2, 3, 1, 4]);
        assert_eq!(from_positions(&queue, &saved), ["a", "b", "c", "a"]);
        assert!(positions(&queue, Some(0), &[]).is_empty());
    }

    #[test]
    fn thirty_thousand_songs_are_quick() {
        // About 3 000 artists of very different sizes, a few hundred played recently.
        let now = 1_800_000_000u64;
        let artists: Vec<String> = (0..30_000).map(|i: usize| format!("artist{}", (i * 7919) % (1 + i % 3_000))).collect();
        let albums: Vec<String> = (0..30_000).map(|i: usize| format!("album{}", i / 11)).collect();
        let e: Vec<Entry> = artists
            .iter()
            .zip(&albums)
            .enumerate()
            .map(|(i, (a, b))| entry(a, b, if i % 97 == 0 { now - i as u64 } else { 0 }))
            .collect();
        let started = std::time::Instant::now();
        let order = balanced(&e, None, now, 7);
        let took = started.elapsed();
        assert_eq!(order.len(), 30_000);
        // Generous: debug builds are many times slower than the app's (a few ms there).
        assert!(took < std::time::Duration::from_millis(1_500), "{took:?}");
        eprintln!("balanced shuffle of 30 000 songs: {took:?}");
    }
}
