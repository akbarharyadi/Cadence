//! Today's AI mixes are kept apart by building each one's song pool here, before the model
//! sees anything: songs played around this hour, heavy rotation, old favourites gone quiet,
//! deep cuts, and a corner of the library that hasn't been played lately. The model then only
//! names, trims and orders each pool, which it does well, instead of being asked to invent
//! different mixes from one list (it drifts back to the most played artist every time).
//!
//! A song goes into one pool only, an artist that leads one pool gets at most one song in the
//! others, and songs from the last week's mixes go to the back of the line.

use std::collections::{HashMap, HashSet};

use crate::ai::{Candidate, Rng};
use crate::history::PlayEvent;

/// Songs offered to the model per pool.
pub const POOL: usize = 36;
/// A pool smaller than this isn't worth a mix.
pub const MIN: usize = 8;
/// Mixes made a day.
pub const MIXES: usize = 4;
const DAY: u64 = 86_400;
/// Songs per artist in one pool.
const PER_ARTIST: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    RightNow,
    OnRepeat,
    Rediscover,
    DeepCuts,
    Elsewhere,
    /// Fallback when there's little listening history: a cluster of songs that sound alike.
    Sound,
    /// Last resort: a spread of the whole library.
    Library,
}

impl Lane {
    const DAILY: [Lane; 5] = [Lane::RightNow, Lane::OnRepeat, Lane::Rediscover, Lane::DeepCuts, Lane::Elsewhere];

    /// What the tile says above the mix's title.
    pub fn label(self) -> &'static str {
        match self {
            Lane::RightNow => "Right now",
            Lane::OnRepeat => "On repeat",
            Lane::Rediscover => "Rediscover",
            Lane::DeepCuts => "Deep cuts",
            Lane::Elsewhere => "Somewhere new",
            Lane::Sound => "By sound",
            Lane::Library => "From your library",
        }
    }

    /// What the model is told the pool is.
    fn brief(self) -> &'static str {
        match self {
            Lane::RightNow => "songs the listener tends to play around this time of day",
            Lane::OnRepeat => "what the listener has played most this month",
            Lane::Rediscover => "songs the listener used to play but hasn't in two months or more",
            Lane::DeepCuts => "songs never or barely played, by artists the listener does play",
            Lane::Elsewhere => "a part of the library the listener hasn't been playing lately",
            Lane::Sound => "songs that sound alike",
            Lane::Library => "a spread of the whole library",
        }
    }
}

pub struct Pool {
    pub lane: Lane,
    /// Extra words for the model (the sound cluster's name, for example).
    pub note: String,
    pub songs: Vec<Candidate>,
}

impl Pool {
    pub fn brief(&self) -> String {
        if self.note.is_empty() {
            self.lane.brief().to_string()
        } else {
            format!("{} ({})", self.lane.brief(), self.note)
        }
    }
}

pub struct Input<'a> {
    /// Every song that may be played (skipped songs already left out).
    pub tracks: Vec<Candidate>,
    pub events: &'a [PlayEvent],
    pub now: u64,
    /// Local time minus UTC, in hours.
    pub utc_offset: i64,
    /// Groups of songs that sound alike: (name, track ids).
    pub clusters: Vec<(String, Vec<String>)>,
    /// Songs in the mixes of the last week.
    pub recent: HashSet<String>,
    pub seed: u64,
    /// Which of the five daily lanes sits out today.
    pub day: u64,
}

#[derive(Default)]
struct Played {
    total: u32,
    month: u32,
    /// Plays in the last 90 days.
    quarter: u32,
    /// Plays within two hours of the current local hour.
    this_hour: u32,
    last: u64,
}

pub fn pools(input: Input) -> Vec<Pool> {
    let Input { tracks, events, now, utc_offset, clusters, recent, seed, day } = input;
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let hour_of = |at: u64| ((at as i64 / 3600 + utc_offset).rem_euclid(24)) as i64;
    let hour_now = hour_of(now);

    let mut played: HashMap<&str, Played> = HashMap::new();
    let mut by_artist: HashMap<String, u32> = HashMap::new();
    let artist_of: HashMap<&str, String> = tracks.iter().map(|c| (c.track.id.as_str(), artist_key(c))).collect();
    for e in events.iter().filter(|e| !e.skipped) {
        let Some((id, artist)) = artist_of.get_key_value(e.track_id.as_str()) else { continue };
        let p = played.entry(id).or_default();
        p.total += 1;
        if now.saturating_sub(e.at) <= 30 * DAY {
            p.month += 1;
        }
        if now.saturating_sub(e.at) <= 90 * DAY {
            p.quarter += 1;
        }
        let d = (hour_of(e.at) - hour_now).rem_euclid(24);
        if d.min(24 - d) <= 2 {
            p.this_hour += 1;
        }
        p.last = p.last.max(e.at);
        *by_artist.entry(artist.clone()).or_default() += 1;
    }
    let stat = |c: &Candidate| played.get(c.track.id.as_str());

    // The part of the library played least lately, for the "somewhere new" lane: a sound cluster
    // if the library has been analysed, otherwise a genre.
    let mut groups: Vec<(String, Vec<String>)> = clusters;
    if groups.is_empty() {
        let mut by_genre: HashMap<String, Vec<String>> = HashMap::new();
        for c in &tracks {
            let g = c.track.genre.trim();
            if !g.is_empty() {
                by_genre.entry(g.to_lowercase()).or_default().push(c.track.id.clone());
            }
        }
        groups = by_genre.into_iter().collect();
        groups.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let quiet = groups
        .iter()
        .filter(|(_, ids)| ids.len() >= MIN * 2)
        .min_by(|a, b| {
            let share = |ids: &[String]| {
                ids.iter().map(|id| played.get(id.as_str()).map_or(0, |p| p.quarter)).sum::<u32>() as f64
                    / ids.len() as f64
            };
            share(&a.1).total_cmp(&share(&b.1))
        })
        .cloned();

    // Weight each song for a lane (0 = not in it).
    let weight = |lane: Lane, c: &Candidate| -> f64 {
        let p = stat(c);
        match lane {
            Lane::RightNow => p.map_or(0.0, |p| p.this_hour as f64),
            Lane::OnRepeat => p.map_or(0.0, |p| p.month as f64),
            Lane::Rediscover => p.map_or(0.0, |p| {
                if p.total >= 2 && now.saturating_sub(p.last) >= 60 * DAY { p.total as f64 } else { 0.0 }
            }),
            Lane::DeepCuts => {
                let artist_plays = by_artist.get(&artist_key(c)).copied().unwrap_or(0);
                if p.map_or(0, |p| p.total) <= 1 && artist_plays >= 3 { 1.0 } else { 0.0 }
            }
            Lane::Elsewhere => match &quiet {
                Some((_, ids)) if ids.contains(&c.track.id) => {
                    if p.is_none() { 2.0 } else { 1.0 }
                }
                _ => 0.0,
            },
            Lane::Sound | Lane::Library => 1.0,
        }
    };

    let mut used: HashSet<String> = HashSet::new();
    // Artists that lead an earlier pool (3+ songs in it).
    let mut leads: HashSet<String> = HashSet::new();
    let mut out: Vec<Pool> = Vec::new();
    let mut fill = |lane: Lane, note: String, members: &dyn Fn(&Candidate) -> f64, rng: &mut Rng, out: &mut Vec<Pool>| {
        // Weighted random order (Efraimidis–Spirakis), last week's songs pushed back.
        let mut order: Vec<(f64, &Candidate)> = tracks
            .iter()
            .filter(|c| !used.contains(&c.track.id))
            .filter_map(|c| {
                let mut w = members(c);
                if w <= 0.0 {
                    return None;
                }
                if recent.contains(&c.track.id) {
                    w *= 0.15;
                }
                Some((rng.unit().powf(1.0 / w), c))
            })
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut per_artist: HashMap<String, usize> = HashMap::new();
        let mut songs = Vec::new();
        for (_, c) in order {
            let a = artist_key(c);
            let cap = if leads.contains(&a) { 1 } else { PER_ARTIST };
            let n = per_artist.entry(a).or_default();
            if *n < cap {
                *n += 1;
                songs.push(c.clone());
                if songs.len() == POOL {
                    break;
                }
            }
        }
        if songs.len() < MIN {
            return;
        }
        used.extend(songs.iter().map(|c| c.track.id.clone()));
        leads.extend(per_artist.into_iter().filter(|(_, n)| *n >= 3).map(|(a, _)| a));
        out.push(Pool { lane, note, songs });
    };

    // The five daily lanes, a different one going first (and so choosing first) each day.
    let mut lanes = Lane::DAILY.to_vec();
    let n = lanes.len() as u64;
    lanes.rotate_left((day % n) as usize);
    for lane in lanes {
        if out.len() == MIXES {
            break;
        }
        let note = match (lane, &quiet) {
            (Lane::Elsewhere, Some((name, _))) => name.clone(),
            _ => String::new(),
        };
        fill(lane, note, &|c| weight(lane, c), &mut rng, &mut out);
    }
    // Little history: sound clusters, then the whole library.
    for (name, ids) in &groups {
        if out.len() >= MIXES {
            break;
        }
        let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
        fill(Lane::Sound, name.clone(), &|c| if ids.contains(c.track.id.as_str()) { 1.0 } else { 0.0 }, &mut rng, &mut out);
    }
    while out.len() < MIXES {
        let before = out.len();
        fill(Lane::Library, String::new(), &|_| 1.0, &mut rng, &mut out);
        if out.len() == before {
            break;
        }
    }
    out
}

pub fn artist_key(c: &Candidate) -> String {
    c.track.artist.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Track;

    fn song(artist: &str, n: usize) -> Candidate {
        Candidate {
            track: Track { id: format!("{artist}-{n}"), artist: artist.into(), ..Default::default() },
            sound: None,
        }
    }

    fn play(id: &str, at: u64) -> PlayEvent {
        PlayEvent { track_id: id.into(), album_id: String::new(), at, listened: 200.0, duration: 200.0, skipped: false }
    }

    #[test]
    fn lanes_are_separate() {
        let now = 400 * DAY + 9 * 3600; // 09:00 UTC
        let mut tracks: Vec<Candidate> = (0..120).map(|n| song("Aimer", n)).collect();
        for a in 0..30 {
            tracks.extend((0..6).map(|n| song(&format!("A{a}"), n)));
        }
        let mut events = Vec::new();
        // Aimer every morning this month; A0..A9 played a lot a year ago; A10..A14 a few plays.
        for d in 0..30 {
            for n in 0..20 {
                events.push(play(&format!("Aimer-{n}"), now - d * DAY - 600));
            }
        }
        for a in 0..10 {
            for n in 0..6 {
                for k in 0..3 {
                    events.push(play(&format!("A{a}-{n}"), now - 300 * DAY + k * 3600));
                }
            }
        }
        for a in 10..15 {
            events.push(play(&format!("A{a}-0"), now - 5 * DAY + 14 * 3600));
            events.push(play(&format!("A{a}-0"), now - 4 * DAY + 14 * 3600));
            events.push(play(&format!("A{a}-1"), now - 4 * DAY + 14 * 3600));
        }
        let clusters = vec![
            ("bright".to_string(), (0..120).map(|n| format!("Aimer-{n}")).collect()),
            ("quiet".to_string(), (15..30).flat_map(|a| (0..6).map(move |n| format!("A{a}-{n}"))).collect()),
        ];
        for day in 0..5 {
            let pools = pools(Input {
                tracks: tracks.clone(),
                events: &events,
                now,
                utc_offset: 0,
                clusters: clusters.clone(),
                recent: HashSet::new(),
                seed: 7 + day,
                day,
            });
            assert_eq!(pools.len(), MIXES, "day {day}");
            let mut seen = HashSet::new();
            let mut leads: HashMap<String, usize> = HashMap::new();
            for p in &pools {
                let mut per: HashMap<String, usize> = HashMap::new();
                for c in &p.songs {
                    assert!(seen.insert(c.track.id.clone()), "song in two pools");
                    *per.entry(artist_key(c)).or_default() += 1;
                }
                assert!(per.values().all(|&n| n <= PER_ARTIST));
                for (a, n) in per {
                    if n >= 3 {
                        *leads.entry(a).or_default() += 1;
                    }
                }
            }
            assert!(leads.values().all(|&n| n == 1), "an artist leads two pools on day {day}");
            // A different lane leads off each day.
            assert_eq!(pools[0].lane, Lane::DAILY[day as usize], "day {day}");
        }
    }

    /// Run by hand: today's pools from the real library (CADENCE_DATA=...\com.cadence.player).
    #[test]
    #[ignore]
    fn real_pools() {
        let Ok(dir) = std::env::var("CADENCE_DATA") else { return };
        let dir = std::path::Path::new(&dir);
        let cache: crate::library::LibraryCache = crate::state::read_json(&dir.join("library.json"));
        let history: crate::history::History = crate::state::read_json(&dir.join("history.json"));
        let sonic = crate::state::read_json::<crate::sonic::Store>(&dir.join("sonic.json")).checked();
        let items: Vec<_> = cache
            .tracks
            .iter()
            .filter_map(|t| sonic.features(t).map(|f| crate::sonic::Item { track: t, f }))
            .collect();
        let clusters = crate::sonic::mixes(&items).into_iter().map(|m| (m.name, m.track_ids)).collect();
        let now = crate::history::now();
        let tracks = cache.tracks.iter().map(|t| Candidate { track: t.clone(), sound: None }).collect();
        let started = std::time::Instant::now();
        let pools = pools(Input {
            tracks,
            events: &history.events,
            now,
            utc_offset: 7,
            clusters,
            recent: HashSet::new(),
            seed: now,
            day: now / DAY,
        });
        println!("{} songs, {} plays, pools in {:?}", cache.tracks.len(), history.events.len(), started.elapsed());
        for p in &pools {
            let mut per: HashMap<String, usize> = HashMap::new();
            for c in &p.songs {
                *per.entry(c.track.artist.clone()).or_default() += 1;
            }
            let mut per: Vec<_> = per.into_iter().collect();
            per.sort_by(|a, b| b.1.cmp(&a.1));
            let top: Vec<_> = per.iter().take(8).map(|(a, n)| format!("{a} {n}")).collect();
            println!("== {} — {} ({} songs, {} artists): {}", p.lane.label(), p.brief(), p.songs.len(), per.len(), top.join(", "));
        }
        // CADENCE_LIVE=1 also asks the configured AI service (one request, as "New mixes" does).
        if std::env::var("CADENCE_LIVE").is_err() {
            return;
        }
        let settings: serde_json::Value = crate::state::read_json(&dir.join("settings.json"));
        let ai: crate::ai::AiSettings = serde_json::from_value(settings["ai"].clone()).unwrap();
        let stats = history.stats();
        let by_id: HashMap<&str, &crate::library::Track> = cache.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
        let taste = crate::ai::taste(|id| by_id.get(id).copied(), &stats);
        let started = std::time::Instant::now();
        let mixes = crate::ai::daily_mixes(&ai, &taste, &pools, &stats, &[]).unwrap();
        println!("AI answered in {:?}", started.elapsed());
        for m in &mixes {
            let artists: HashSet<&str> = m.track_ids.iter().map(|id| by_id[id.as_str()].artist.as_str()).collect();
            println!("## [{}] {} — {} ({} songs, {} artists)", m.lane, m.title, m.reason, m.track_ids.len(), artists.len());
            for id in &m.track_ids {
                let t = by_id[id.as_str()];
                println!("   {} — {}", t.artist, t.title);
            }
        }
    }

    #[test]
    fn new_listener_still_gets_mixes() {
        let tracks: Vec<Candidate> = (0..10).flat_map(|a| (0..10).map(move |n| song(&format!("A{a}"), n))).collect();
        let pools = pools(Input {
            tracks,
            events: &[],
            now: 100 * DAY,
            utc_offset: 7,
            clusters: Vec::new(),
            recent: HashSet::new(),
            seed: 1,
            day: 0,
        });
        assert_eq!(pools.len(), MIXES);
        assert!(pools.iter().all(|p| p.lane == Lane::Library && p.songs.len() >= MIN));
    }
}
