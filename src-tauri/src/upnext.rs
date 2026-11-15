//! What comes after the queue.
//!
//! - **Album shuffle**: a random album, played whole and in order, then another, every album of
//!   the scope once before any comes again (albums not played lately first). The queue holds the
//!   playing album and the next one: when the last album queued starts, the engine asks for one
//!   more (`Status::more`), so a whole-library album shuffle never queues thousands of songs.
//! - **Autoplay**: when the queue is about to run out (and nothing repeats), ten songs like the
//!   last few: their neighbours by sound (`sonic`), or the same artists and genres before the
//!   sound analysis has reached them. Nothing played this session or in the last few hours,
//!   nothing the listener skips, never one artist twice in a row.
//!
//! The engine only says what it's waiting for; the songs are picked here, on a thread of their
//! own (not the audio thread, not the window's), once per extension.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::audio::{Cmd, More, QueueItem, Repeat, Status};
use crate::ctx::Ctx;
use crate::library::{Album, Track};
use crate::shuffle::{self, Entry, Rng, artist_key, recency};
use crate::state::{Aliases, AppState, match_key};

/// Songs Autoplay adds at a time.
pub const BATCH: usize = 10;
/// Songs played this recently are not picked by Autoplay.
const RECENT_SECS: u64 = 4 * 3600;
/// When Cadence started: what has played since is "this session".
static STARTED: LazyLock<u64> = LazyLock::new(crate::history::now);

// ---------- album shuffle ----------

/// An album, as the album shuffle orders it.
pub struct AlbumInfo {
    pub id: String,
    /// `artist_key` of the album's artist (albums by one artist are spread out too).
    pub artist: String,
    /// Unix seconds of the album's last play (0: not lately).
    pub last_played: u64,
}

/// The albums in the order they'll play: a balanced shuffle of them (one artist's albums spread
/// out, albums played lately later).
pub fn album_order(albums: &[AlbumInfo], now: u64, seed: u64) -> Vec<String> {
    let entries: Vec<Entry> = albums.iter().map(|a| Entry { artist: &a.artist, album: &a.id, last_played: a.last_played }).collect();
    shuffle::balanced(&entries, None, now, seed).into_iter().map(|i| albums[i].id.clone()).collect()
}

/// An album shuffle under way: the albums it shuffles, and those still to come this round.
pub struct AlbumPlan {
    pub scope: Vec<String>,
    pub left: VecDeque<String>,
}

impl AlbumPlan {
    pub fn new(scope: Vec<String>, order: Vec<String>) -> Self {
        AlbumPlan { scope, left: order.into() }
    }

    /// The next album's songs (`songs`: None or empty for one that can't play, which is passed over).
    pub fn next(&mut self, mut songs: impl FnMut(&str) -> Option<Vec<String>>) -> Option<Vec<String>> {
        while let Some(id) = self.left.pop_front() {
            if let Some(s) = songs(&id).filter(|s| !s.is_empty()) {
                return Some(s);
            }
        }
        None
    }
}

static PLAN: Mutex<Option<AlbumPlan>> = Mutex::new(None);

/// An album's songs in disc and track order, without the songs the listener skips.
pub fn album_songs<'a>(album: &Album, track: impl Fn(&str) -> Option<&'a Track>, skips: impl Fn(&Track) -> bool) -> Vec<String> {
    let mut songs: Vec<&'a Track> = album.track_ids.iter().filter_map(|id| track(id)).filter(|t| !skips(t)).collect();
    songs.sort_by_key(|t| (t.disc_no.unwrap_or(1), t.track_no.unwrap_or(u32::MAX)));
    songs.into_iter().map(|t| t.id.clone()).collect()
}

fn songs_of(state: &AppState, album_id: &str) -> Option<Vec<String>> {
    let rule = state.skip_rule();
    let lib = state.library.lock();
    let album = lib.snapshot.albums.iter().find(|a| a.id == album_id)?;
    Some(album_songs(album, |id| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i]), |t| rule.skips(t)))
}

/// The albums of `ids` with what ordering them needs (unknown albums are left out).
fn album_infos(state: &AppState, ids: &[String]) -> Vec<AlbumInfo> {
    let heard: HashMap<String, u64> = {
        let history = state.history.lock();
        let from = history.events.len().saturating_sub(5_000);
        history.events[from..].iter().map(|e| (e.album_id.clone(), e.at)).collect()
    };
    let aliases = shuffle::aliases(&state.data_dir);
    let lib = state.library.lock();
    let by_id: HashMap<&str, &Album> = lib.snapshot.albums.iter().map(|a| (a.id.as_str(), a)).collect();
    let mut seen = HashSet::new();
    ids.iter()
        .filter(|id| seen.insert(id.as_str()))
        .filter_map(|id| by_id.get(id.as_str()))
        .map(|a| AlbumInfo { id: a.id.clone(), artist: artist_key(&a.artist, &aliases), last_played: heard.get(&a.id).copied().unwrap_or(0) })
        .collect()
}

/// Starts an album shuffle over `album_ids`. False when none of them has a song to play.
pub fn shuffle_albums(ctx: &Ctx, album_ids: Vec<String>) -> bool {
    let state = &ctx.state;
    let infos = album_infos(state, &album_ids);
    let order = album_order(&infos, crate::history::now(), shuffle::clock_seed());
    let mut plan = AlbumPlan::new(infos.into_iter().map(|a| a.id).collect(), order);
    let Some(first) = plan.next(|id| songs_of(state, id)) else { return false };
    *PLAN.lock() = Some(plan);
    *ASKED.lock() = None;
    state.engine.send(Cmd::PlayAlbums { items: state.queue_items(&first), more: true });
    true
}

/// Plays one album of `album_ids`, picked at random (one not played lately, more likely), whole
/// and in order. Its id, or None when none can play.
pub fn surprise_album(ctx: &Ctx, album_ids: Vec<String>) -> Option<String> {
    let state = &ctx.state;
    let infos = album_infos(state, &album_ids);
    let order = album_order(&infos, crate::history::now(), shuffle::clock_seed());
    let (id, songs) = order.into_iter().find_map(|id| songs_of(state, &id).filter(|s| !s.is_empty()).map(|s| (id, s)))?;
    state.engine.send(Cmd::PlayAlbums { items: state.queue_items(&songs), more: false });
    Some(id)
}

/// The album shuffle's next album; with every album played, a new round under Repeat All.
fn next_album(state: &AppState, repeat: Repeat) -> Vec<QueueItem> {
    let mut plan = PLAN.lock();
    let Some(plan) = plan.as_mut() else { return Vec::new() };
    let mut songs = plan.next(|id| songs_of(state, id));
    if songs.is_none() && repeat == Repeat::All {
        let infos = album_infos(state, &plan.scope);
        plan.left = album_order(&infos, crate::history::now(), shuffle::clock_seed()).into();
        songs = plan.next(|id| songs_of(state, id));
    }
    songs.map(|ids| state.queue_items(&ids)).unwrap_or_default()
}

// ---------- Autoplay ----------

/// What Autoplay picks from.
pub struct Autoplay<'a> {
    pub tracks: &'a [Track],
    /// Sound features of the analysed songs, by id.
    pub features: &'a HashMap<&'a str, &'a [f32]>,
    /// The last songs of the queue, the latest last.
    pub seeds: &'a [&'a Track],
    /// Songs not to pick (in the queue, played this session).
    pub exclude: &'a HashSet<&'a str>,
    pub skips: &'a dyn Fn(&Track) -> bool,
    /// When songs last played (unix seconds).
    pub last_played: &'a HashMap<String, u64>,
    pub aliases: &'a Aliases,
    pub now: u64,
    pub seed: u64,
}

/// Up to `count` songs to play after the seeds (ids, in order).
pub fn autoplay_picks(input: &Autoplay, count: usize) -> Vec<String> {
    let key = |t: &Track| artist_key(&t.artist, input.aliases);
    let heard = |t: &Track| input.last_played.get(&t.id).copied().unwrap_or(0);
    let candidates: Vec<&Track> = input
        .tracks
        .iter()
        .filter(|t| !input.exclude.contains(t.id.as_str()) && !(input.skips)(t))
        .filter(|t| input.now.saturating_sub(heard(t)) >= RECENT_SECS)
        .collect();
    let mut rng = Rng::new(input.seed);
    // The latest song counts most.
    let seeds: Vec<(&[f32], f32)> = input
        .seeds
        .iter()
        .rev()
        .zip([0.5f32, 0.3, 0.2])
        .filter_map(|(s, w)| input.features.get(s.id.as_str()).map(|f| (*f, w)))
        .collect();
    let analysed = candidates.iter().filter(|t| input.features.contains_key(t.id.as_str())).count();
    let mut scored: Vec<(f32, &Track)> = if !seeds.is_empty() && analysed >= count * 3 {
        let total: f32 = seeds.iter().map(|(_, w)| w).sum();
        candidates
            .iter()
            .filter_map(|t| {
                let f = input.features.get(t.id.as_str())?;
                let d = seeds.iter().map(|(s, w)| w * crate::sonic::distance(s, f)).sum::<f32>() / total;
                // Songs heard lately a little less, and a touch of chance so it isn't the same every time.
                Some((d * (1.0 + 0.5 * recency(heard(t), input.now)) * (1.0 + 0.08 * rng.unit()), *t))
            })
            .collect()
    } else {
        // No sound analysis yet: the same artists and genres, in a random order, fresher first;
        // then anything else.
        let artists: HashSet<String> = input.seeds.iter().map(|t| key(t)).collect();
        let genres: HashSet<String> = input.seeds.iter().map(|t| match_key(&t.genre)).filter(|g| !g.is_empty()).collect();
        candidates
            .iter()
            .map(|t| {
                let near = artists.contains(&key(t)) || genres.contains(&match_key(&t.genre));
                (if near { 0.0 } else { 2.0 } + rng.unit() + 0.7 * recency(heard(t), input.now), *t)
            })
            .collect()
    };
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));

    let song = |t: &Track| format!("{}\u{1}{}", match_key(&t.title), key(t));
    let mut songs: HashSet<String> = input.seeds.iter().map(|t| song(t)).collect();
    let mut prev = input.seeds.last().map(|t| key(t));
    let mut per_artist: HashMap<String, usize> = HashMap::new();
    let mut picked: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    // At most two songs per artist in a batch; then, if that wasn't enough, more.
    for cap in [2, usize::MAX] {
        for (_, t) in &scored {
            if out.len() >= count {
                break;
            }
            if picked.contains(t.id.as_str()) {
                continue;
            }
            let a = key(t);
            if prev.as_ref() == Some(&a) || per_artist.get(&a).copied().unwrap_or(0) >= cap {
                continue;
            }
            // The same recording on another album.
            if !songs.insert(song(t)) {
                continue;
            }
            picked.insert(&t.id);
            out.push(t.id.clone());
            *per_artist.entry(a.clone()).or_default() += 1;
            prev = Some(a);
        }
    }
    out
}

/// Autoplay's next songs after `queue`.
fn similar_more(state: &AppState, queue: &[String]) -> Vec<QueueItem> {
    let now = crate::history::now();
    let since = (*STARTED).min(now.saturating_sub(RECENT_SECS));
    let (last_played, session): (HashMap<String, u64>, HashSet<String>) = {
        let history = state.history.lock();
        let session = history.events.iter().rev().take_while(|e| e.at >= since).map(|e| e.track_id.clone()).collect();
        (shuffle::recent_plays(&history), session)
    };
    let rule = state.skip_rule();
    let aliases = shuffle::aliases(&state.data_dir);
    let mut exclude: HashSet<&str> = queue.iter().map(String::as_str).collect();
    exclude.extend(session.iter().map(String::as_str));
    let skips = |t: &Track| rule.skips(t);
    // Copied out under the library's lock (only the songs that may be picked, and the seeds),
    // then the sound features under the analysis' lock (one lock at a time, briefly); scored with
    // neither held, so the scan, the window and the analysis aren't kept waiting.
    let (candidates, seeds): (Vec<Track>, Vec<Track>) = {
        let lib = state.library.lock();
        let candidates = lib
            .snapshot
            .tracks
            .iter()
            .filter(|t| !exclude.contains(t.id.as_str()) && !skips(t) && now.saturating_sub(last_played.get(&t.id).copied().unwrap_or(0)) >= RECENT_SECS)
            .cloned()
            .collect();
        let seeds = queue[queue.len().saturating_sub(3)..]
            .iter()
            .filter_map(|id| lib.by_id.get(id).map(|&i| lib.snapshot.tracks[i].clone()))
            .collect();
        (candidates, seeds)
    };
    let owned: Vec<(String, Vec<f32>)> = {
        let sonic = state.sonic.lock();
        candidates.iter().chain(&seeds).filter_map(|t| sonic.features(t).map(|f| (t.id.clone(), f.to_vec()))).collect()
    };
    let features: HashMap<&str, &[f32]> = owned.iter().map(|(id, f)| (id.as_str(), f.as_slice())).collect();
    let seed_refs: Vec<&Track> = seeds.iter().collect();
    let ids = autoplay_picks(
        &Autoplay {
            tracks: &candidates,
            features: &features,
            seeds: &seed_refs,
            exclude: &exclude,
            skips: &skips,
            last_played: &last_played,
            aliases: &aliases,
            now,
            seed: shuffle::clock_seed(),
        },
        BATCH,
    );
    log::info!("autoplay: {} songs", ids.len());
    state.queue_items(&ids)
}

// ---------- asked by the engine ----------

/// What more songs were last looked for: which queue (`Status::queue_gen`, so the same list played
/// again is asked for anew), for what, its length and its last song. Asked once per such queue.
type Asked = (u64, More, usize, String);
static ASKED: Mutex<Option<Asked>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

/// What `s` would ask for.
fn ask_key(s: &Status, more: More) -> Asked {
    (s.queue_gen, more, s.queue.len(), s.queue.last().cloned().unwrap_or_default())
}

/// Whether `key` wasn't asked for yet (and now it is).
fn first_ask(asked: &mut Option<Asked>, key: Asked) -> bool {
    if asked.as_ref() == Some(&key) {
        return false;
    }
    *asked = Some(key);
    true
}

/// For every engine status (the status thread): when the queue is about to run out, looks for
/// what comes next on a thread of its own (at background priority).
pub fn on_status(ctx: &Ctx, s: &Status) {
    LazyLock::force(&STARTED);
    let Some(more) = s.more else { return };
    let key = ask_key(s, more);
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    if !first_ask(&mut ASKED.lock(), key) {
        BUSY.store(false, Ordering::Release);
        return;
    }
    let (ctx, queue, repeat, queue_gen) = (ctx.clone(), s.queue.clone(), s.repeat, s.queue_gen);
    let spawned = std::thread::Builder::new().name("cadence-upnext".into()).spawn(move || {
        crate::jobs::background_priority();
        let items = match more {
            More::Albums => next_album(&ctx.state, repeat),
            More::Similar => similar_more(&ctx.state, &queue),
        };
        if !items.is_empty() {
            ctx.state.engine.send(Cmd::Extend { items, auto: more == More::Similar, queue: Some(queue_gen) });
        } else if more == More::Albums && repeat != Repeat::All {
            // Every album of the shuffle has played: it ends, and Autoplay takes over if it's on
            // (else the last album plays out and playback stops, as at the end of any queue).
            ctx.state.engine.send(Cmd::AlbumsDone { queue: queue_gen });
        }
        BUSY.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        BUSY.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, artist: &str, album: &str, genre: &str) -> Track {
        Track { id: id.into(), title: format!("Title {id}"), artist: artist.into(), album: album.into(), album_id: album.into(), genre: genre.into(), duration: 200.0, ..Default::default() }
    }

    #[test]
    fn the_same_list_played_again_is_asked_for_again() {
        let status = |queue_gen: u64, ids: &[&str]| Status { queue: ids.iter().map(|s| s.to_string()).collect(), queue_gen, ..Default::default() };
        let mut asked = None;
        let album = ["a1", "a2", "a3"];
        assert!(first_ask(&mut asked, ask_key(&status(1, &album), More::Similar)));
        assert!(!first_ask(&mut asked, ask_key(&status(1, &album), More::Similar)), "once per queue");
        // The album played again (a new queue, the same songs): its end asks again.
        assert!(first_ask(&mut asked, ask_key(&status(2, &album), More::Similar)));
        // The album shuffle ended and Autoplay takes over on the same queue: asked for that too.
        assert!(first_ask(&mut asked, ask_key(&status(2, &album), More::Albums)));
        assert!(first_ask(&mut asked, ask_key(&status(2, &album), More::Similar)));
    }

    #[test]
    fn every_album_plays_once_before_any_again() {
        let infos: Vec<AlbumInfo> = (0..25)
            .map(|i| AlbumInfo { id: format!("al{i}"), artist: format!("artist{}", i % 6), last_played: 0 })
            .collect();
        let order = album_order(&infos, 0, 3);
        let mut plan = AlbumPlan::new(infos.iter().map(|a| a.id.clone()).collect(), order.clone());
        let mut seen = HashSet::new();
        while let Some(songs) = plan.next(|id| Some(vec![id.to_string()])) {
            assert!(seen.insert(songs[0].clone()), "{} came twice", songs[0]);
        }
        assert_eq!(seen.len(), 25, "all of them");
        // One artist's albums aren't back to back where they can be apart.
        let artist = |id: &String| infos.iter().find(|a| &a.id == id).unwrap().artist.clone();
        assert!(order.windows(2).all(|w| artist(&w[0]) != artist(&w[1])));
        // An album that can't play is passed over.
        let mut plan = AlbumPlan::new(vec![], vec!["empty".into(), "gone".into(), "ok".into()]);
        let next = plan.next(|id| match id {
            "empty" => Some(vec![]),
            "gone" => None,
            _ => Some(vec!["s1".into()]),
        });
        assert_eq!(next, Some(vec!["s1".to_string()]));
        assert!(plan.next(|_| Some(vec!["x".into()])).is_none(), "round over");
    }

    #[test]
    fn albums_played_lately_come_later() {
        let now = 1_800_000_000;
        let infos: Vec<AlbumInfo> = (0..40)
            .map(|i| AlbumInfo { id: format!("al{i}"), artist: format!("artist{i}"), last_played: if i < 5 { now - 600 } else { 0 } })
            .collect();
        for seed in 0..20 {
            let order = album_order(&infos, now, seed);
            let first_half = &order[..20];
            let early = (0..5).filter(|i| first_half.contains(&format!("al{i}"))).count();
            assert!(early <= 1, "seed {seed}: {early} recently played albums early");
        }
    }

    #[test]
    fn an_album_plays_whole_and_in_disc_and_track_order() {
        let mut tracks = Vec::new();
        for (id, disc, no) in [("d2t1", 2, 1), ("d1t2", 1, 2), ("d1t1", 1, 1), ("d2t2", 2, 2), ("skip", 1, 3)] {
            let mut t = song(id, "A", "al", "");
            t.disc_no = Some(disc);
            t.track_no = Some(no);
            tracks.push(t);
        }
        let album = Album { id: "al".into(), track_ids: tracks.iter().map(|t| t.id.clone()).collect(), ..Default::default() };
        let by_id: HashMap<&str, &Track> = tracks.iter().map(|t| (t.id.as_str(), t)).collect();
        let songs = album_songs(&album, |id| by_id.get(id).copied(), |t| t.id == "skip");
        assert_eq!(songs, ["d1t1", "d1t2", "d2t1", "d2t2"]);
    }

    struct Lib {
        tracks: Vec<Track>,
        features: Vec<(String, Vec<f32>)>,
    }

    /// 8 artists × 6 songs; each artist's songs sound alike (features near the artist's number).
    fn lib(analysed: bool) -> Lib {
        let dims = bliss_audio::NUMBER_FEATURES;
        let mut tracks = Vec::new();
        let mut features = Vec::new();
        for a in 0..8 {
            for k in 0..6 {
                let id = format!("a{a}s{k}");
                tracks.push(song(&id, &format!("Artist {a}"), &format!("Album {a}"), if a < 4 { "Rock" } else { "Jazz" }));
                if analysed {
                    let mut f = vec![a as f32 * 0.1; dims];
                    f[1] += k as f32 * 0.001;
                    features.push((id, f));
                }
            }
        }
        Lib { tracks, features }
    }

    fn picks(l: &Lib, seeds: &[&str], exclude: &[&str], skip: &[&str], heard: &[(&str, u64)], now: u64) -> Vec<String> {
        let features: HashMap<&str, &[f32]> = l.features.iter().map(|(id, f)| (id.as_str(), f.as_slice())).collect();
        let seeds: Vec<&Track> = seeds.iter().map(|id| l.tracks.iter().find(|t| t.id == *id).unwrap()).collect();
        let exclude: HashSet<&str> = exclude.iter().copied().collect();
        let skip: HashSet<String> = skip.iter().map(|s| s.to_string()).collect();
        let skips = |t: &Track| skip.contains(&t.id);
        let last_played: HashMap<String, u64> = heard.iter().map(|(id, at)| (id.to_string(), *at)).collect();
        let aliases = Aliases::new();
        autoplay_picks(
            &Autoplay { tracks: &l.tracks, features: &features, seeds: &seeds, exclude: &exclude, skips: &skips, last_played: &last_played, aliases: &aliases, now, seed: 5 },
            BATCH,
        )
    }

    fn artist_of(l: &Lib, id: &str) -> String {
        l.tracks.iter().find(|t| t.id == id).unwrap().artist.clone()
    }

    #[test]
    fn autoplay_picks_songs_that_sound_alike_and_none_it_mustnt() {
        let l = lib(true);
        let now = 1_800_000_000;
        let queue = ["a2s0", "a2s1", "a2s2"];
        let session = ["a2s3", "a3s0"];
        let mut exclude: Vec<&str> = queue.to_vec();
        exclude.extend(session);
        let got = picks(&l, &queue, &exclude, &["a2s4", "a1s0"], &[("a3s1", now - 3600), ("a1s1", now - 30 * 86_400)], now);
        assert_eq!(got.len(), BATCH, "a batch");
        for id in &got {
            assert!(!exclude.contains(&id.as_str()), "{id} was queued or played this session");
            assert!(!["a2s4", "a1s0"].contains(&id.as_str()), "{id} is skipped");
            assert_ne!(id, "a3s1", "played an hour ago");
        }
        // Never one artist twice in a row, nor the last song's artist first.
        assert_ne!(artist_of(&l, &got[0]), "Artist 2");
        assert!(got.windows(2).all(|w| artist_of(&l, &w[0]) != artist_of(&l, &w[1])), "{got:?}");
        // The closest-sounding artists come first (artist 1 and 3 are nearest to 2).
        assert!(["Artist 1", "Artist 3", "Artist 2"].contains(&artist_of(&l, &got[0]).as_str()), "{got:?}");
        // A played-long-ago song may come back.
        let unique: HashSet<&String> = got.iter().collect();
        assert_eq!(unique.len(), got.len());
    }

    #[test]
    fn autoplay_without_sound_analysis_keeps_to_the_artists_and_genres() {
        let l = lib(false);
        let queue = ["a0s0", "a1s0"];
        let got = picks(&l, &queue, &queue, &[], &[], 1_800_000_000);
        assert_eq!(got.len(), BATCH);
        // The first picks are rock (artists 0–3), the seeds' genre.
        for id in &got[..6] {
            let t = l.tracks.iter().find(|t| &t.id == id).unwrap();
            assert_eq!(t.genre, "Rock", "{got:?}");
        }
        assert!(got.windows(2).all(|w| artist_of(&l, &w[0]) != artist_of(&l, &w[1])), "{got:?}");
        assert_ne!(artist_of(&l, &got[0]), "Artist 1");
        assert!(got.iter().all(|id| !queue.contains(&id.as_str())));
    }
}
