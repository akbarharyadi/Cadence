//! Favourites and ListenBrainz's "loved" songs.
//!
//! Hearting a song sends feedback score 1 to ListenBrainz, un-hearting sends 0 (it needs the
//! song's recording id; a song without one is skipped quietly). Like listens, feedback waits in a
//! file when it can't be sent (offline, busy), per account, at most `MOST_WAITING` of it, and it's
//! sent by the scrobbler's own thread: hearting never waits for the network.
//!
//! The recording id comes from the file's tags (MUSICBRAINZ_TRACKID and its kin, `tagids`), else
//! from ListenBrainz's metadata lookup of artist and title (needs the token), whose answers
//! (and the lack of one, for a month) are kept in `listenbrainz-recordings.json`.
//!
//! "Sync favorites" compares both ways once, shows the numbers, and on a yes sends the local
//! favourites ListenBrainz doesn't have and hearts the library's songs it loves. Nothing is ever
//! un-hearted here by a pull.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ctx::Ctx;
use crate::lbapi::{self, Fail, Loved, Net, Reply, Req};
use crate::library::Track;
use crate::musicbrainz::{Clock, Pacer};
use crate::scrobble::Sent;

/// "Send hearts to ListenBrainz" as the sender checks it before every turn (the setting, kept in
/// step by `set_sending`): turned off, what waits stays waiting (bounded, `MOST_WAITING`) and
/// nothing is sent or looked up until it's turned on again. Off until the app says otherwise.
static SENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_sending(on: bool) {
    SENDING.store(on, std::sync::atomic::Ordering::SeqCst);
}

pub fn sending() -> bool {
    SENDING.load(std::sync::atomic::Ordering::SeqCst)
}

/// The most feedback kept waiting (the newest).
pub const MOST_WAITING: usize = 2000;
/// Requests in one turn of the scrobbler's thread (listens are sent between turns).
pub const PER_TURN: usize = 3;
/// A song ListenBrainz had no recording for is asked about again after this long.
const ASK_AGAIN: u64 = 30 * 86_400;
const MOST_CACHED: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// The account it was given under.
    pub user: String,
    pub track_id: String,
    /// The song's file (empty: a CUE song, whose file's tags are the album's).
    #[serde(default)]
    pub path: String,
    pub artist: String,
    pub title: String,
    #[serde(default)]
    pub album: String,
    /// 1: loved, 0: no longer.
    pub score: i8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mbid: Option<String>,
}

impl Item {
    pub fn of(t: &Track, user: &str, score: i8) -> Item {
        Item {
            user: user.to_string(),
            track_id: t.id.clone(),
            path: if t.cue.is_none() { t.path.clone() } else { String::new() },
            artist: t.artist.clone(),
            title: t.title.clone(),
            album: t.album.clone(),
            score,
            mbid: None,
        }
    }
}

/// The feedback waiting to be sent, kept in a file.
pub struct Queue {
    file: PathBuf,
    items: Mutex<Vec<Item>>,
}

impl Queue {
    pub fn new(file: PathBuf) -> Queue {
        let mut items: Vec<Item> = crate::state::read_json(&file);
        if items.len() > MOST_WAITING {
            items.drain(..items.len() - MOST_WAITING);
        }
        Queue { file, items: Mutex::new(items) }
    }

    fn save(&self, items: &[Item]) {
        crate::state::write_json(&self.file, &items);
    }

    /// Keeps feedback for sending. A song's earlier waiting feedback is replaced (hearted, then
    /// un-hearted while offline: only the last counts). At most `MOST_WAITING`: the oldest go.
    pub fn push(&self, item: Item) {
        let mut q = self.items.lock();
        q.retain(|x| !(x.user == item.user && x.track_id == item.track_id));
        q.push(item);
        if q.len() > MOST_WAITING {
            let n = q.len() - MOST_WAITING;
            q.drain(..n);
            log::warn!("more than {MOST_WAITING} loved songs waiting to be sent; the {n} oldest let go");
        }
        self.save(&q);
    }

    pub fn len(&self) -> usize {
        self.items.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.lock().is_empty()
    }

    pub fn clear(&self) {
        let mut q = self.items.lock();
        q.clear();
        self.save(&q);
    }

    fn done(&self, item: &Item) {
        let mut q = self.items.lock();
        let before = q.len();
        q.retain(|x| !(x.user == item.user && x.track_id == item.track_id && x.score == item.score));
        if q.len() != before {
            self.save(&q);
        }
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> Vec<Item> {
        self.items.lock().clone()
    }
}

/// Forgets feedback waiting in a file (no queue running yet): disconnecting.
pub fn clear_file(file: &Path) {
    if file.exists() {
        crate::state::write_json(file, &Vec::<Item>::new());
    }
}

/// How the recording of a song was found out.
#[derive(Debug, PartialEq)]
pub enum Resolved {
    Id(String),
    /// No recording to be found: the feedback is let go.
    None,
    /// Couldn't ask now: try again later (after this long, if known).
    Retry(Option<Duration>),
    Unauthorized,
}

/// What a turn came to.
#[derive(Debug, PartialEq)]
pub struct Turn {
    pub outcome: Sent,
    /// Feedback still waiting.
    pub left: usize,
}

/// Sends up to `max` waiting items for `user`, oldest first. Accepted: done. Refused as bad (400):
/// let go. A song with no recording to be found: let go. Busy or offline: stops, the rest wait.
/// Another account's feedback is let go, never sent to this one.
pub fn flush(q: &Queue, user: &str, max: usize, resolve: &mut dyn FnMut(&Item) -> Resolved, send: &mut dyn FnMut(&Item, &str) -> Sent) -> Turn {
    {
        let mut items = q.items.lock();
        let before = items.len();
        items.retain(|i| i.user == user);
        if items.len() != before {
            q.save(&items);
        }
    }
    let mut outcome = Sent::Ok;
    for _ in 0..max {
        let Some(item) = q.items.lock().first().cloned() else { break };
        let mbid = match item.mbid.clone().map(Resolved::Id).unwrap_or_else(|| resolve(&item)) {
            Resolved::Id(m) => m,
            Resolved::None => {
                q.done(&item);
                continue;
            }
            Resolved::Retry(w) => {
                outcome = Sent::Retry(w, "couldn’t look up a recording".into());
                break;
            }
            Resolved::Unauthorized => {
                outcome = Sent::Unauthorized;
                break;
            }
        };
        match send(&item, &mbid) {
            Sent::Ok => q.done(&item),
            Sent::Rejected(why) => {
                log::info!("ListenBrainz refused the feedback for {} – {} ({why}); let go", item.artist, item.title);
                q.done(&item);
            }
            other => {
                outcome = other;
                break;
            }
        }
    }
    Turn { outcome, left: q.len() }
}

// ---------- finding a song's recording ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cached {
    mbid: Option<String>,
    asked: u64,
}

/// What ListenBrainz said about songs' recordings.
pub struct Recordings {
    file: PathBuf,
    map: HashMap<String, Cached>,
    dirty: bool,
}

fn key(artist: &str, title: &str) -> String {
    format!("{}\u{1}{}", crate::identity::fold(artist), crate::identity::fold(title))
}

impl Recordings {
    pub fn new(file: PathBuf) -> Recordings {
        Recordings { map: crate::state::read_json(&file), file, dirty: false }
    }

    /// Known: Some(Some(id)), or Some(None) when it was asked lately and there was none.
    fn known(&self, artist: &str, title: &str, now: u64) -> Option<Option<String>> {
        let c = self.map.get(&key(artist, title))?;
        (c.mbid.is_some() || now < c.asked + ASK_AGAIN).then(|| c.mbid.clone())
    }

    fn note(&mut self, artist: &str, title: &str, mbid: Option<String>, now: u64) {
        self.map.insert(key(artist, title), Cached { mbid, asked: now });
        if self.map.len() > MOST_CACHED {
            let mut ages: Vec<u64> = self.map.values().map(|c| c.asked).collect();
            ages.sort_unstable();
            let cut = ages[ages.len() - MOST_CACHED];
            self.map.retain(|_, c| c.asked >= cut);
        }
        self.dirty = true;
    }

    fn save(&mut self) {
        if std::mem::take(&mut self.dirty) {
            // A cache: it can be asked for again, so no .bak.
            crate::state::write_json_with(&self.file, &self.map, false);
        }
    }
}

/// Finds recordings: the file's tags first (no network), then what was kept, then ListenBrainz.
pub struct Resolver {
    pub recordings: Recordings,
    pub(crate) pacer: Pacer,
}

impl Resolver {
    pub fn new(file: PathBuf) -> Resolver {
        Resolver { recordings: Recordings::new(file), pacer: Pacer::default() }
    }

    pub fn resolve(&mut self, item: &Item, net: &mut dyn Net, clock: &mut dyn Clock) -> Resolved {
        if !item.path.is_empty()
            && let Some(id) = crate::tagids::read(Path::new(&item.path)).recording
        {
            return Resolved::Id(id);
        }
        let now = clock.unix();
        if let Some(known) = self.recordings.known(&item.artist, &item.title, now) {
            return known.map(Resolved::Id).unwrap_or(Resolved::None);
        }
        self.pacer.wait(clock);
        match net.call(&Req::get(lbapi::lookup_path(&item.artist, &item.title), true)) {
            Reply::Ok { body, .. } => {
                let found = lbapi::parse_lookup(&body);
                self.recordings.note(&item.artist, &item.title, found.clone(), now);
                self.recordings.save();
                found.map(Resolved::Id).unwrap_or(Resolved::None)
            }
            Reply::Failed(_) => {
                // A question it won't answer (400): not asked again for a month.
                self.recordings.note(&item.artist, &item.title, None, now);
                self.recordings.save();
                Resolved::None
            }
            Reply::Unauthorized => Resolved::Unauthorized,
            Reply::Busy(after) => Resolved::Retry(after.map(Duration::from_secs)),
            Reply::Offline => Resolved::Retry(None),
        }
    }
}

/// One turn for the scrobbler's thread: up to `PER_TURN` requests, one at a time and spaced by the
/// resolver's pacer. Also returns how long to wait before the next request when the rate limit's
/// headers say none are left.
pub fn turn(q: &Queue, user: &str, net: &mut dyn Net, clock: &mut dyn Clock, resolver: &mut Resolver) -> (Turn, Option<Duration>) {
    let wait: std::cell::Cell<Option<Duration>> = std::cell::Cell::new(None);
    let net = std::cell::RefCell::new(net);
    let clock = std::cell::RefCell::new(clock);
    let resolver = std::cell::RefCell::new(resolver);
    let t = flush(
        q,
        user,
        PER_TURN,
        &mut |item| resolver.borrow_mut().resolve(item, &mut **net.borrow_mut(), &mut **clock.borrow_mut()),
        &mut |item, mbid| {
            let body = json!({ "recording_mbid": mbid, "score": item.score });
            resolver.borrow_mut().pacer.wait(&mut **clock.borrow_mut());
            let reply = net.borrow_mut().call(&Req::post("/feedback/recording-feedback", body));
            match reply {
                Reply::Ok { rate, .. } => {
                    if rate.remaining == Some(0) {
                        wait.set(Some(Duration::from_secs(rate.reset_in.unwrap_or(1).clamp(1, 120))));
                    }
                    Sent::Ok
                }
                Reply::Busy(after) => Sent::Retry(after.map(Duration::from_secs), "ListenBrainz is busy".into()),
                Reply::Unauthorized => Sent::Unauthorized,
                Reply::Failed(code) => Sent::Rejected(format!("ListenBrainz answered {code}")),
                Reply::Offline => Sent::Retry(None, "offline".into()),
            }
        },
    );
    (t, wait.get())
}

// ---------- "Sync favorites" ----------

/// The loved songs of an account, newest first (100 a request, no more than `PAGES` of them).
pub fn fetch_loved(net: &mut dyn Net, clock: &mut dyn Clock, user: &str, stop: &dyn Fn() -> bool) -> Result<Vec<Loved>, Fail> {
    const PAGES: u64 = 200;
    let mut pacer = Pacer::default();
    let mut out: Vec<Loved> = Vec::new();
    for _ in 0..PAGES {
        let body = lbapi::request(net, clock, &mut pacer, &Req::get(lbapi::feedback_path(user, out.len() as u64), false), stop)?;
        let Some((page, total)) = lbapi::parse_feedback(&body) else { return Err(Fail::Refused(200)) };
        let n = page.len();
        out.extend(page.into_iter().filter(|l| l.score == 1));
        // (A page's size counts what was asked for, not only the loved ones kept.)
        if n == 0 || (out.len() as u64) >= total || n < 100 {
            break;
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPreview {
    /// Favourites here.
    pub favorites: usize,
    /// Songs loved on ListenBrainz.
    pub loved: usize,
    /// In both.
    pub in_both: usize,
    /// Favourites ListenBrainz doesn't have: would be sent.
    pub to_send: usize,
    /// Loved songs of the library not hearted here: would be hearted.
    pub to_heart: usize,
    /// Loved songs not in the library for sure (or not named well enough to tell).
    pub not_found: usize,
    /// …of those, songs that fit more than one of the library's.
    pub ambiguous: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncPlan {
    pub send: Vec<String>,
    pub heart: Vec<String>,
}

/// Compares the favourites with the loved songs (`judge` finds a loved song in the library:
/// Some(song id), or None with whether it was ambiguous).
pub fn plan(favorites: &[String], loved: &[Loved], judge: &mut dyn FnMut(&Loved) -> Result<String, bool>) -> (SyncPlan, SyncPreview) {
    let mut found: Vec<String> = Vec::new();
    let (mut not_found, mut ambiguous) = (0, 0);
    for l in loved {
        match judge(l) {
            Ok(id) => {
                if !found.contains(&id) {
                    found.push(id);
                }
            }
            Err(amb) => {
                not_found += 1;
                ambiguous += usize::from(amb);
            }
        }
    }
    let mine: std::collections::HashSet<&str> = favorites.iter().map(String::as_str).collect();
    let theirs: std::collections::HashSet<&str> = found.iter().map(String::as_str).collect();
    let send: Vec<String> = favorites.iter().filter(|id| !theirs.contains(id.as_str())).cloned().collect();
    let heart: Vec<String> = found.iter().filter(|id| !mine.contains(id.as_str())).cloned().collect();
    let preview = SyncPreview {
        favorites: favorites.len(),
        loved: loved.len(),
        in_both: found.len() - heart.len(),
        to_send: send.len(),
        to_heart: heart.len(),
        not_found,
        ambiguous,
    };
    (SyncPlan { send, heart }, preview)
}

/// Hearts songs, never un-hearting: a song already hearted stays so, and none is touched but the
/// ones listed. Returns how many were hearted now.
pub fn heart(marks: &mut crate::marks::Marks, songs: &[&Track]) -> usize {
    let loved = marks.view();
    let mut n = 0;
    for t in songs {
        if !loved.get(&t.id).is_some_and(|m| m.loved) {
            marks.set_loved(t, true);
            n += 1;
        }
    }
    n
}

/// The last preview, kept until it's applied (its numbers are what's applied).
static PLAN: Mutex<Option<(String, SyncPlan)>> = Mutex::new(None);

/// Asks ListenBrainz for the loved songs and compares them with the favourites (nothing changes).
pub fn preview(ctx: &Ctx) -> Result<SyncPreview, String> {
    let state = &ctx.state;
    let user = state.settings.lock().listenbrainz_user.clone();
    if user.is_empty() {
        return Err("Connect ListenBrainz first.".into());
    }
    let mut http = lbapi::Http::new(None).ok_or("This system can’t make secure connections (no CA certificates).")?;
    static NEVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let mut clock = lbapi::Wall(std::time::Instant::now(), &NEVER);
    let loved = fetch_loved(&mut http, &mut clock, &user, &|| false).map_err(|f| f.say())?;
    let tracks = state.library.lock().snapshot.tracks.clone();
    let favorites: Vec<String> = {
        let marks = state.marks.lock().view();
        let mut ids: Vec<String> = tracks.iter().filter(|t| marks.get(&t.id).is_some_and(|m| m.loved)).map(|t| t.id.clone()).collect();
        ids.sort();
        ids
    };
    let mut matcher = crate::lbimport::Matcher::new(&tracks, crate::identity::current(), |t| crate::tagids::of_track(t).recording);
    let (p, preview) = plan(&favorites, &loved, &mut |l| {
        if l.artist.is_empty() || l.title.is_empty() {
            return Err(false);
        }
        let remote = lbapi::Remote { artist: l.artist.clone(), title: l.title.clone(), album: l.album.clone(), recording_mbid: l.recording_mbid.clone(), ..Default::default() };
        match matcher.judge(&remote) {
            crate::lbimport::Verdict::Matched(e) => Ok(e.track_id),
            crate::lbimport::Verdict::Unmatched(_) => Err(false),
            crate::lbimport::Verdict::Ambiguous(_) => Err(true),
        }
    });
    *PLAN.lock() = Some((user, p));
    Ok(preview)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDone {
    pub hearted: usize,
    /// Favourites handed to the sender (it sends them in the background).
    pub queued: usize,
}

/// A sync's preview is being applied now (it changes the favourites).
static APPLYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn applying() -> bool {
    APPLYING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Applies the last preview: hearts the loved songs, queues the favourites to send.
pub fn apply(ctx: &Ctx) -> Result<SyncDone, String> {
    if APPLYING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("The sync is already being applied.".into());
    }
    let done = apply_now(ctx);
    APPLYING.store(false, std::sync::atomic::Ordering::SeqCst);
    done
}

fn apply_now(ctx: &Ctx) -> Result<SyncDone, String> {
    if crate::backup::restoring() {
        return Err("A backup is being restored. Try again when it has finished.".into());
    }
    let state = &ctx.state;
    let user = state.settings.lock().listenbrainz_user.clone();
    let (who, plan) = PLAN.lock().take().ok_or("Check again first: nothing is ready to apply.")?;
    if who != user || user.is_empty() {
        return Err("The account changed. Check again first.".into());
    }
    let hearted = {
        let lib = state.library.lock();
        let songs: Vec<&Track> = plan.heart.iter().filter_map(|id| lib.by_id.get(id).map(|&i| &lib.snapshot.tracks[i])).collect();
        heart(&mut state.marks.lock(), &songs)
    };
    if hearted > 0 {
        state.save_marks();
        ctx.emit("marks-changed", ());
    }
    let mut queued = 0;
    if let Some(s) = crate::scrobble::running() {
        let lib = state.library.lock();
        for id in &plan.send {
            if let Some(&i) = lib.by_id.get(id) {
                s.loved(Item::of(&lib.snapshot.tracks[i], &user, 1));
                queued += 1;
            }
        }
    }
    Ok(SyncDone { hearted, queued })
}

/// How many feedback items wait to be sent.
pub fn waiting() -> usize {
    crate::scrobble::running().map(|s| s.loved_waiting()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lbapi::fakes::*;

    const ID_A: &str = "30d08f4c-d825-4ae1-b79c-44242cddd7c0";

    fn item(user: &str, id: &str, score: i8) -> Item {
        Item { user: user.into(), track_id: id.into(), path: String::new(), artist: format!("Artist {id}"), title: format!("Title {id}"), album: String::new(), score, mbid: Some(ID_A.into()) }
    }

    fn queue(name: &str) -> Queue {
        let dir = std::env::temp_dir().join(format!("cadence-loved-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Queue::new(dir.join("loved.json"))
    }

    #[test]
    fn feedback_waits_per_account_is_bounded_and_replaced_by_the_last_word() {
        let q = queue("wait");
        q.push(item("me", "a", 1));
        q.push(item("me", "b", 1));
        q.push(item("me", "a", 0)); // un-hearted while offline: only this counts
        assert_eq!(q.snapshot().iter().map(|i| (i.track_id.as_str(), i.score)).collect::<Vec<_>>(), [("b", 1), ("a", 0)]);
        // Kept on disk: a restart still has it.
        assert_eq!(Queue::new(q.file.clone()).len(), 2);
        // Offline: everything waits.
        let t = flush(&q, "me", 10, &mut |_| unreachable!("it has its recording"), &mut |_, _| Sent::Retry(None, "offline".into()));
        assert!(matches!(t.outcome, Sent::Retry(..)));
        assert_eq!(t.left, 2);
        // The bound: the newest are kept.
        let big = queue("big");
        for i in 0..MOST_WAITING + 5 {
            big.push(item("me", &format!("s{i}"), 1));
        }
        let s = big.snapshot();
        assert_eq!(s.len(), MOST_WAITING);
        assert_eq!(s[0].track_id, "s5");
        // Another account's feedback is let go, not sent.
        let q = queue("accounts");
        q.push(item("them", "x", 1));
        q.push(item("me", "y", 1));
        let mut sent = Vec::new();
        let t = flush(&q, "me", 10, &mut |_| unreachable!(), &mut |i, _| {
            sent.push(i.track_id.clone());
            Sent::Ok
        });
        assert_eq!((sent, t.left), (vec!["y".to_string()], 0));
    }

    #[test]
    fn a_400_is_dropped_a_5xx_retried_and_a_401_stops() {
        let q = queue("answers");
        for id in ["a", "b", "c"] {
            q.push(item("me", id, 1));
        }
        // a refused as bad: let go, then b busy: stops, b and c wait.
        let mut calls = 0;
        let t = flush(&q, "me", 10, &mut |_| unreachable!(), &mut |_, _| {
            calls += 1;
            if calls == 1 { Sent::Rejected("400".into()) } else { Sent::Retry(Some(Duration::from_secs(9)), "503".into()) }
        });
        assert!(matches!(t.outcome, Sent::Retry(Some(d), _) if d == Duration::from_secs(9)));
        assert_eq!(q.snapshot().iter().map(|i| i.track_id.as_str()).collect::<Vec<_>>(), ["b", "c"]);
        // Tried again later: accepted.
        let t = flush(&q, "me", 10, &mut |_| unreachable!(), &mut |_, _| Sent::Ok);
        assert_eq!((t.outcome, t.left), (Sent::Ok, 0));
        // A token it doesn't accept stops the turn, the feedback stays.
        q.push(item("me", "d", 1));
        let t = flush(&q, "me", 10, &mut |_| unreachable!(), &mut |_, _| Sent::Unauthorized);
        assert_eq!((t.outcome, t.left), (Sent::Unauthorized, 1));
        // At most `max` requests a turn.
        let q = queue("max");
        for i in 0..5 {
            q.push(item("me", &format!("m{i}"), 1));
        }
        let t = flush(&q, "me", 2, &mut |_| unreachable!(), &mut |_, _| Sent::Ok);
        assert_eq!(t.left, 3);
    }

    #[test]
    fn a_song_without_a_recording_is_skipped_quietly_and_lookups_are_kept() {
        let q = queue("lookup");
        let mut no_tag = item("me", "n", 1);
        no_tag.mbid = None;
        no_tag.path = "Z:/not/there.flac".into();
        q.push(no_tag.clone());
        let dir = std::env::temp_dir().join(format!("cadence-recordings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut resolver = Resolver::new(dir.join("recordings.json"));
        // ListenBrainz knows no recording: let go without sending anything.
        let (mut net, mut clock) = fakes(vec![ok(r#"{}"#)]);
        let t = flush(&q, "me", 3, &mut |i| resolver.resolve(i, &mut net, &mut clock), &mut |_, _| panic!("no recording: nothing to send"));
        assert_eq!((t.outcome, t.left), (Sent::Ok, 0));
        assert_eq!(net.asked.len(), 1);
        assert!(net.asked[0].0.starts_with("/metadata/lookup/?artist_name=Artist%20n&recording_name=Title%20n"));
        // Asked again for the same song: answered from what was kept (no request), even after a restart.
        let mut resolver = Resolver::new(dir.join("recordings.json"));
        let (mut net, mut clock) = fakes(vec![]);
        assert_eq!(resolver.resolve(&no_tag, &mut net, &mut clock), Resolved::None);
        assert!(net.asked.is_empty());
        // A found recording is kept for good and used.
        let mut other = no_tag.clone();
        other.title = "Another".into();
        let (mut net, mut clock) = fakes(vec![ok(&format!(r#"{{"recording_mbid":"{ID_A}"}}"#))]);
        assert_eq!(resolver.resolve(&other, &mut net, &mut clock), Resolved::Id(ID_A.into()));
        assert_eq!(resolver.recordings.known("Artist n", "Another", u64::MAX / 2), Some(Some(ID_A.to_string())));
        // Offline: try later, nothing kept.
        let mut third = no_tag.clone();
        third.title = "Third".into();
        let (mut net, mut clock) = fakes(vec![Reply::Offline]);
        assert_eq!(resolver.resolve(&third, &mut net, &mut clock), Resolved::Retry(None));
        assert!(resolver.recordings.known("Artist n", "Third", 0).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_turn_sends_the_score_and_honours_the_rate_limit() {
        let q = queue("turn");
        q.push(item("me", "a", 0));
        q.push(item("me", "b", 1));
        let mut resolver = Resolver::new(std::env::temp_dir().join(format!("cadence-recordings-turn-{}.json", std::process::id())));
        let (mut net, mut clock) = fakes(vec![ok_rate("{}", 0, 6), ok("{}")]);
        let (t, wait) = turn(&q, "me", &mut net, &mut clock, &mut resolver);
        assert_eq!(t.outcome, Sent::Ok);
        assert_eq!(wait, Some(Duration::from_secs(6)));
        assert_eq!(net.asked.len(), 2);
        assert_eq!(net.asked[0].0, "/feedback/recording-feedback");
        let t = turn(&q, "me", &mut fakes(vec![]).0, &mut fakes(vec![]).1, &mut resolver).0;
        assert_eq!(t.left, 0);
    }

    fn loved(artist: &str, title: &str) -> Loved {
        Loved { score: 1, artist: artist.into(), title: title.into(), ..Default::default() }
    }

    #[test]
    fn the_sync_compares_both_ways_and_a_pull_never_un_hearts() {
        // Library ids: a, b, c. Hearted here: a, b. Loved there: b, c, and two songs not in the library.
        let favorites = vec!["a".to_string(), "b".to_string()];
        let there = vec![loved("A2", "T-b"), loved("A3", "T-c"), loved("Nobody", "Nothing"), loved("Someone", "Intro")];
        let (p, preview) = plan(&favorites, &there, &mut |l| match l.title.as_str() {
            "T-b" => Ok("b".into()),
            "T-c" => Ok("c".into()),
            "Intro" => Err(true),
            _ => Err(false),
        });
        assert_eq!(p, SyncPlan { send: vec!["a".into()], heart: vec!["c".into()] });
        assert_eq!(preview, SyncPreview { favorites: 2, loved: 4, in_both: 1, to_send: 1, to_heart: 1, not_found: 2, ambiguous: 1 });
        // Applying the pull hearts c and touches nothing else: a stays hearted although ListenBrainz doesn't love it.
        let song = |id: &str| Track { id: id.into(), title: format!("T-{id}"), artist: "A".into(), ..Default::default() };
        let mut marks = crate::marks::Marks::default();
        marks.set_loved(&song("a"), true);
        marks.set_loved(&song("b"), true);
        marks.set_rating(&song("d"), 4);
        let (c, a) = (song("c"), song("a"));
        assert_eq!(heart(&mut marks, &[&c, &a]), 1, "a was hearted already");
        let v = marks.view();
        assert!(v["a"].loved && v["b"].loved && v["c"].loved);
        assert_eq!(v["d"].rating, 4);
        // Nothing hearted that wasn't in the plan: an empty pull changes nothing.
        assert_eq!(heart(&mut marks, &[]), 0);
    }

    #[test]
    fn loved_songs_are_fetched_in_pages_and_only_the_loved_are_kept() {
        let item = |t: &str, s: i64| format!(r#"{{"score":{s},"track_metadata":{{"artist_name":"A","track_name":"{t}"}}}}"#);
        let first: Vec<String> = (0..100).map(|i| item(&format!("t{i}"), 1)).collect();
        let second = vec![item("x", 1), item("hated", -1)];
        let (mut net, mut clock) = fakes(vec![
            ok(&format!(r#"{{"feedback":[{}],"total_count":102,"count":100,"offset":0}}"#, first.join(","))),
            ok(&format!(r#"{{"feedback":[{}],"total_count":102,"count":2,"offset":100}}"#, second.join(","))),
        ]);
        let got = fetch_loved(&mut net, &mut clock, "me", &|| false).unwrap();
        assert_eq!(got.len(), 101);
        assert!(net.asked[1].0.contains("offset=100"));
        let (mut net, mut clock) = fakes(vec![Reply::Failed(404)]);
        assert_eq!(fetch_loved(&mut net, &mut clock, "me", &|| false), Err(Fail::Refused(404)));
    }
}
