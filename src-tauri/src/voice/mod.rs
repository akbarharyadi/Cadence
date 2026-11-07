//! Lyrics that follow the singer: Cadence listens to a song once (vocals separated from the
//! music, then a letter model), places every word of its lyrics on the singing, and keeps both:
//!
//! - the listening, per song, in the local data folder (`voice/<id>.bin`, ~1.4 KB per second),
//!   so new or corrected lyrics are placed again in a moment without listening again;
//! - the placed lyrics next to the saved lyrics (`lyrics/<id>.voice.json`).
//!
//! Everything runs on this computer, in the background at low priority: for songs whose lyrics the
//! listener opens (plus the next song in the queue, so it's ready when it starts), or for every
//! song with lyrics when asked to from Settings (most-played first; songs done are skipped).
//! The models are an optional download (about 440 MB).

// Phones don't listen (the models are desktop-only): most of this is unused there.
#![cfg_attr(not(desktop), allow(dead_code, unused_imports))]

pub mod align;
// Telling singers apart needs the separated vocals (desktop); its results are read everywhere.
pub mod sing;
#[cfg(desktop)]
mod listen;
#[cfg(desktop)]
mod models;

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use crate::ctx::Ctx;

use crate::library::Track;
use crate::lyrics::{Line, Lyrics};
use crate::state::AppState;

/// A singer the AI found: the name, and "lower" or "higher" when it knows whose voice is which.
#[derive(Clone, Serialize, Deserialize)]
pub struct AiSinger {
    pub name: String,
    #[serde(default)]
    pub voice: String,
    /// The page that credits them as a vocalist on this song (singers without one aren't kept).
    #[serde(default)]
    pub credit: String,
}

/// What the AI found about who sings a song's lyrics (`lyrics/<id>.singers.json`).
#[derive(Serialize, Deserialize)]
struct AiSingers {
    /// Hash of the lyrics' text, as `Placed::text`.
    text: String,
    singers: Vec<AiSinger>,
    /// Per line, who sings it (indexes into `singers`); empty when the AI found no published parts.
    #[serde(default)]
    parts: Vec<Vec<u8>>,
}

/// Placed lyrics, saved per song.
#[derive(Serialize, Deserialize)]
struct Placed {
    /// `align::VERSION` when placed.
    v: u32,
    /// Hash of the lyrics' text: other lyrics are placed again.
    text: String,
    lines: Vec<Line>,
    /// The listener turned it off for this song (the lyrics' own timing is shown).
    #[serde(default)]
    off: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub track_id: String,
    pub title: String,
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The models can run on this system.
    pub supported: bool,
    pub installed: bool,
    /// Bytes downloaded so far and in all, while downloading.
    pub installing: Option<(u64, u64)>,
    pub download_size: u64,
    pub error: String,
    /// Follow the singer for every song whose lyrics are opened.
    pub auto: bool,
    pub job: Option<Job>,
    /// Following the singer in every song with lyrics (or how the last such run ended).
    pub batch: Option<Batch>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub running: bool,
    /// Still finding the songs with lyrics.
    pub preparing: bool,
    pub done: usize,
    pub total: usize,
    /// Songs whose lyrics couldn't be found in the singing (or couldn't be read).
    pub failed: usize,
    /// Estimated time left, once a song or two are done.
    pub seconds_left: Option<u64>,
    #[serde(skip)]
    started: Option<std::time::Instant>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub track_id: String,
    pub error: String,
}

pub struct Voice {
    /// The downloads (local data, not roaming: they're big).
    models_dir: PathBuf,
    /// Saved listening, per song.
    listened_dir: PathBuf,
    /// Saved lyrics; placed lyrics go next to them.
    lyrics_dir: PathBuf,
    queue: Mutex<VecDeque<String>>,
    wake: Condvar,
    worker: Mutex<bool>,
    job: Mutex<Option<Job>>,
    installing: Mutex<Option<(u64, u64)>>,
    error: Mutex<String>,
    batch: Mutex<Option<Batch>>,
    /// Songs of the running batch not done yet.
    batch_ids: Mutex<HashSet<String>>,
    /// Stop the song being listened to (the batch was stopped).
    stop: AtomicBool,
    /// The app is being used (a page opening): listening waits until then (ms since the epoch),
    /// so the graphics card and processor are the app's while it changes pages.
    busy_until: AtomicU64,
}

fn text_of(l: &Lyrics) -> Vec<String> {
    if !l.synced.is_empty() {
        l.synced.iter().map(|x| x.text.clone()).collect()
    } else {
        l.plain.lines().map(str::to_string).collect()
    }
}

/// The title or artist names more than one performer: "Song (feat. X)", "A ft. B", "A with B",
/// "A × B", "A x B". Not "&" or "and": band names ("Judy & Mary", "Bob Marley & The Wailers").
pub fn credits_several(title: &str, artist: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | ','))
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    let t = words(title);
    let a = words(artist);
    let marks = |w: &[String], with: bool| {
        w.iter().enumerate().any(|(i, x)| {
            matches!(x.as_str(), "feat." | "feat" | "ft." | "featuring" | "×" | "duet")
                || (x == "x" && i > 0 && i + 1 < w.len())
                || (with && x == "with" && i > 0 && i + 1 < w.len())
        })
    };
    // "with" in a title is usually just a word ("Dancing with Myself"); in a credit it's a guest.
    marks(&t, false) || marks(&a, true) || artist.contains('×') || title.contains('×')
}

fn text_hash(texts: &[String]) -> String {
    blake3::hash(texts.join("\n").as_bytes()).to_hex()[..16].to_string()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Voice {
    /// The app is being used: hold off for a moment (see `busy_until`).
    pub fn ui_busy(&self, ms: u64) {
        self.busy_until.fetch_max(now_ms() + ms, Ordering::Relaxed);
    }

    /// Between two steps of listening: wait while the app is busy (at most a few seconds at a
    /// time, and not when stopping).
    fn wait_while_busy(&self) {
        let start = now_ms();
        while now_ms() < self.busy_until.load(Ordering::Relaxed) && !self.stop.load(Ordering::Relaxed) && now_ms() - start < 5_000 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    pub fn new(data_dir: &Path, local_dir: &Path) -> Self {
        let models_dir = local_dir.join("models");
        #[cfg(desktop)]
        models::finish_removal(&models_dir);
        Self {
            models_dir,
            listened_dir: local_dir.join("voice"),
            lyrics_dir: data_dir.join("lyrics"),
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            worker: Mutex::new(false),
            job: Mutex::new(None),
            installing: Mutex::new(None),
            error: Mutex::new(String::new()),
            batch: Mutex::new(None),
            batch_ids: Mutex::new(HashSet::new()),
            stop: AtomicBool::new(false),
            busy_until: AtomicU64::new(0),
        }
    }

    fn placed_path(&self, id: &str) -> PathBuf {
        self.lyrics_dir.join(format!("{id}.voice.json"))
    }

    fn listened_path(&self, id: &str) -> PathBuf {
        self.listened_dir.join(format!("{id}.bin"))
    }

    fn voices_path(&self, id: &str) -> PathBuf {
        self.listened_dir.join(format!("{id}.sing"))
    }

    /// Whether the song's voices have been described (who sings when).
    pub fn has_voices(&self, track: &Track) -> bool {
        #[cfg(desktop)]
        return sing::is_saved(&self.voices_path(&track.id), &crate::sonic::stamp(track));
        #[cfg(not(desktop))]
        {
            let _ = track;
            false
        }
    }

    /// Who sings each line of these (placed) lyrics, from the song's saved voices.
    pub fn split(&self, track: &Track, lines: &[Line]) -> Option<sing::Split> {
        #[cfg(desktop)]
        {
            let v = sing::load(&self.voices_path(&track.id), &crate::sonic::stamp(track))?;
            sing::split(&v, lines)
        }
        #[cfg(not(desktop))]
        {
            let _ = (track, lines);
            None
        }
    }

    fn read_placed(&self, id: &str) -> Option<Placed> {
        serde_json::from_slice(&std::fs::read(self.placed_path(id)).ok()?).ok()
    }

    fn write_placed(&self, id: &str, p: &Placed) {
        let _ = std::fs::create_dir_all(&self.lyrics_dir);
        if let Ok(bytes) = serde_json::to_vec(p) {
            let _ = std::fs::write(self.placed_path(id), bytes);
        }
    }

    pub fn installed(&self) -> bool {
        #[cfg(desktop)]
        return models::installed(&self.models_dir);
        #[cfg(not(desktop))]
        false
    }

    pub fn status(&self, auto: bool) -> Status {
        #[cfg(desktop)]
        let (supported, download_size) = (models::RUNTIME.is_some(), models::download_size(&self.models_dir));
        #[cfg(not(desktop))]
        let (supported, download_size) = (false, 0);
        Status {
            supported,
            installed: self.installed(),
            installing: *self.installing.lock(),
            download_size,
            error: self.error.lock().clone(),
            auto,
            job: self.job.lock().clone(),
            batch: self.batch.lock().clone(),
        }
    }

    /// Places these lyrics with the song's saved listening, if there is one. Lyrics placed before
    /// are reused; changed lyrics are placed again (quick: the song isn't listened to again).
    pub fn apply(&self, track: &Track, mut lyrics: Lyrics) -> Lyrics {
        if lyrics.instrumental {
            return lyrics;
        }
        let texts = text_of(&lyrics);
        if texts.iter().all(|t| t.trim().is_empty()) {
            return lyrics;
        }
        let hash = text_hash(&texts);
        let saved = self.read_placed(&track.id);
        let off = saved.as_ref().is_some_and(|p| p.off);
        let placed = match saved {
            Some(p) if p.text == hash && p.v == align::VERSION => Some(p),
            _ => self.place(track, &lyrics, &texts, &hash, off),
        };
        let Some(placed) = placed else {
            self.attach_singers(track, &mut lyrics, &hash, None);
            return lyrics;
        };
        if placed.off {
            lyrics.voice = "off".into();
        } else {
            lyrics.voice = "on".into();
            let mut lines = placed.lines.clone();
            // Singers named in the lyrics stay with their lines.
            if lines.len() == lyrics.synced.len() {
                for (l, o) in lines.iter_mut().zip(&lyrics.synced) {
                    l.singers = o.singers.clone();
                }
            }
            lyrics.synced = lines;
        }
        self.attach_singers(track, &mut lyrics, &hash, Some(&placed.lines));
        lyrics
    }

    fn ai_singers_path(&self, id: &str) -> PathBuf {
        self.lyrics_dir.join(format!("{id}.singers.json"))
    }

    fn read_ai_singers(&self, id: &str) -> Option<AiSingers> {
        serde_json::from_slice(&std::fs::read(self.ai_singers_path(id)).ok()?).ok()
    }

    /// Keeps what the AI found about who sings this song's lyrics (`texts`, line by line).
    pub fn save_ai_singers(&self, id: &str, texts: &[String], singers: Vec<AiSinger>, parts: Vec<Vec<u8>>) {
        let _ = std::fs::create_dir_all(&self.lyrics_dir);
        let found = AiSingers {
            text: text_hash(texts),
            singers,
            parts,
        };
        if let Ok(bytes) = serde_json::to_vec(&found) {
            let _ = std::fs::write(self.ai_singers_path(id), bytes);
        }
    }

    /// Whether listening to the song would add something: never listened to, or a song with
    /// several singers whose voices haven't been described yet.
    pub fn needs_listening(&self, track: &Track) -> bool {
        !self.has_listened(track) || (self.several_singers(track) && !self.has_voices(track))
    }

    /// Whether the song is known to have several singers: the AI found two or more credited
    /// vocalists, or the title or artist says so ("feat.", "with", "×").
    pub fn several_singers(&self, track: &Track) -> bool {
        self.read_ai_singers(&track.id).is_some_and(|a| a.singers.iter().filter(|s| !s.credit.is_empty()).count() >= 2) || credits_several(&track.title, &track.artist)
    }

    /// Who sings each line, when the lyrics don't say: the AI's parts (from published lyrics
    /// that mark them), else the voices told apart in the singing (`placed`: the lines placed on
    /// it), named by the AI when it found the singers.
    fn attach_singers(&self, track: &Track, lyrics: &mut Lyrics, hash: &str, placed: Option<&[Line]>) {
        if !lyrics.singers.is_empty() || lyrics.synced.is_empty() {
            return;
        }
        let ai = self.read_ai_singers(&track.id).filter(|a| a.text == hash);
        if let Some(ai) = &ai {
            let n = ai.singers.len();
            if n >= 2 && ai.parts.len() == lyrics.synced.len() && ai.parts.iter().filter(|p| !p.is_empty()).count() * 2 >= ai.parts.len() {
                for (l, p) in lyrics.synced.iter_mut().zip(&ai.parts) {
                    l.singers = p.iter().copied().filter(|&i| (i as usize) < n).collect();
                }
                lyrics.singers = ai.singers.iter().map(|s| s.name.clone()).collect();
                lyrics.singers_from = "ai".into();
                return;
            }
        }
        // Pitch and tone can't tell one singer moving between registers from two singers (measured
        // on a real library: nearly every one-singer song "split"), so voices are only told apart
        // in songs known to have several singers.
        if !self.several_singers(track) {
            return;
        }
        let Some(placed) = placed.filter(|p| p.len() == lyrics.synced.len()) else { return };
        let Some(split) = self.split(track, placed) else { return };
        // The AI's singers, matched by voice: the lower one first.
        let named = |voice: &str| {
            ai.as_ref()
                .and_then(|a| a.singers.iter().find(|s| s.voice == voice))
                .map(|s| s.name.clone())
                .unwrap_or_default()
        };
        lyrics.singers = vec![named("lower"), named("higher")];
        for (l, g) in lyrics.synced.iter_mut().zip(&split.lines) {
            l.singers = match g {
                Some(2) => vec![0, 1],
                Some(g) => vec![*g],
                None => Vec::new(),
            };
        }
        lyrics.singers_from = "voice".into();
    }

    /// Places lyrics with the saved listening and saves the result; None without a listening
    /// (or when the words couldn't be found in it).
    fn place(&self, track: &Track, lyrics: &Lyrics, texts: &[String], hash: &str, off: bool) -> Option<Placed> {
        #[cfg(desktop)]
        {
            let em = listen::load(&self.listened_path(&track.id), &crate::sonic::stamp(track))?;
            let times: Option<Vec<f64>> = (!lyrics.synced.is_empty()).then(|| lyrics.synced.iter().map(|l| l.time).collect());
            let started = std::time::Instant::now();
            let aligned = align::align(&em, texts, times.as_deref())?;
            log::info!(
                "placed lyrics of {} on the singing: {} of {} lines, offset {:+.2} s, in {:?}",
                track.title,
                aligned.placed,
                aligned.lines_with_words,
                aligned.offset,
                started.elapsed()
            );
            // Most lines nowhere near the singing: probably the wrong lyrics.
            if aligned.placed * 2 < aligned.lines_with_words {
                return None;
            }
            let p = Placed {
                v: align::VERSION,
                text: hash.to_string(),
                lines: aligned.lines,
                off,
            };
            self.write_placed(&track.id, &p);
            Some(p)
        }
        #[cfg(not(desktop))]
        {
            let _ = (track, lyrics, texts, hash, off);
            None
        }
    }

    /// Turns following the singer on or off for one song (it stays placed either way).
    pub fn set_on(&self, id: &str, on: bool) {
        if let Some(mut p) = self.read_placed(id) {
            p.off = !on;
            self.write_placed(id, &p);
        }
    }

    pub fn has_listened(&self, track: &Track) -> bool {
        #[cfg(desktop)]
        return listen::is_saved(&self.listened_path(&track.id), &crate::sonic::stamp(track));
        #[cfg(not(desktop))]
        {
            let _ = track;
            false
        }
    }
}

/// Lyrics for a song as the lyrics view shows them, without the voice placing.
pub type Resolve = fn(&AppState, &Track) -> Lyrics;

/// Asks for a song to be listened to (first in line). Does nothing if it's already been.
pub fn request(ctx: &Ctx, track_id: String, resolve: Resolve) {
    let voice = ctx.voice.clone();
    {
        let mut q = voice.queue.lock();
        q.retain(|id| *id != track_id);
        if voice.job.lock().as_ref().is_some_and(|j| j.track_id == track_id) {
            return;
        }
        q.push_front(track_id);
    }
    voice.wake.notify_one();
    #[cfg(desktop)]
    start_worker(ctx, voice, resolve);
    #[cfg(not(desktop))]
    let _ = resolve;
}

fn emit_batch(ctx: &Ctx, voice: &Voice) {
    ctx.emit("voice-batch", voice.batch.lock().clone());
}

/// Remembers whether a batch is running, so it carries on after Cadence restarts.
fn remember_batch(ctx: &Ctx, running: bool) {
    let state = &ctx.state;
    let changed = {
        let mut s = state.settings.lock();
        std::mem::replace(&mut s.voice_batch, running) != running
    };
    if changed {
        state.save_settings();
    }
}

/// Carries on with a batch that was running when Cadence last closed (a minute after start, so
/// opening Cadence and its library scan come first).
pub fn resume_batch(ctx: &Ctx, resolve: Resolve) {
    if !ctx.state.settings.lock().voice_batch {
        return;
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        log::info!("carrying on following the singer in all songs");
        start_batch(&ctx, resolve);
    });
}

/// Follows the singer in every song with lyrics that hasn't had it: most-played first, after
/// anything asked for already. Finding the songs reads their tags, so it runs in the background
/// too ("voice-batch" events tell how it goes).
pub fn start_batch(ctx: &Ctx, resolve: Resolve) {
    let voice = ctx.voice.clone();
    {
        let mut batch = voice.batch.lock();
        if batch.as_ref().is_some_and(|b| b.running) || !voice.installed() {
            return;
        }
        *batch = Some(Batch {
            running: true,
            preparing: true,
            ..Default::default()
        });
    }
    remember_batch(ctx, true);
    emit_batch(ctx, &voice);
    #[cfg(desktop)]
    {
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("cadence-voice-batch".into()).spawn(move || {
            crate::jobs::background_priority();
            let state = ctx.state.clone();
            let started = std::time::Instant::now();
            let ids = batch_songs(&state, &voice);
            log::info!("following the singer in {} songs (found in {:?})", ids.len(), started.elapsed());
            let total = ids.len();
            {
                let mut q = voice.queue.lock();
                let mut set = voice.batch_ids.lock();
                for id in ids {
                    if !q.contains(&id) {
                        q.push_back(id.clone());
                    }
                    set.insert(id);
                }
            }
            *voice.batch.lock() = Some(Batch {
                running: total > 0,
                total,
                started: Some(std::time::Instant::now()),
                ..Default::default()
            });
            emit_batch(&ctx, &voice);
            if total > 0 {
                voice.wake.notify_one();
                start_worker(&ctx, voice, resolve);
            } else {
                remember_batch(&ctx, false);
            }
        });
        if let Err(e) = spawned {
            log::warn!("couldn't start following the singer in all songs: {e}");
        }
    }
    #[cfg(not(desktop))]
    let _ = resolve;
}

/// Stops following the singer in all songs (the songs done so far stay done).
pub fn stop_batch(ctx: &Ctx) {
    let voice = ctx.voice.clone();
    let ids = std::mem::take(&mut *voice.batch_ids.lock());
    voice.queue.lock().retain(|id| !ids.contains(id));
    if voice.job.lock().as_ref().is_some_and(|j| ids.contains(&j.track_id)) {
        voice.stop.store(true, Ordering::Relaxed);
    }
    if let Some(b) = voice.batch.lock().as_mut() {
        b.running = false;
        b.preparing = false;
        b.seconds_left = None;
    }
    remember_batch(ctx, false);
    emit_batch(ctx, &voice);
}

/// Whether a song has lyrics without asking LRCLIB: chosen or saved ones, or in the file.
#[cfg(desktop)]
fn has_lyrics(cache: &Path, track: &Track) -> bool {
    crate::lyrics::picked(cache, &track.id)
        .or_else(|| crate::lyrics::saved(cache, &track.id, &track.title))
        .or_else(|| crate::lyrics::local(Path::new(&track.path)))
        .is_some_and(|l| !l.instrumental && !l.is_empty())
}

/// The songs a batch listens to: with lyrics, not listened to yet, not skipped; most-played and
/// most recently played first.
#[cfg(desktop)]
fn batch_songs(state: &AppState, voice: &Voice) -> Vec<String> {
    let tracks = state.library.lock().snapshot.tracks.clone();
    let stats = state.history.lock().stats();
    let skipped = state.settings.lock().skipped.clone();
    let cache = state.data_dir.join("lyrics");
    let mut found: Vec<(u32, u64, String)> = tracks
        .iter()
        .filter(|t| t.duration >= 20.0 && t.duration <= listen::LONGEST_SECS && !skipped.contains(&t.id))
        .filter(|t| voice.needs_listening(t) && !crate::lyrics::is_hidden(&cache, &t.id))
        .filter(|t| has_lyrics(&cache, t))
        .map(|t| {
            (
                stats.plays.get(&t.id).copied().unwrap_or(0),
                stats.last_played.get(&t.id).copied().unwrap_or(0),
                t.id.clone(),
            )
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    found.into_iter().map(|(_, _, id)| id).collect()
}

#[cfg(desktop)]
fn start_worker(ctx: &Ctx, voice: Arc<Voice>, resolve: Resolve) {
    let mut running = voice.worker.lock();
    if *running {
        return;
    }
    *running = true;
    drop(running);
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("cadence-voice".into())
        .spawn(move || {
            crate::jobs::background_priority();
            worker(ctx, voice, resolve)
        });
    if let Err(e) = spawned {
        log::warn!("couldn't start the voice worker: {e}");
    }
}

#[cfg(desktop)]
mod runtime {
    use std::sync::OnceLock;

    use ort::environment::{Environment, GlobalThreadPoolOptions, ThreadManager};

    /// Inference threads: the voice worker plus one more at most, all in background mode.
    struct BackgroundThreads;

    impl ThreadManager for BackgroundThreads {
        type Thread = std::thread::JoinHandle<()>;

        fn create(&self, work: impl FnOnce() + Send + 'static) -> ort::Result<Self::Thread> {
            std::thread::Builder::new()
                .name("cadence-voice-math".into())
                .spawn(move || {
                    crate::jobs::background_priority();
                    work()
                })
                .map_err(|e| ort::Error::new(e.to_string()))
        }

        fn join(thread: Self::Thread) -> ort::Result<()> {
            let _ = thread.join();
            Ok(())
        }
    }

    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();

    /// ONNX Runtime has been loaded (its library can't be replaced until Cadence restarts).
    pub fn loaded() -> bool {
        ENV.get().is_some()
    }

    /// ONNX Runtime, loaded once from the downloaded library.
    pub fn environment(dylib: &std::path::Path) -> Result<&'static Environment, String> {
        ENV.get_or_init(|| {
            let builder = ort::init_from(dylib).map_err(|e| e.to_string())?;
            let threads = std::thread::available_parallelism().map(|n| (n.get() / 4).clamp(1, 2)).unwrap_or(1);
            let pool = GlobalThreadPoolOptions::default()
                .with_intra_threads(threads)
                .and_then(|p| p.with_inter_threads(1))
                .and_then(|p| p.with_spin_control(false))
                .and_then(|p| p.with_thread_manager(BackgroundThreads))
                .map_err(|e| e.to_string())?;
            builder
                .with_name("cadence")
                .with_telemetry(false)
                .with_global_thread_pool(pool)
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| e.clone())
    }
}

/// Where the models run: on the graphics card when ONNX Runtime can use one here (Windows: any
/// DirectX 12 GPU, through DirectML), else on the processor as before.
#[cfg(desktop)]
mod device {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Per model (separator, letters): the graphics card couldn't run it, or there's none that
    /// can. The processor until Cadence restarts.
    static FAILED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

    pub fn failed(separator: bool) {
        FAILED[separator as usize].store(true, Ordering::Relaxed);
    }

    /// The graphics card to try for this model, if any. `CADENCE_VOICE_DEVICE=cpu` keeps
    /// everything on the processor.
    pub fn gpu(separator: bool) -> Option<ort::ep::ExecutionProviderDispatch> {
        if FAILED[separator as usize].load(Ordering::Relaxed)
            || std::env::var("CADENCE_VOICE_DEVICE").is_ok_and(|v| v.eq_ignore_ascii_case("cpu"))
        {
            return None;
        }
        // Only real graphics cards (DXCore hardware adapters, never the WARP software one); the
        // fastest first, as on a laptop with two. DirectML.dll loads from next to onnxruntime.dll.
        #[cfg(windows)]
        return Some(
            ort::ep::DirectML::default()
                .with_device_filter(ort::ep::directml::DeviceFilter::Gpu)
                .with_performance_preference(ort::ep::directml::PerformancePreference::HighPerformance)
                .build()
                .error_on_failure(),
        );
        // macOS: Microsoft's build has CoreML, untried so far. Linux: CUDA needs a 200+ MB
        // runtime plus CUDA and cuDNN installed, so it stays on the processor.
        #[cfg(not(windows))]
        None
    }
}

/// Opens one of the models (the separator, or the letter model), on the graphics card if it can
/// run there (true), else on the processor. Memory is handed back to the system after every run
/// (no arena, no memory pattern; DirectML needs the latter off and runs nodes one at a time), so
/// it doesn't stay at its peak.
#[cfg(desktop)]
fn open_model(voice: &Voice, separator: bool) -> anyhow::Result<(ort::session::Session, bool)> {
    use ort::session::{Session, builder::GraphOptimizationLevel};
    let runtime = models::RUNTIME.as_ref().ok_or_else(|| anyhow::anyhow!("not available on this system"))?;
    let env = runtime::environment(&models::path(&voice.models_dir, runtime)).map_err(|e| anyhow::anyhow!("ONNX Runtime: {e}"))?;
    let part = if separator { &models::SEPARATOR } else { &models::LETTERS };
    let file = models::path(&voice.models_dir, part);
    let open = |gpu: Option<ort::ep::ExecutionProviderDispatch>| -> anyhow::Result<Session> {
        let fail = |e: ort::Error<_>| anyhow::anyhow!("{e}");
        let cpu = ort::ep::CPU::default().with_arena_allocator(false).build();
        Ok(Session::builder(env)?
            .with_execution_providers(gpu.into_iter().chain([cpu]).collect::<Vec<_>>())
            .map_err(fail)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(fail)?
            .with_memory_pattern(false)
            .map_err(fail)?
            .with_parallel_execution(false)
            .map_err(fail)?
            .commit_from_file(&file)?)
    };
    if let Some(gpu) = device::gpu(separator) {
        match open(Some(gpu)) {
            Ok(session) => return Ok((session, true)),
            Err(e) => {
                log::info!("{} runs on the processor: the graphics card can't ({e:#})", part.file);
                device::failed(separator);
            }
        }
    }
    Ok((open(None)?, false))
}

#[cfg(desktop)]
fn worker(ctx: Ctx, voice: Arc<Voice>, resolve: Resolve) {
    let state = ctx.state.clone();
    loop {
        let next = {
            let mut q = voice.queue.lock();
            while q.is_empty() {
                voice.wake.wait(&mut q);
            }
            q.pop_front()
        };
        let Some(id) = next else { continue };
        let track = {
            let lib = state.library.lock();
            lib.by_id.get(&id).map(|&i| lib.snapshot.tracks[i].clone())
        };
        let Some(track) = track else { continue };
        let error = match run(&ctx, &state, &voice, &track, resolve) {
            Ok(()) => String::new(),
            Err(e) if voice.stop.load(Ordering::Relaxed) => {
                log::info!("stopped listening to {}", track.title);
                format!("{e:#}")
            }
            Err(e) => {
                log::warn!("couldn't follow the singer in {}: {e:#}", track.title);
                format!("{e:#}")
            }
        };
        *voice.job.lock() = None;
        if voice.batch_ids.lock().remove(&id) {
            let finished = voice.batch_ids.lock().is_empty();
            if let Some(b) = voice.batch.lock().as_mut() {
                b.done += 1;
                if !error.is_empty() {
                    b.failed += 1;
                }
                if finished {
                    b.running = false;
                    b.seconds_left = None;
                    remember_batch(&ctx, false);
                    log::info!("followed the singer in {} songs ({} couldn't be)", b.done - b.failed, b.failed);
                } else if let Some(started) = b.started {
                    let per_song = started.elapsed().as_secs_f64() / b.done as f64;
                    b.seconds_left = Some((per_song * (b.total - b.done) as f64) as u64);
                }
            }
            emit_batch(&ctx, &voice);
        }
        ctx.emit(
            "voice-done",
            Done {
                track_id: id.clone(),
                error,
            },
        );
        look_ahead(&ctx, &state, &voice, &id);
    }
}

/// After the song that's playing: listens to the song after it in the queue next (ahead of a
/// batch), if it has lyrics already.
#[cfg(desktop)]
fn look_ahead(ctx: &Ctx, state: &AppState, voice: &Voice, just_done: &str) {
    if !state.settings.lock().voice_auto {
        return;
    }
    let status = state.engine.status();
    let Some(i) = status.index else { return };
    if status.queue.get(i).map(String::as_str) != Some(just_done) {
        return;
    }
    let Some(next) = status.queue.get(i + 1).cloned() else { return };
    let track = {
        let lib = state.library.lock();
        lib.by_id.get(&next).map(|&i| lib.snapshot.tracks[i].clone())
    };
    let Some(track) = track else { return };
    if !voice.needs_listening(&track) || track.duration > listen::LONGEST_SECS {
        return;
    }
    if has_lyrics(&state.data_dir.join("lyrics"), &track) {
        let mut q = voice.queue.lock();
        q.retain(|id| *id != next);
        q.push_front(next);
        let _ = ctx;
    }
}

#[cfg(desktop)]
fn run(ctx: &Ctx, state: &AppState, voice: &Voice, track: &Track, resolve: Resolve) -> anyhow::Result<()> {
    use anyhow::bail;
    let lyrics = resolve(state, track);
    let texts = text_of(&lyrics);
    if lyrics.instrumental || texts.iter().all(|t| t.trim().is_empty()) {
        bail!("This song has no lyrics to follow.");
    }
    let stamp = crate::sonic::stamp(track);
    let listened = voice.listened_path(&track.id);
    let voices_file = voice.voices_path(&track.id);
    let has_letters = listen::is_saved(&listened, &stamp);
    let has_voices = sing::is_saved(&voices_file, &stamp);
    if !has_letters || (!has_voices && voice.several_singers(track)) {
        if track.duration > listen::LONGEST_SECS {
            bail!("This song is too long to listen to.");
        }
        if !voice.installed() {
            bail!("The voice models aren’t downloaded.");
        }
        update_runtime(ctx, voice);
        // Listened to before the voices were described: only the separation again.
        let only_voices = has_letters;
        let total = if only_voices { listen::voice_steps(track.duration) } else { listen::steps(track.duration) } + 1;
        let mut job = Job {
            track_id: track.id.clone(),
            title: track.title.clone(),
            done: 0,
            total,
        };
        voice.stop.store(false, Ordering::Relaxed);
        *voice.job.lock() = Some(job.clone());
        ctx.emit("voice-progress", job.clone());
        let started = std::time::Instant::now();
        let (heard, gpu) = loop {
            let mut gpu = false;
            let open = |separator| {
                let opened = open_model(voice, separator)?;
                gpu |= opened.1;
                Ok(opened)
            };
            let step = || {
                job.done = (job.done + 1).min(total - 1);
                *voice.job.lock() = Some(job.clone());
                ctx.emit("voice-progress", job.clone());
                voice.wait_while_busy();
                !voice.stop.load(Ordering::Relaxed)
            };
            let result = if only_voices {
                listen::listen_voices(Path::new(&track.path), open, step).map(|v| (None, v))
            } else {
                listen::listen(Path::new(&track.path), open, step).map(|(em, v)| (Some(em), v))
            };
            match result {
                // The graphics card failed while working (its driver restarted, out of memory…):
                // the song again, on the processor.
                Err(e) if gpu && !voice.stop.load(Ordering::Relaxed) => {
                    log::warn!("the graphics card failed while listening to {} ({e:#}); listening on the processor", track.title);
                    device::failed(true);
                    device::failed(false);
                    job.done = 0;
                }
                result => break (result?, gpu),
            }
        };
        log::info!(
            "listened to {} ({:.0} s) in {:?}, on the {}",
            track.title,
            track.duration,
            started.elapsed(),
            if gpu { "graphics card" } else { "processor" }
        );
        let (em, voices) = heard;
        if let Some(em) = em {
            listen::save(&listened, &stamp, &em)?;
        }
        sing::save(&voices_file, &stamp, &voices)?;
    }
    let hash = text_hash(&texts);
    let off = voice.read_placed(&track.id).is_some_and(|p| p.off);
    if voice.place(track, &lyrics, &texts, &hash, off).is_none() {
        bail!("Couldn’t find these lyrics in the singing. Are they the right ones?");
    }
    Ok(())
}

/// Downloads the models in the background (progress as "voice-install" events).
pub fn install(ctx: &Ctx) {
    #[cfg(desktop)]
    {
        let voice = ctx.voice.clone();
        if !start_download(&voice) {
            return;
        }
        voice.error.lock().clear();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = download(&ctx, &voice);
            if let Err(e) = result {
                log::warn!("voice models download failed: {e:#}");
                *voice.error.lock() = format!("{e:#}");
            }
            ctx.emit("voice-install", (0u64, 0u64));
        });
    }
    #[cfg(not(desktop))]
    let _ = ctx;
}

/// Marks a download as started; false if one is already going.
#[cfg(desktop)]
fn start_download(voice: &Voice) -> bool {
    let mut installing = voice.installing.lock();
    if installing.is_some() {
        return false;
    }
    *installing = Some((0, models::download_size(&voice.models_dir)));
    true
}

/// Downloads what's missing (after `start_download`), with progress as "voice-install" events.
#[cfg(desktop)]
fn download(ctx: &Ctx, voice: &Voice) -> anyhow::Result<()> {
    let mut last = std::time::Instant::now();
    let result = models::install(&voice.models_dir, |done, total| {
        *voice.installing.lock() = Some((done, total));
        if last.elapsed().as_millis() > 250 || done == total {
            last = std::time::Instant::now();
            ctx.emit("voice-install", (done, total));
        }
        true
    });
    *voice.installing.lock() = None;
    result
}

/// Replaces the ONNX Runtime an earlier Cadence downloaded (Windows: without DirectML) with this
/// one, before it's loaded: once per run of Cadence, when a song is first listened to (25 MB,
/// shown like the first download). If that fails, the old one still listens, on the processor.
#[cfg(desktop)]
fn update_runtime(ctx: &Ctx, voice: &Voice) {
    static TRIED: AtomicBool = AtomicBool::new(false);
    if runtime::loaded() || !models::outdated_runtime(&voice.models_dir) || TRIED.swap(true, Ordering::Relaxed) {
        return;
    }
    if !start_download(voice) {
        return;
    }
    log::info!("updating ONNX Runtime, so the voice models can run on the graphics card");
    if let Err(e) = download(ctx, voice) {
        log::warn!("couldn't update ONNX Runtime ({e:#}); listening on the processor");
    }
    ctx.emit("voice-install", (0u64, 0u64));
}

pub fn remove(voice: &Voice) {
    #[cfg(desktop)]
    models::remove(&voice.models_dir);
    #[cfg(not(desktop))]
    let _ = voice;
}

#[cfg(all(test, desktop))]
mod tests {
    use super::*;

    /// Listens to a real song with the real models and compares the placed lines with the LRC.
    /// `CADENCE_VOICE_LOCAL` = a folder with `models/` (as downloaded), `CADENCE_VOICE_SONG` = the
    /// audio file, `CADENCE_VOICE_LYRICS` = a saved lyrics file (`lyrics/<id>.json`).
    #[test]
    #[ignore]
    fn listen_to_a_real_song() {
        let local = PathBuf::from(std::env::var("CADENCE_VOICE_LOCAL").unwrap());
        let song = PathBuf::from(std::env::var("CADENCE_VOICE_SONG").unwrap());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(std::env::var("CADENCE_VOICE_LYRICS").unwrap()).unwrap()).unwrap();
        let lyrics: Lyrics = serde_json::from_value(saved["lyrics"].clone()).unwrap();
        let voice = Voice::new(&local, &local);
        assert!(voice.installed(), "models missing in {}", voice.models_dir.display());
        let started = std::time::Instant::now();
        let mut steps = 0;
        let mut opened = Vec::new();
        let em = listen::listen(
            &song,
            |separator| {
                let at = started.elapsed();
                let model = open_model(&voice, separator)?;
                opened.push((separator, model.1, at, started.elapsed()));
                Ok(model)
            },
            || {
                steps += 1;
                true
            },
        )
        .unwrap()
        .0;
        let end = started.elapsed();
        println!("listened in {end:?} ({steps} steps, {} frames)", em.frames);
        // (separator, on the GPU, asked at, opened at): decoding, then each model's load and work.
        if let [(_, sep_gpu, s0, s1), (_, let_gpu, l0, l1)] = opened[..] {
            println!(
                "decode {s0:.1?}; separator on {} load {:.1?} work {:.1?}; letters on {} load {:.1?} work {:.1?}",
                if sep_gpu { "GPU" } else { "CPU" },
                s1 - s0,
                l0 - s1,
                if let_gpu { "GPU" } else { "CPU" },
                l1 - l0,
                end - l1
            );
        }
        listen::save(&local.join("listened.bin"), "test", &em).unwrap();
        let em = listen::load(&local.join("listened.bin"), "test").unwrap();
        let texts = text_of(&lyrics);
        let times: Vec<f64> = lyrics.synced.iter().map(|l| l.time).collect();
        for (name, t) in [("without LRC times", None), ("guided by LRC times", Some(&times[..]))] {
            let started = std::time::Instant::now();
            let out = align::align(&em, &texts, t).unwrap();
            let mut errs: Vec<f64> = out
                .lines
                .iter()
                .zip(&lyrics.synced)
                .filter(|(a, _)| !a.words.is_empty())
                .map(|(a, l)| a.time - l.time - out.offset)
                .collect();
            errs.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
            let within = |s: f64| errs.iter().filter(|e| e.abs() <= s).count() * 100 / errs.len().max(1);
            println!(
                "{name}: {} of {} lines placed in {:?}, offset {:+.2} s, median |err| {:.2} s, ≤0.3 s {}%, ≤0.5 s {}%, ≤1 s {}%",
                out.placed,
                out.lines_with_words,
                started.elapsed(),
                out.offset,
                errs.get(errs.len() / 2).map_or(0.0, |e| e.abs()),
                within(0.3),
                within(0.5),
                within(1.0)
            );
            if t.is_some() {
                let mut lrc = String::new();
                for l in &out.lines {
                    let ts = |s: f64| format!("{:02}:{:05.2}", (s / 60.0) as u32, s % 60.0);
                    lrc.push_str(&format!("[{}]", ts(l.time)));
                    if l.words.is_empty() {
                        lrc.push_str(&l.text);
                    }
                    for w in &l.words {
                        lrc.push_str(&format!("<{}>{}", ts(w.time), w.text));
                    }
                    if let Some(end) = l.words.last().and_then(|w| w.end) {
                        lrc.push_str(&format!("<{}>", ts(end)));
                    }
                    lrc.push('\n');
                }
                std::fs::write(local.join("placed.lrc"), lrc).unwrap();
            }
        }
    }
}

#[cfg(all(test, desktop))]
mod sing_tests {
    use super::*;

    /// Tells the singers of a real song apart: `CADENCE_VOICE_LOCAL` = a folder with `models/`,
    /// `CADENCE_VOICE_SONG` = the audio file, `CADENCE_VOICE_PLACED` = its `lyrics/<id>.voice.json`.
    /// The voices are kept in `CADENCE_VOICE_LOCAL/<song name>.sing`, so a second run is quick.
    #[test]
    #[ignore]
    fn split_a_real_song() {
        let local = PathBuf::from(std::env::var("CADENCE_VOICE_LOCAL").unwrap());
        let song = PathBuf::from(std::env::var("CADENCE_VOICE_SONG").unwrap());
        let placed: Placed = serde_json::from_slice(&std::fs::read(std::env::var("CADENCE_VOICE_PLACED").unwrap()).unwrap()).unwrap();
        let cached = local.join(format!("{}.sing", song.file_stem().unwrap().to_string_lossy()));
        let voices = sing::load(&cached, "test").unwrap_or_else(|| {
            let voice = Voice::new(&local, &local);
            let started = std::time::Instant::now();
            let v = listen::listen_voices(&song, |sep| open_model(&voice, sep), || true).unwrap();
            println!("separated and described in {:?}", started.elapsed());
            sing::save(&cached, "test", &v).unwrap();
            v
        });
        let started = std::time::Instant::now();
        let split = sing::split(&voices, &placed.lines);
        println!("split in {:?}", started.elapsed());
        let Some(split) = split else {
            println!("one singer");
            return;
        };
        println!("voices at {:.1} / {:.1} semitones above 55 Hz", split.pitch[0], split.pitch[1]);
        for (l, g) in placed.lines.iter().zip(&split.lines) {
            let who = match g {
                Some(0) => "LOW ",
                Some(1) => "HIGH",
                Some(2) => "BOTH",
                _ => " ?  ",
            };
            println!("{:7.2} {who} {}", l.time, l.text);
        }
    }

}

#[cfg(test)]
mod credit_tests {
    #[test]
    fn several_singers_in_the_credits() {
        use super::credits_several as several;
        assert!(several("Song (feat. Someone)", "A"));
        assert!(several("Song", "A ft. B"));
        assert!(several("Song", "A with B"));
        assert!(several("Song", "A × B"));
        assert!(several("Song", "A x B"));
        assert!(several("Song (Duet ver.)", "A"));
        // Band names and ordinary titles: one credit.
        assert!(!several("Daydream", "Judy & Mary"));
        assert!(!several("This Train", "Bob Marley & The Wailers"));
        assert!(!several("Dancing with Myself", "Billy Idol"));
        assert!(!several("Forget it", "Yorushika"));
        assert!(!several("X", "X Japan"));
        assert!(!several("Malcolm X", "A"));
    }
}

#[cfg(all(test, desktop))]
mod sing_survey {
    use super::*;

    /// How every song with saved voices is split (read-only): `CADENCE_DATA` = the roaming data
    /// folder (library.json, lyrics/), `CADENCE_LOCAL` = the local one (voice/).
    #[test]
    #[ignore]
    fn survey_library() {
        let data = PathBuf::from(std::env::var("CADENCE_DATA").unwrap());
        let local = PathBuf::from(std::env::var("CADENCE_LOCAL").unwrap());
        #[derive(Deserialize)]
        struct Lib {
            tracks: Vec<Track>,
        }
        let lib: Lib = serde_json::from_slice(&std::fs::read(data.join("library.json")).unwrap()).unwrap();
        for t in &lib.tracks {
            let Some(v) = sing::load(&local.join("voice").join(format!("{}.sing", t.id)), &crate::sonic::stamp(t)) else { continue };
            let Ok(bytes) = std::fs::read(data.join("lyrics").join(format!("{}.voice.json", t.id))) else { continue };
            let Ok(placed) = serde_json::from_slice::<Placed>(&bytes) else { continue };
            sing::LAST.with(|l| *l.borrow_mut() = None);
            let split = sing::split(&v, &placed.lines);
            let s = sing::LAST.with(|l| *l.borrow());
            match s {
                Some(s) => println!(
                    "{:5} gap {:5.1} sil {:.2} timbre {:5.2} groups {:2}/{:2}  {} — {}",
                    if split.is_some() { "SPLIT" } else { "one" },
                    s.gap,
                    s.silhouette,
                    s.timbre,
                    s.n0,
                    s.n1,
                    t.artist,
                    t.title
                ),
                None => println!("  -   (too little to measure)  {} — {}", t.artist, t.title),
            }
        }
    }
}

#[cfg(all(test, desktop))]
mod batch_tests {
    use super::*;

    /// How long finding the songs with lyrics takes on a real library (read-only).
    /// `CADENCE_DATA` = a Cadence data folder (library.json + lyrics/).
    #[test]
    #[ignore]
    fn finding_songs_with_lyrics() {
        let data = PathBuf::from(std::env::var("CADENCE_DATA").unwrap());
        #[derive(Deserialize)]
        struct Lib {
            tracks: Vec<Track>,
        }
        let lib: Lib = serde_json::from_slice(&std::fs::read(data.join("library.json")).unwrap()).unwrap();
        let cache = data.join("lyrics");
        let started = std::time::Instant::now();
        let with = lib.tracks.iter().filter(|t| has_lyrics(&cache, t)).count();
        println!("{with} of {} songs have lyrics; checked in {:?}", lib.tracks.len(), started.elapsed());
    }
}
