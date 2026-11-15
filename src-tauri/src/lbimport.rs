//! Restoring the listening history from ListenBrainz: the account's listens, newest to oldest, are
//! read page by page, each matched to a song of the library, and the matched plays that aren't in
//! the history yet are added to it (`history::merge`), once the listener has seen the numbers and
//! said yes.
//!
//! It's a background job the listener starts, at low priority, one request at a time and no more
//! than one a second (`lbapi`), shown in the sidebar, and it can be paused. Where it is (the next
//! page's start), what it has matched and what it couldn't are kept in `listenbrainz-import/`, so
//! a restart carries on. Listens that don't match a song for sure are counted and written to a CSV
//! the listener can save; nothing is ever guessed. Plays are matched by artist and title (across
//! scripts), with the length when the listen has one, as a playlist import does
//! (`playlist_io::Index`); when both the listen and the file's tags name a recording and they
//! differ, it's another recording of the song and the listen isn't matched.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::history::PlayEvent;
use crate::identity::Identity;
use crate::lbapi::{self, Fail, Net, Remote, Req};
use crate::library::Track;
use crate::musicbrainz::{Clock, Pacer};
use crate::playlist_io::{Entry, Index, Outcome};

// ---------- matching ----------

pub enum Verdict {
    Matched(PlayEvent),
    Unmatched(String),
    Ambiguous(String),
}

/// Matches listens to the library's songs.
pub struct Matcher<'a> {
    index: Index<'a>,
    who: Identity,
    /// What a song's tags say its recording is (asked only for a song a listen with a recording
    /// matched, once per song).
    mbid_of: Box<dyn FnMut(&Track) -> Option<String> + 'a>,
    seen: HashMap<String, Option<String>>,
}

impl<'a> Matcher<'a> {
    pub fn new(tracks: &'a [Track], who: Identity, mbid_of: impl FnMut(&Track) -> Option<String> + 'a) -> Self {
        Matcher { index: Index::new(tracks), who, mbid_of: Box::new(mbid_of), seen: HashMap::new() }
    }

    pub fn judge(&mut self, l: &Remote) -> Verdict {
        let e = Entry { artist: l.artist.clone(), title: l.title.clone(), album: l.album.clone(), duration: l.duration, ..Default::default() };
        match self.index.find(&e, None, &self.who) {
            Outcome::Matched(t) => {
                if let Some(theirs) = &l.recording_mbid {
                    let mbid_of = &mut self.mbid_of;
                    let ours = self.seen.entry(t.id.clone()).or_insert_with(|| mbid_of(t));
                    if ours.as_ref().is_some_and(|o| o != theirs) {
                        return Verdict::Unmatched("Another recording of the song (its recording id differs from the file’s)".into());
                    }
                }
                Verdict::Matched(PlayEvent {
                    track_id: t.id.clone(),
                    album_id: t.album_id.clone(),
                    at: l.at,
                    listened: t.duration,
                    duration: t.duration,
                    skipped: false,
                })
            }
            Outcome::NotFound(why) => Verdict::Unmatched(why),
            Outcome::Ambiguous(why) => Verdict::Ambiguous(why),
        }
    }
}

// ---------- what's kept between runs ----------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Summary {
    pub on_listenbrainz: u64,
    pub matched: u64,
    pub already_here: u64,
    pub will_add: u64,
    pub unmatched: u64,
    pub ambiguous: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Applied {
    pub added: usize,
    pub live: usize,
    pub archived: usize,
    pub already: usize,
    /// The safety snapshot taken first.
    pub safety: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    /// The account it reads.
    pub user: String,
    pub started: bool,
    /// The next page is the listens older than this (none: from the newest).
    pub cursor: Option<u64>,
    pub finished: bool,
    pub seen: u64,
    pub matched: u64,
    pub unmatched: u64,
    pub ambiguous: u64,
    /// How long the two files were when this was saved: a crash between writing them and saving
    /// this leaves lines the next run cuts off before it reads that page again.
    pub matched_len: u64,
    pub unmatched_len: u64,
    pub summary: Option<Summary>,
    pub applied: Option<Applied>,
}

pub struct Stage {
    dir: PathBuf,
}

const CSV_HEADER: &str = "Listened at (unix seconds),Artist,Title,Album,Why not matched\r\n";

impl Stage {
    pub fn new(data_dir: &Path) -> Stage {
        Stage { dir: data_dir.join("listenbrainz-import") }
    }
    fn state(&self) -> PathBuf {
        self.dir.join("state.json")
    }
    pub fn matched(&self) -> PathBuf {
        self.dir.join("matched.jsonl")
    }
    pub fn unmatched(&self) -> PathBuf {
        self.dir.join("unmatched.csv")
    }
    pub fn load(&self) -> Saved {
        crate::state::read_json(&self.state())
    }
    pub fn save(&self, s: &Saved) {
        // Small, rewritten each page, and can be started again: no .bak.
        crate::state::write_json_with(&self.state(), s, false);
    }
    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
    /// Cuts the two files back to what the saved state counted.
    fn cut_to(&self, s: &Saved) {
        for (f, len) in [(self.matched(), s.matched_len), (self.unmatched(), s.unmatched_len)] {
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&f)
                && file.metadata().is_ok_and(|m| m.len() > len)
            {
                let _ = file.set_len(len);
            }
        }
    }
    fn append(&self, file: PathBuf, bytes: &[u8], header: Option<&str>) -> std::io::Result<u64> {
        std::fs::create_dir_all(&self.dir)?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
        if f.metadata()?.len() == 0
            && let Some(h) = header
        {
            f.write_all(&[0xEF, 0xBB, 0xBF])?;
            f.write_all(h.as_bytes())?;
        }
        f.write_all(bytes)?;
        f.sync_data()?;
        Ok(f.metadata()?.len())
    }
    /// The matched plays so far.
    pub fn events(&self) -> Vec<PlayEvent> {
        let mut out = Vec::new();
        let _ = crate::history::read_archive_from(&self.matched(), 0, |e| out.push(e));
        out
    }
}

// ---------- reading the listens ----------

#[derive(Debug, PartialEq)]
pub enum End {
    Finished,
    Stopped,
    Failed(String),
}

/// Reads pages from where `saved` says until there are none (or `stop`, or a failure), filing each
/// page's matches and misses and saving the state after each page.
pub fn scan(
    stage: &Stage,
    saved: &mut Saved,
    net: &mut dyn Net,
    clock: &mut dyn Clock,
    judge: &mut dyn FnMut(&Remote) -> Verdict,
    stop: &dyn Fn() -> bool,
    mut tick: impl FnMut(&Saved),
) -> End {
    stage.cut_to(saved);
    let mut pacer = Pacer::default();
    loop {
        let body = match lbapi::request(net, clock, &mut pacer, &Req::get(lbapi::listens_path(&saved.user, saved.cursor), false), stop) {
            Ok(b) => b,
            Err(Fail::Stopped) => return End::Stopped,
            Err(f) => return End::Failed(f.say()),
        };
        let Some(listens) = lbapi::parse_listens(&body) else {
            return End::Failed("ListenBrainz gave an answer Cadence couldn’t read.".into());
        };
        let oldest = lbapi::oldest_of(&body);
        let (mut hits, mut misses) = (Vec::new(), Vec::new());
        let page = listens.len() as u64;
        for l in &listens {
            let verdict = judge(l);
            let ambiguous = matches!(verdict, Verdict::Ambiguous(_));
            match verdict {
                Verdict::Matched(e) => {
                    if let Ok(mut line) = serde_json::to_vec(&e) {
                        line.push(b'\n');
                        hits.extend_from_slice(&line);
                        saved.matched += 1;
                    }
                }
                Verdict::Unmatched(why) | Verdict::Ambiguous(why) => {
                    saved.unmatched += 1;
                    saved.ambiguous += u64::from(ambiguous);
                    let row = [l.at.to_string(), l.artist.clone(), l.title.clone(), l.album.clone(), why];
                    misses.extend_from_slice(row.iter().map(|f| crate::playlist_io::csv_field(f)).collect::<Vec<_>>().join(",").as_bytes());
                    misses.extend_from_slice(b"
");
                }
            }
        }
        saved.seen += page;
        let wrote = (|| -> std::io::Result<()> {
            if !hits.is_empty() {
                saved.matched_len = stage.append(stage.matched(), &hits, None)?;
            }
            if !misses.is_empty() {
                saved.unmatched_len = stage.append(stage.unmatched(), &misses, Some(CSV_HEADER))?;
            }
            Ok(())
        })();
        if let Err(e) = wrote {
            return End::Failed(format!("Couldn’t keep the listens read so far ({e})."));
        }
        // A page of listens that ListenBrainz keeps older than the last one's start; nothing
        // older (or a page that doesn't get older) is the end. Told by the page's raw listens
        // (`oldest_of`), not those kept: a page whose listens all lack a title isn't the end.
        let more = oldest.is_some_and(|o| saved.cursor.is_none_or(|c| o < c));
        if more {
            saved.cursor = oldest;
        } else {
            saved.finished = true;
        }
        saved.started = true;
        stage.save(saved);
        tick(saved);
        if !more {
            return End::Finished;
        }
    }
}

/// What adding the matched plays would do (nothing is written). `resolve`: which song a play's id
/// is now (`library_songs`), so a play recorded here before the song's file moved counts as the
/// same play as the listen.
pub fn summarize(data_dir: &Path, history: &parking_lot::Mutex<crate::history::History>, stage: &Stage, saved: &Saved, resolve: crate::history::Resolve) -> Summary {
    let events = stage.events();
    let matched = events.len() as u64;
    let (will_add, already) = crate::history::count_new_resolved(data_dir, history, events, resolve);
    Summary { on_listenbrainz: saved.seen, matched, already_here: already as u64, will_add: will_add as u64, unmatched: saved.unmatched, ambiguous: saved.ambiguous }
}

/// Adds the matched plays to the history, once `before_write` (the safety snapshot) has been done.
pub fn commit(
    data_dir: &Path,
    history: &parking_lot::Mutex<crate::history::History>,
    stage: &Stage,
    resolve: crate::history::Resolve,
    before_write: impl FnOnce() -> Result<(), String>,
) -> Result<crate::history::Merged, String> {
    crate::history::merge_resolved(history, data_dir, stage.events(), resolve, before_write)
}

/// The library's song ids now: an id that is one of them is that song; another one that moved
/// (`state::moved_to`) to one of them is that one; anything else is a song that's gone.
pub fn library_songs(state: &crate::state::AppState) -> impl Fn(&str) -> Option<String> + use<> {
    let ids: std::collections::HashSet<String> = state.library.lock().by_id.keys().cloned().collect();
    move |id: &str| {
        if ids.contains(id) {
            return Some(id.to_string());
        }
        crate::state::moved_to(id).filter(|n| ids.contains(n))
    }
}

// ---------- the job in the app ----------

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static DISCARD: AtomicBool = AtomicBool::new(false);
static APPLYING: AtomicBool = AtomicBool::new(false);
static MESSAGE: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "idle", "scanning", "paused", "failed", "ready", "applying" or "done".
    pub phase: String,
    pub connected: bool,
    pub seen: u64,
    pub matched: u64,
    pub unmatched: u64,
    pub ambiguous: u64,
    pub summary: Option<Summary>,
    pub applied: Option<Applied>,
    /// Why it stopped, or empty.
    pub message: String,
}

pub fn status(ctx: &Ctx) -> Status {
    let user = ctx.state.settings.lock().listenbrainz_user.clone();
    let s = Stage::new(&ctx.state.data_dir).load();
    // Another account's leftovers aren't this account's.
    let mine = !user.is_empty() && s.user == user;
    let s = if mine { s } else { Saved::default() };
    let phase = if APPLYING.load(Ordering::SeqCst) {
        "applying"
    } else if RUNNING.load(Ordering::SeqCst) {
        "scanning"
    } else if s.applied.is_some() {
        "done"
    } else if s.finished && s.summary.is_some() {
        "ready"
    } else if s.started && !MESSAGE.lock().is_empty() {
        "failed"
    } else if s.started {
        "paused"
    } else if !MESSAGE.lock().is_empty() {
        "failed"
    } else {
        "idle"
    };
    Status {
        phase: phase.into(),
        connected: !user.is_empty(),
        seen: s.seen,
        matched: s.matched,
        unmatched: s.unmatched,
        ambiguous: s.ambiguous,
        summary: s.summary,
        applied: s.applied,
        message: MESSAGE.lock().clone(),
    }
}

fn emit(ctx: &Ctx) {
    ctx.emit("listenbrainz-import-progress", status(ctx));
}

/// Starts reading (or carries on from where it stopped). Err: why not.
pub fn start(ctx: &Ctx) -> Result<Status, String> {
    let user = ctx.state.settings.lock().listenbrainz_user.clone();
    if user.is_empty() {
        return Err("Connect ListenBrainz first.".into());
    }
    if APPLYING.load(Ordering::SeqCst) {
        return Err("The history is being updated.".into());
    }
    let stage = Stage::new(&ctx.state.data_dir);
    let mut saved = stage.load();
    if saved.user != user || saved.applied.is_some() || (saved.finished && saved.summary.is_some()) {
        // Another account's, finished, or already added: a new reading from the newest listen.
        stage.clear();
        saved = Saved { user: user.clone(), ..Default::default() };
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(status(ctx));
    }
    STOP.store(false, Ordering::SeqCst);
    DISCARD.store(false, Ordering::SeqCst);
    MESSAGE.lock().clear();
    saved.user = user;
    saved.started = true;
    stage.save(&saved);
    let job = ctx.clone();
    let spawned = std::thread::Builder::new().name("cadence-listenbrainz-import".into()).spawn(move || {
        crate::jobs::background_priority();
        run(&job, &stage, saved);
        RUNNING.store(false, Ordering::SeqCst);
        if DISCARD.swap(false, Ordering::SeqCst) {
            stage.clear();
        }
        emit(&job);
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        return Err("Couldn’t start the job.".into());
    }
    emit(ctx);
    Ok(status(ctx))
}

fn run(ctx: &Ctx, stage: &Stage, mut saved: Saved) {
    let token = crate::secrets::get(crate::scrobble::TOKEN_SECRET);
    // Reading listens needs no token (they're public), but a signed-out account isn't read either.
    let Some(mut http) = lbapi::Http::new(token) else {
        *MESSAGE.lock() = "This system can’t make secure connections (no CA certificates).".into();
        return;
    };
    let tracks = ctx.state.library.lock().snapshot.tracks.clone();
    let who = crate::identity::current();
    let mut matcher = Matcher::new(&tracks, who, |t| crate::tagids::of_track(t).recording);
    let mut clock = lbapi::Wall(std::time::Instant::now(), &STOP);
    let end = scan(
        stage,
        &mut saved,
        &mut http,
        &mut clock,
        &mut |l| matcher.judge(l),
        &|| STOP.load(Ordering::SeqCst),
        |_| emit(ctx),
    );
    log::info!("ListenBrainz history import: {end:?} ({} listens read, {} matched)", saved.seen, saved.matched);
    match end {
        End::Finished => {
            let resolve = library_songs(&ctx.state);
            let summary = summarize(&ctx.state.data_dir, &ctx.state.history, stage, &saved, &resolve);
            saved.summary = Some(summary);
            stage.save(&saved);
        }
        End::Stopped => {}
        End::Failed(why) => *MESSAGE.lock() = why,
    }
}

/// Pauses: the reading stops and keeps where it is.
pub fn pause(ctx: &Ctx) {
    STOP.store(true, Ordering::SeqCst);
    emit(ctx);
}

/// Forgets what was read (and stops the job if it runs).
pub fn discard(ctx: &Ctx) {
    MESSAGE.lock().clear();
    if RUNNING.load(Ordering::SeqCst) {
        DISCARD.store(true, Ordering::SeqCst);
        STOP.store(true, Ordering::SeqCst);
    } else {
        Stage::new(&ctx.state.data_dir).clear();
    }
    emit(ctx);
}

/// The matched plays are being added to the history now.
pub fn applying() -> bool {
    APPLYING.load(Ordering::SeqCst)
}

/// Adds the matched plays to the history, after a snapshot of the data (Settings › Backup keeps it).
pub fn apply(ctx: &Ctx) -> Result<Applied, String> {
    let state = &ctx.state;
    if crate::backup::restoring() {
        return Err("A backup is being restored. Try again when it has finished.".into());
    }
    let stage = Stage::new(&state.data_dir);
    let mut saved = stage.load();
    if saved.user != state.settings.lock().listenbrainz_user || !saved.finished || saved.applied.is_some() {
        return Err("There is nothing ready to add. Start again from Settings.".into());
    }
    if RUNNING.load(Ordering::SeqCst) || APPLYING.swap(true, Ordering::SeqCst) {
        return Err("Wait for the reading to finish first.".into());
    }
    // (A restore that started just now: it waits for this one, or this one for it.)
    if crate::backup::restoring() {
        APPLYING.store(false, Ordering::SeqCst);
        return Err("A backup is being restored. Try again when it has finished.".into());
    }
    emit(ctx);
    let mut safety = String::new();
    let resolve = library_songs(state);
    let merged = commit(&state.data_dir, &state.history, &stage, &resolve, || {
        let path = crate::backup::snapshot(state, "safety")?;
        crate::backup::rotate_safety(&crate::backup::backups_dir(&state.data_dir));
        safety = path.to_string_lossy().into_owned();
        Ok(())
    });
    APPLYING.store(false, Ordering::SeqCst);
    let merged = match merged {
        Ok(m) => m,
        Err(e) => {
            emit(ctx);
            return Err(e);
        }
    };
    state.save_history();
    ctx.emit("history-changed", ());
    let applied = Applied { added: merged.added, live: merged.live, archived: merged.archived, already: merged.already, safety };
    saved.applied = Some(applied.clone());
    stage.save(&saved);
    let _ = std::fs::remove_file(stage.matched());
    emit(ctx);
    Ok(applied)
}

/// Writes the listens that matched no song to a CSV: `path`, or Documents › Cadence on Android.
pub fn save_unmatched(ctx: &Ctx, path: Option<String>) -> Result<crate::playlist_io::SavedFile, String> {
    let stage = Stage::new(&ctx.state.data_dir);
    let bytes = std::fs::read(stage.unmatched()).map_err(|_| "There are no unmatched listens to save.".to_string())?;
    let out = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            let mut p = PathBuf::from(p);
            if p.extension().is_none() {
                p.set_extension("csv");
            }
            p
        }
        None => crate::playlist_io::default_dir()?.join("ListenBrainz listens not in library.csv"),
    };
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("Can't make the folder {}: {e}", dir.display()))?;
    }
    std::fs::write(&out, &bytes).map_err(|e| format!("Can't save {}: {e}", out.display()))?;
    let rows = bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()).count().saturating_sub(1);
    Ok(crate::playlist_io::SavedFile { path: out.to_string_lossy().into_owned(), rows })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use crate::lbapi::fakes::*;
    use crate::lbapi::Reply;

    fn song(id: &str, title: &str, artist: &str, album: &str, duration: f64) -> Track {
        Track { id: id.into(), title: title.into(), artist: artist.into(), album: album.into(), album_id: format!("al-{album}"), duration, path: format!("D:/Music/{id}.flac"), ..Default::default() }
    }

    fn remote(at: u64, artist: &str, title: &str, duration: Option<f64>) -> Remote {
        Remote { at, artist: artist.into(), title: title.into(), duration, ..Default::default() }
    }

    fn listen_json(at: u64, artist: &str, title: &str, ms: u64) -> String {
        format!(r#"{{"listened_at":{at},"track_metadata":{{"artist_name":"{artist}","track_name":"{title}","additional_info":{{"duration_ms":{ms}}}}}}}"#)
    }

    fn page(listens: &[String]) -> String {
        format!(r#"{{"payload":{{"count":{},"user_id":"me","listens":[{}]}}}}"#, listens.len(), listens.join(","))
    }

    fn library() -> Vec<Track> {
        vec![
            song("lemon", "Lemon", "Kenshi Yonezu", "Bootleg", 255.0),
            song("glass", "Glass", "Lumen", "Prism", 180.0),
            song("i1", "Intro", "Nova Quartet", "One", 90.0),
            song("i2", "Intro", "Nova Quartet", "Two", 92.5),
            song("blue", "Blue Hour", "Nova Quartet", "Evenings", 201.0),
        ]
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cadence-lbimport-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn listens_match_only_when_it_is_certain() {
        let lib = library();
        let mut m = Matcher::new(&lib, Identity::from_pairs(&[("米津玄師", "Kenshi Yonezu")]), |t| (t.id == "glass").then(|| "30d08f4c-d825-4ae1-b79c-44242cddd7c0".to_string()));
        // The other script's name, and the length agrees.
        assert!(matches!(m.judge(&remote(1, "米津玄師", "Lemon", Some(255.2))), Verdict::Matched(e) if e.track_id == "lemon" && e.at == 1));
        // A listen with no length still matches a single song.
        assert!(matches!(m.judge(&remote(2, "Lumen", "Glass", None)), Verdict::Matched(_)));
        // Another length: another recording.
        assert!(matches!(m.judge(&remote(3, "Lumen", "Glass", Some(240.0))), Verdict::Unmatched(_)));
        // The title is in the library, by another artist.
        assert!(matches!(m.judge(&remote(4, "Somebody Else", "Lemon", Some(255.0))), Verdict::Unmatched(_)));
        // Two songs fit equally well: not guessed.
        assert!(matches!(m.judge(&remote(5, "Nova Quartet", "Intro", Some(91.2))), Verdict::Ambiguous(_)));
        // Not in the library at all.
        assert!(matches!(m.judge(&remote(6, "Nobody", "Nothing", Some(100.0))), Verdict::Unmatched(_)));
        // Both name a recording and they differ: not matched; the same: matched.
        let mut other = remote(7, "Lumen", "Glass", Some(180.0));
        other.recording_mbid = Some("98255a8c-017a-4bc7-8dd6-1fa36124572b".into());
        assert!(matches!(m.judge(&other), Verdict::Unmatched(w) if w.contains("recording")));
        other.recording_mbid = Some("30d08f4c-d825-4ae1-b79c-44242cddd7c0".into());
        assert!(matches!(m.judge(&other), Verdict::Matched(_)));
        // The file names none: the listen's recording alone doesn't stop it.
        let mut no_tag = remote(8, "Nova Quartet", "Blue Hour", Some(201.0));
        no_tag.recording_mbid = Some("98255a8c-017a-4bc7-8dd6-1fa36124572b".into());
        assert!(matches!(m.judge(&no_tag), Verdict::Matched(_)));
    }

    #[test]
    fn reading_pages_honours_the_rate_limit_resumes_after_a_failure_and_counts_right() {
        let dir = temp("scan");
        let stage = Stage::new(&dir);
        let lib = library();
        let p1 = page(&[listen_json(1_790_000_300, "Lumen", "Glass", 180_000), listen_json(1_790_000_200, "Nobody", "Nothing", 100_000), listen_json(1_790_000_100, "Kenshi Yonezu", "Lemon", 255_000)]);
        let p2 = page(&[listen_json(1_790_000_050, "Lumen", "Glass", 180_000), listen_json(1_790_000_040, "Nova Quartet", "Intro", 91_000)]);
        let empty = page(&[]);
        let who = || Identity::from_pairs(&[("米津玄師", "Kenshi Yonezu")]);
        let saved0 = Saved { user: "me".into(), started: true, ..Default::default() };

        // First run: the first page comes (no calls left in the window: it waits), then the connection drops.
        let mut saved = saved0.clone();
        let (mut net, mut clock) = fakes(vec![ok_rate(&p1, 0, 4), Reply::Offline]);
        let mut m = Matcher::new(&lib, who(), |_| None);
        let end = scan(&stage, &mut saved, &mut net, &mut clock, &mut |l| m.judge(l), &|| false, |_| {});
        assert!(matches!(end, End::Failed(_)), "{end:?}");
        assert!(clock.1.contains(&4_000), "the rate limit's window waited out: {:?}", clock.1);
        assert_eq!(saved.cursor, Some(1_790_000_100));
        assert_eq!((saved.seen, saved.matched, saved.unmatched), (3, 2, 1));
        assert!(!saved.finished);

        // A crash left a half-filed page after the last save: it is cut off when the run resumes.
        let mut f = std::fs::OpenOptions::new().append(true).open(stage.matched()).unwrap();
        f.write_all(b"{\"trackId\":\"junk\",\"albumId\":\"x\",\"at\":1,\"listened\":1.0,\"duration\":1.0,\"skipped\":false}\n").unwrap();
        drop(f);

        // After a restart: the state is read back, and the next page starts where the last ended.
        let mut saved = stage.load();
        assert_eq!(saved.cursor, Some(1_790_000_100));
        let (mut net, mut clock) = fakes(vec![ok(&p2), ok(&empty)]);
        let mut m = Matcher::new(&lib, who(), |_| None);
        let end = scan(&stage, &mut saved, &mut net, &mut clock, &mut |l| m.judge(l), &|| false, |_| {});
        assert_eq!(end, End::Finished);
        assert_eq!(net.asked[0].0, "/user/me/listens?count=1000&max_ts=1790000100");
        assert_eq!(net.asked[1].0, "/user/me/listens?count=1000&max_ts=1790000040");
        assert!(net.asked[1].1 - net.asked[0].1 >= crate::musicbrainz::GAP_MS, "one request a second at most");
        assert_eq!((saved.seen, saved.matched, saved.unmatched, saved.ambiguous), (5, 3, 2, 1));
        assert!(saved.finished);
        let events = stage.events();
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|e| e.track_id != "junk"));
        // The misses are in a CSV (with a header), each with its reason.
        let csv = std::fs::read_to_string(stage.unmatched()).unwrap();
        assert_eq!(csv.lines().count(), 3);
        assert!(csv.contains("Nobody,Nothing") && csv.contains("Intro"));

        // Paused by the person: stops before asking, and keeps where it was.
        let mut saved = Saved { user: "me".into(), started: true, ..Default::default() };
        let (mut net, mut clock) = fakes(vec![ok(&p1)]);
        let mut m = Matcher::new(&lib, who(), |_| None);
        assert_eq!(scan(&Stage::new(&temp("scan2")), &mut saved, &mut net, &mut clock, &mut |l| m.judge(l), &|| true, |_| {}), End::Stopped);
        assert!(net.asked.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_whose_listens_are_all_left_out_is_not_the_end() {
        let dir = temp("raw-paging");
        let stage = Stage::new(&dir);
        let lib = library();
        // A whole page of listens without a title (left out), then a real one, then nothing.
        let untitled: Vec<String> = (0..3).map(|i| format!(r#"{{"listened_at":{},"track_metadata":{{"artist_name":"Lumen","track_name":""}}}}"#, 1_790_000_300 - i)).collect();
        let p2 = page(&[listen_json(1_790_000_100, "Lumen", "Glass", 180_000)]);
        let mut saved = Saved { user: "me".into(), started: true, ..Default::default() };
        let (mut net, mut clock) = fakes(vec![ok(&page(&untitled)), ok(&p2), ok(&page(&[]))]);
        let mut m = Matcher::new(&lib, Identity::default(), |_| None);
        assert_eq!(scan(&stage, &mut saved, &mut net, &mut clock, &mut |l| m.judge(l), &|| false, |_| {}), End::Finished);
        assert_eq!(net.asked.len(), 3, "{:?}", net.asked);
        assert_eq!(net.asked[1].0, "/user/me/listens?count=1000&max_ts=1790000298");
        assert_eq!(saved.matched, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_dry_run_says_what_the_real_run_does_and_the_snapshot_comes_first() {
        let dir = temp("apply");
        let stage = Stage::new(&dir);
        let lib = library();
        let t = 1_735_000_000u64; // December 2024
        // Heard here already: Glass at t (a listen this app submitted), and Blue Hour at t+1000, 3 s off.
        let mut events = vec![
            PlayEvent { track_id: "glass".into(), album_id: "al-Prism".into(), at: t, listened: 180.0, duration: 180.0, skipped: false },
            PlayEvent { track_id: "blue".into(), album_id: "al-Evenings".into(), at: t + 1003, listened: 201.0, duration: 201.0, skipped: false },
        ];
        // The window is full: the rest of it is later plays.
        events.extend((0..19_998u64).map(|i| PlayEvent { track_id: format!("f{i}"), album_id: "x".into(), at: 1_800_000_000 + i * 5, listened: 100.0, duration: 100.0, skipped: false }));
        let history = parking_lot::Mutex::new(History { events });
        let p = page(&[
            listen_json(t + 1000, "Nova Quartet", "Blue Hour", 201_000),
            listen_json(t + 500, "Lumen", "Glass", 180_000),
            listen_json(t, "Lumen", "Glass", 180_000),
            listen_json(t - 86_400 * 400, "Lumen", "Glass", 180_000),
            listen_json(t - 86_400 * 800, "Kenshi Yonezu", "Lemon", 255_000),
            listen_json(t - 86_400 * 800, "Kenshi Yonezu", "Lemon", 255_000),
            listen_json(t - 5, "Nobody", "Nothing", 1_000),
        ]);
        let mut saved = Saved { user: "me".into(), started: true, ..Default::default() };
        let (mut net, mut clock) = fakes(vec![ok(&p), ok(&page(&[]))]);
        let mut m = Matcher::new(&lib, Identity::default(), |_| None);
        assert_eq!(scan(&stage, &mut saved, &mut net, &mut clock, &mut |l| m.judge(l), &|| false, |_| {}), End::Finished);
        let summary = summarize(&dir, &history, &stage, &saved, &crate::history::as_is);
        assert_eq!(summary, Summary { on_listenbrainz: 7, matched: 6, already_here: 3, will_add: 3, unmatched: 1, ambiguous: 0 });
        // Nothing has been written yet.
        assert!(!crate::history::archive_dir(&dir).exists());
        assert_eq!(history.lock().events.len(), 20_000);

        let mut taken = false;
        let merged = commit(&dir, &history, &stage, &crate::history::as_is, || {
            assert!(!crate::history::archive_dir(&dir).exists(), "the snapshot is taken before anything is written");
            taken = true;
            Ok(())
        })
        .unwrap();
        assert!(taken);
        assert_eq!((merged.added as u64, merged.already as u64), (summary.will_add, summary.already_here), "the real run is the dry run");
        // The two old plays, and the oldest play the window had (it is full), are in the archive's
        // year files, each in its own year; the others are in the window, in order.
        let h = history.lock();
        assert_eq!(h.events.len(), 20_000);
        assert!(h.events.windows(2).all(|w| w[0].at <= w[1].at));
        assert_eq!((merged.live, merged.archived), (1, 2));
        drop(h);
        for (year, n) in [(2022, 1), (2023, 1), (2024, 1)] {
            let mut got = 0;
            let _ = crate::history::read_archive_from(&crate::history::archive_dir(&dir).join(format!("{year}.jsonl")), 0, |_| got += 1);
            assert_eq!(got, n, "{year}");
        }
        // Running it again adds nothing.
        let again = summarize(&dir, &history, &stage, &saved, &crate::history::as_is);
        assert_eq!(again.will_add, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
