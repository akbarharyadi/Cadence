//! Find a song by a line you remember: the words of every saved song's lyrics, searchable as
//! written (case, accents and punctuation folded away) and as read in Latin letters (romaji,
//! pinyin, Korean romanized), so "ame ga" finds a line written 雨が.
//!
//! Each song's lines are kept in two strings (what's searched, and the lines as written) rather
//! than one allocation per line; a search runs a SIMD substring finder over them. The index is
//! made once in the background (one low-priority thread, shown in the sidebar) the first time a
//! lyric is searched, kept in `lyric-index.txt` (a cache, not backed up), and from then on only
//! songs whose lyrics changed are read again. Readings of Japanese need the large dictionary: it
//! is loaded for a batch and let go afterwards (`romanize::readings_for_index`).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;

use serde::Serialize;

/// One song's lines.
#[derive(Clone, Debug, Default)]
struct Song {
    id: String,
    /// Where these lines come from and which version: "c:<modified>:<size>" for the lyrics cache
    /// file, "f:<hash>" for lyrics in the song's own file (seen in the lyrics view).
    stamp: String,
    /// Per line, '\n' between lines: the line folded, '\u{1}', its reading folded without spaces.
    hay: String,
    /// The lines as written, '\n' between them.
    text: String,
    /// Seconds; negative for untimed lyrics.
    times: Vec<f32>,
}

/// A line found.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub track_id: String,
    /// The line as written.
    pub line: String,
    /// Where it's sung, in seconds (None: the lyrics aren't timed).
    pub time: Option<f64>,
}

#[derive(Default)]
pub struct Index {
    songs: Vec<Song>,
    by_id: HashMap<String, usize>,
}

/// Folded for searching (as the window folds: NFKD, marks dropped, lower case), every run of
/// anything but letters and digits one space.
pub fn norm(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    use unicode_normalization::char::is_combining_mark;
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.nfkd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn squeeze(s: &str) -> String {
    s.chars().filter(|c| *c != ' ').collect()
}

/// A line as written, safe to keep between the separators.
fn clean(s: &str) -> String {
    s.trim().chars().map(|c| if matches!(c, '\n' | '\r' | '\t' | '\u{1}') { ' ' } else { c }).collect()
}

impl Index {
    pub fn len(&self) -> usize {
        self.songs.len()
    }

    pub fn lines(&self) -> usize {
        self.songs.iter().map(|s| s.times.len()).sum()
    }

    /// Bytes of text held (the searched strings, the lines, their times).
    pub fn bytes(&self) -> usize {
        self.songs.iter().map(|s| s.id.len() + s.stamp.len() + s.hay.capacity() + s.text.capacity() + s.times.capacity() * 4 + std::mem::size_of::<Song>()).sum()
    }

    pub fn stamp(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).map(|&i| self.songs[i].stamp.as_str())
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.songs.iter().map(|s| s.id.as_str())
    }

    /// A song's lines: (seconds or negative, the line as written, its reading in Latin letters
    /// or ""). Replaces what it had; no lines with words removes it.
    pub fn put(&mut self, id: &str, stamp: &str, lines: &[(f64, String, String)]) {
        let mut song = Song { id: id.to_string(), stamp: stamp.to_string(), ..Default::default() };
        for (time, text, reading) in lines {
            let text = clean(text);
            let folded = norm(&text);
            if folded.is_empty() {
                continue;
            }
            if !song.times.is_empty() {
                song.hay.push('\n');
                song.text.push('\n');
            }
            song.hay.push_str(&folded);
            song.hay.push('\u{1}');
            song.hay.push_str(&squeeze(&norm(reading)));
            song.text.push_str(&text);
            song.times.push(if time.is_finite() && *time >= 0.0 { *time as f32 } else { -1.0 });
        }
        if song.times.is_empty() {
            self.remove(id);
            return;
        }
        song.hay.shrink_to_fit();
        song.text.shrink_to_fit();
        song.times.shrink_to_fit();
        match self.by_id.get(id) {
            Some(&i) => self.songs[i] = song,
            None => {
                self.by_id.insert(id.to_string(), self.songs.len());
                self.songs.push(song);
            }
        }
    }

    pub fn remove(&mut self, id: &str) {
        let Some(i) = self.by_id.remove(id) else { return };
        self.songs.swap_remove(i);
        if let Some(moved) = self.songs.get(i) {
            self.by_id.insert(moved.id.clone(), i);
        }
    }

    /// Songs with a line holding every word of `query` (as written, or as read in Latin
    /// letters), best first: the words together as typed, then the words anywhere in the line,
    /// then by reading. One line per song (its best, the first of those), at most `limit` songs.
    /// `skip`: songs not to list (not in the library, lyrics being changed).
    pub fn search(&self, query: &str, limit: usize, skip: &dyn Fn(&str) -> bool) -> Vec<Hit> {
        let q = norm(query);
        if q.chars().count() < 2 || limit == 0 {
            return Vec::new();
        }
        let words: Vec<&str> = q.split(' ').collect();
        let q_read = squeeze(&q);
        let longest = words.iter().copied().max_by_key(|w| w.len()).unwrap_or(&q);
        let finder = memchr::memmem::Finder::new(longest.as_bytes());
        // (rank, order, song, line)
        let mut found: Vec<(u8, usize, usize, usize)> = Vec::new();
        let mut best_kind = 0;
        for (si, song) in self.songs.iter().enumerate() {
            if skip(&song.id) {
                continue;
            }
            let hay = song.hay.as_bytes();
            let mut best: Option<(u8, usize)> = None;
            let mut last_line_start = usize::MAX;
            for pos in finder.find_iter(hay) {
                let start = memchr::memrchr(b'\n', &hay[..pos]).map_or(0, |i| i + 1);
                if start == last_line_start {
                    continue;
                }
                last_line_start = start;
                let end = memchr::memchr(b'\n', &hay[pos..]).map_or(hay.len(), |i| pos + i);
                let line = &song.hay[start..end];
                let (written, read) = line.split_once('\u{1}').unwrap_or((line, ""));
                let rank = if written.contains(q.as_str()) {
                    0
                } else if words.iter().all(|w| written.contains(w)) {
                    1
                } else if !read.is_empty() && read.contains(q_read.as_str()) {
                    2
                } else if !read.is_empty() && words.iter().all(|w| read.contains(w)) {
                    3
                } else {
                    continue;
                };
                if best.is_none_or(|(r, _)| rank < r) {
                    let line_no = memchr::memchr_iter(b'\n', &hay[..start]).count();
                    best = Some((rank, line_no));
                }
                if rank == 0 {
                    break;
                }
            }
            if let Some((rank, line)) = best {
                found.push((rank, found.len(), si, line));
                if rank == 0 {
                    best_kind += 1;
                }
                // Enough of the best kind: nothing later ranks higher.
                if best_kind >= limit {
                    break;
                }
            }
        }
        found.sort_by_key(|&(rank, order, _, _)| (rank, order));
        found
            .into_iter()
            .take(limit)
            .map(|(_, _, si, li)| {
                let s = &self.songs[si];
                let t = s.times.get(li).copied().unwrap_or(-1.0);
                Hit { track_id: s.id.clone(), line: s.text.split('\n').nth(li).unwrap_or("").to_string(), time: (t >= 0.0).then_some(t as f64) }
            })
            .collect()
    }

    // ---------- kept on disk ----------

    const HEADER: &'static str = "cadence-lyric-index 1";

    /// Written as text: a header, then per song "@id\tstamp", then per line
    /// "seconds\tline\tsearched" (the searched string with '\u{1}' as written in memory).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            writeln!(f, "{}", Self::HEADER)?;
            for s in &self.songs {
                writeln!(f, "@{}\t{}", s.id, s.stamp)?;
                for ((t, text), hay) in s.times.iter().zip(s.text.split('\n')).zip(s.hay.split('\n')) {
                    writeln!(f, "{t}\t{text}\t{hay}")?;
                }
            }
            f.flush()?;
        }
        std::fs::rename(&tmp, path)
    }

    /// The index kept on disk; None when there's none or it's of another version or damaged.
    pub fn load(path: &Path) -> Option<Index> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut rows = text.lines();
        if rows.next()? != Self::HEADER {
            return None;
        }
        let mut idx = Index::default();
        let mut cur: Option<Song> = None;
        let finish = |idx: &mut Index, s: Option<Song>| {
            if let Some(s) = s.filter(|s| !s.times.is_empty()) {
                idx.by_id.insert(s.id.clone(), idx.songs.len());
                idx.songs.push(s);
            }
        };
        for row in rows {
            if let Some(head) = row.strip_prefix('@') {
                finish(&mut idx, cur.take());
                let (id, stamp) = head.split_once('\t')?;
                cur = Some(Song { id: id.to_string(), stamp: stamp.to_string(), ..Default::default() });
                continue;
            }
            let s = cur.as_mut()?;
            let mut f = row.splitn(3, '\t');
            let (t, line, hay) = (f.next()?.parse::<f32>().ok()?, f.next()?, f.next()?);
            if !s.times.is_empty() {
                s.text.push('\n');
                s.hay.push('\n');
            }
            s.times.push(t);
            s.text.push_str(line);
            s.hay.push_str(hay);
        }
        finish(&mut idx, cur);
        for s in &mut idx.songs {
            s.hay.shrink_to_fit();
            s.text.shrink_to_fit();
        }
        Some(idx)
    }
}

// ---------- in the app ----------

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::ctx::Ctx;

/// The index, once loaded (None until the first lyric search).
static INDEX: Mutex<Option<Index>> = Mutex::new(None);
static BUILDING: AtomicBool = AtomicBool::new(false);
/// Songs whose lyrics changed since: read again from the lyrics cache (None), or these lines from
/// the song's own file (as the lyrics view got them).
static CHANGED: Mutex<Option<HashMap<String, Option<(String, Vec<(f64, String)>)>>>> = Mutex::new(None);

fn file(dir: &Path) -> std::path::PathBuf {
    dir.join("lyric-index.txt")
}

/// The index is let go of after this long without a lyric search (it is read back from
/// `lyric-index.txt` on the next one): tens of MB for a big library aren't kept for nothing.
const IDLE: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// What changed since the index was last written is kept on disk once this many songs changed, or
/// this long after the last write (and always before the index is let go of), not after every
/// change: the file is rewritten whole.
const SAVE_AFTER_SONGS: usize = 200;
const SAVE_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

struct Use {
    /// The last search.
    searched: Option<std::time::Instant>,
    /// Songs changed in memory and not written yet, and when it was last written.
    unsaved: usize,
    saved: Option<std::time::Instant>,
}

static USE: Mutex<Use> = Mutex::new(Use { searched: None, unsaved: 0, saved: None });

/// Whether a loaded index should be let go of now.
fn idle_too_long(loaded: bool, building: bool, since_search: Option<std::time::Duration>) -> bool {
    loaded && !building && since_search.is_none_or(|d| d >= IDLE)
}

/// Whether the changes kept in memory should be written now.
fn save_due(unsaved: usize, since_save: Option<std::time::Duration>, letting_go: bool) -> bool {
    unsaved > 0 && (letting_go || unsaved >= SAVE_AFTER_SONGS || since_save.is_none_or(|d| d >= SAVE_AFTER))
}

/// Checked when lyrics change (no timer): an index not searched for a while is let go of (written
/// first if it has changes), on the background pool.
fn let_go_if_idle(ctx: &Ctx) {
    let since = USE.lock().searched.map(|t| t.elapsed());
    if !idle_too_long(INDEX.lock().is_some(), BUILDING.load(Ordering::SeqCst), since) || BUILDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    let asked = USE.lock().searched;
    in_turn(move || {
        let dir = ctx.state.data_dir.clone();
        let taken = INDEX.lock().take();
        if let Some(idx) = taken {
            let unsaved = std::mem::take(&mut USE.lock().unsaved);
            if save_due(unsaved, None, true)
                && let Err(e) = idx.save(&file(&dir))
            {
                log::warn!("couldn't keep the lyrics index: {e}");
            }
            log::info!("lyrics index let go of after {} min without a search", IDLE.as_secs() / 60);
        }
        BUILDING.store(false, Ordering::SeqCst);
        // A search came meanwhile (it found nothing loaded and is waiting): made again for it.
        if USE.lock().searched != asked {
            start(ctx);
        }
    });
}

/// A song's saved lyrics changed (picked, pasted, hidden, looked up): its lines are read again.
pub fn touched(track_id: &str) {
    CHANGED.lock().get_or_insert_with(HashMap::new).insert(track_id.to_string(), None);
    if let Some(ctx) = APP.lock().clone() {
        let_go_if_idle(&ctx);
    }
}

/// The app the index belongs to (set by its first search).
static APP: Mutex<Option<Ctx>> = Mutex::new(None);

/// The lyrics view showed a song's lyrics: those from its own file (embedded or a .lrc beside it)
/// are indexed too, the first time and when they change.
pub fn seen(track_id: &str, l: &crate::lyrics::Lyrics) {
    if !matches!(l.source.as_str(), "embedded" | "file") || l.is_empty() {
        return;
    }
    let lines = lines_of(l);
    let stamp = format!("f:{}", crate::library::short_hash(&lines.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n")));
    if INDEX.lock().as_ref().and_then(|i| i.stamp(track_id).map(str::to_string)).as_deref() == Some(stamp.as_str()) {
        return;
    }
    CHANGED.lock().get_or_insert_with(HashMap::new).insert(track_id.to_string(), Some((stamp, lines)));
    if let Some(ctx) = APP.lock().clone() {
        let_go_if_idle(&ctx);
    }
}

/// The lines of a song's saved lyrics, as the lyrics view would show them: none when they're
/// hidden, or saved for another language version of the title.
fn cached_lines(cache: &Path, id: &str, title: &str) -> Option<Vec<(f64, String)>> {
    if crate::lyrics::is_hidden(cache, id) {
        return None;
    }
    crate::lyrics::saved(cache, id, title).map(|l| lines_of(&l))
}

fn lines_of(l: &crate::lyrics::Lyrics) -> Vec<(f64, String)> {
    if l.instrumental {
        return Vec::new();
    }
    if !l.synced.is_empty() {
        l.synced.iter().filter(|x| !x.text.trim().is_empty()).map(|x| (x.time, x.text.clone())).collect()
    } else {
        l.plain.lines().filter(|x| !x.trim().is_empty()).map(|x| (-1.0, x.to_string())).collect()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// The index is ready (false: being made; "lyric-index-ready" says when to search again).
    pub ready: bool,
    pub hits: Vec<Hit>,
}

#[derive(Clone, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub running: bool,
}

/// Searches the lyrics; the first call starts making the index (in the background).
pub fn search(ctx: &Ctx, query: &str, limit: usize) -> Found {
    USE.lock().searched = Some(std::time::Instant::now());
    *APP.lock() = Some(ctx.clone());
    let changed: HashSet<String> = CHANGED.lock().as_ref().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let ready = INDEX.lock().is_some();
    if !ready || (!changed.is_empty() && !BUILDING.load(Ordering::SeqCst)) {
        start(ctx.clone());
    }
    if !ready {
        return Found { ready: false, hits: Vec::new() };
    }
    let started = std::time::Instant::now();
    // More than asked for: some may be gone from the library.
    let hits = INDEX.lock().as_ref().map(|i| i.search(query, limit * 2 + 10, &|id| changed.contains(id))).unwrap_or_default();
    let hits: Vec<Hit> = {
        let lib = ctx.state.library.lock();
        hits.into_iter().filter(|h| lib.by_id.contains_key(&h.track_id)).take(limit).collect()
    };
    log::debug!("lyric search {:?}: {} songs in {:?}", query, hits.len(), started.elapsed());
    Found { ready: true, hits }
}

/// Runs the index's work (`BUILDING` set: one at a time) on one of the shared pool's threads (at
/// most two heavy threads in all), taking a turn there: reading lyrics files is light and bounded,
/// and it starts between two songs of a long analysis (hours on a first run) rather than after it,
/// so "find a song by a line" doesn't say for hours that the index isn't ready.
fn in_turn(work: impl FnOnce() + Send + 'static) {
    crate::jobs::run_soon(work);
}

/// Songs to read per batch (a dictionary load each for Japanese; progress between them).
const BATCH: usize = 400;

/// Loads the index kept on disk, then reads the songs whose lyrics changed since it was saved,
/// in a turn on the shared pool (`in_turn`).
fn start(ctx: Ctx) {
    if BUILDING.swap(true, Ordering::SeqCst) {
        return;
    }
    in_turn(move || {
        let started = std::time::Instant::now();
        let dir = ctx.state.data_dir.clone();
        let cache = dir.join("lyrics");
        let first = INDEX.lock().is_none();
        if first {
            let idx = Index::load(&file(&dir)).unwrap_or_default();
            *INDEX.lock() = Some(idx);
        }
        // What to read again: cache files whose stamp changed (on the first run of a session),
        // and the songs said to have changed.
        let mut todo: HashMap<String, Option<(String, Vec<(f64, String)>)>> = CHANGED.lock().take().unwrap_or_default();
        if first {
            let on_disk = cache_stamps(&cache);
            let idx = INDEX.lock();
            let idx = idx.as_ref().expect("loaded");
            for (id, stamp) in &on_disk {
                if idx.stamp(id) != Some(stamp.as_str()) && !todo.contains_key(id) {
                    todo.insert(id.clone(), None);
                }
            }
            // Gone from the cache: no longer there (lyrics from the songs' own files stay).
            for id in idx.ids() {
                if idx.stamp(id).is_some_and(|s| s.starts_with("c:")) && !on_disk.contains_key(id) {
                    todo.entry(id.to_string()).or_insert(None);
                }
            }
        }
        let total = todo.len();
        if total > 0 {
            ctx.emit("lyric-index-progress", Progress { done: 0, total, running: true });
        }
        let titles: HashMap<String, String> = {
            let lib = ctx.state.library.lock();
            todo.keys().filter_map(|id| lib.by_id.get(id).map(|&i| (id.clone(), lib.snapshot.tracks[i].title.clone()))).collect()
        };
        let stamps = cache_stamps_of(&cache, todo.keys());
        let todo: Vec<_> = todo.into_iter().collect();
        let mut done = 0;
        for batch in todo.chunks(BATCH) {
            // Each song's lines and where they're from (None: nothing to index any more).
            let read: Vec<(String, Option<(String, Vec<(f64, String)>)>)> = batch
                .iter()
                .map(|(id, given)| {
                    let found = match given {
                        Some(g) => Some(g.clone()),
                        None => titles.get(id).and_then(|title| Some((stamps.get(id).cloned().unwrap_or_default(), cached_lines(&cache, id, title)?))),
                    };
                    (id.clone(), found.filter(|(_, l)| !l.is_empty()))
                })
                .collect();
            let texts: Vec<Vec<String>> = read.iter().map(|(_, f)| f.as_ref().map(|(_, l)| l.iter().map(|(_, t)| t.clone()).collect()).unwrap_or_default()).collect();
            let readings = crate::romanize::readings_for_index(&texts);
            {
                let mut idx = INDEX.lock();
                let idx = idx.as_mut().expect("loaded");
                for ((id, f), reading) in read.into_iter().zip(readings) {
                    match f {
                        Some((stamp, lines)) => {
                            let rows: Vec<(f64, String, String)> = lines.into_iter().zip(reading).map(|((t, text), r)| (t, text, r)).collect();
                            idx.put(&id, &stamp, &rows);
                        }
                        None => idx.remove(&id),
                    }
                }
            }
            done += batch.len();
            ctx.emit("lyric-index-progress", Progress { done, total, running: done < total });
        }
        if total > 0 || first {
            // Songs no longer in the library are left out of what's kept.
            let keep: HashSet<String> = ctx.state.library.lock().by_id.keys().cloned().collect();
            let mut idx = INDEX.lock();
            if let Some(idx) = idx.as_mut() {
                let gone: Vec<String> = idx.ids().filter(|id| !keep.contains(*id)).map(str::to_string).collect();
                for id in &gone {
                    idx.remove(id);
                }
                // The first load of a session is written at once (what was read again since the
                // file was made); later changes are written in batches (`save_due`).
                let mut u = USE.lock();
                u.unsaved += total + gone.len();
                if (first && u.unsaved > 0) || save_due(u.unsaved, u.saved.map(|t| t.elapsed()), false) {
                    match idx.save(&file(&dir)) {
                        Ok(()) => {
                            u.unsaved = 0;
                            u.saved = Some(std::time::Instant::now());
                        }
                        Err(e) => log::warn!("couldn't keep the lyrics index: {e}"),
                    }
                }
                log::info!("lyrics index: {} songs, {} lines, {} KB; {total} read again in {:?}", idx.len(), idx.lines(), idx.bytes() / 1024, started.elapsed());
            }
        }
        BUILDING.store(false, Ordering::SeqCst);
        ctx.emit("lyric-index-progress", Progress { done: total, total, running: false });
        ctx.emit("lyric-index-ready", ());
    });
}

/// The lyrics cache's song files (`<id>.json`, not `<id>.voice.json` and the like) and their stamps.
fn cache_stamps(cache: &Path) -> HashMap<String, String> {
    let Ok(rd) = std::fs::read_dir(cache) else { return HashMap::new() };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".json").filter(|id| !id.contains('.'))?.to_string();
            Some((id, stamp_of(&e.metadata().ok()?)))
        })
        .collect()
}

fn cache_stamps_of<'a>(cache: &Path, ids: impl Iterator<Item = &'a String>) -> HashMap<String, String> {
    ids.filter_map(|id| Some((id.clone(), stamp_of(&std::fs::metadata(cache.join(format!("{id}.json"))).ok()?)))).collect()
}

fn stamp_of(m: &std::fs::Metadata) -> String {
    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
    format!("c:{t}:{}", m.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented lines (no real lyrics in fixtures).
    fn row(t: f64, text: &str, reading: &str) -> (f64, String, String) {
        (t, text.to_string(), reading.to_string())
    }
    fn none(_: &str) -> bool {
        false
    }
    fn small() -> Index {
        let mut idx = Index::default();
        idx.put("en", "c:1:1", &[row(12.5, "The rain is falling today", ""), row(20.0, "I opened the window, slowly", "")]);
        idx.put("ja", "c:1:2", &[row(5.0, "青い空を見上げた", "aoi sora wo miageta"), row(9.0, "今日は雨が降る", "kyou wa ame ga furu")]);
        idx.put("id", "f:x", &[row(-1.0, "Hari ini hujan turun", "")]);
        idx
    }

    #[test]
    fn finds_a_line_by_a_few_words() {
        let idx = small();
        let hits = idx.search("rain fall", 10, &none);
        assert_eq!(hits, [Hit { track_id: "en".into(), line: "The rain is falling today".into(), time: Some(12.5) }]);
        // Case, punctuation and word order don't matter; the line comes back as written.
        assert_eq!(idx.search("WINDOW slowly", 10, &none)[0].line, "I opened the window, slowly");
        assert_eq!(idx.search("window slowly", 10, &none)[0].time, Some(20.0));
        // As written in Japanese, part of a line.
        assert_eq!(idx.search("雨が", 10, &none)[0].track_id, "ja");
        // By its reading, with or without the spaces.
        assert_eq!(idx.search("ame ga", 10, &none)[0].line, "今日は雨が降る");
        assert_eq!(idx.search("amegafu", 10, &none)[0].time, Some(9.0));
        assert_eq!(idx.search("sora miage", 10, &none)[0].line, "青い空を見上げた");
        // Untimed lyrics: no time.
        assert_eq!(idx.search("hujan", 10, &none)[0].time, None);
        // Too short, or not there.
        assert!(idx.search("a", 10, &none).is_empty());
        assert!(idx.search("thunder", 10, &none).is_empty());
    }

    #[test]
    fn the_words_together_rank_first() {
        let mut idx = Index::default();
        idx.put("a", "c", &[row(1.0, "today the falling rain", "")]);
        idx.put("b", "c", &[row(2.0, "the rain falling", "")]);
        let hits = idx.search("rain falling", 10, &none);
        assert_eq!(hits.iter().map(|h| h.track_id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(idx.search("rain falling", 1, &none).len(), 1);
    }

    #[test]
    fn follows_changes_and_leaves_out_what_its_told_to() {
        let mut idx = small();
        idx.put("en", "c:2:1", &[row(3.0, "the sky is blue", "")]);
        assert!(idx.search("rain fall", 10, &none).is_empty());
        assert_eq!(idx.search("sky blue", 10, &none)[0].time, Some(3.0));
        assert_eq!(idx.stamp("en"), Some("c:2:1"));
        // No lines (hidden lyrics, instrumental): gone.
        idx.put("ja", "c:3", &[]);
        assert!(idx.search("ame", 10, &none).is_empty());
        assert_eq!(idx.len(), 2);
        idx.remove("id");
        assert!(idx.search("hujan", 10, &none).is_empty());
        assert_eq!(idx.search("sky", 10, &none).len(), 1);
        // Songs being changed (or gone from the library) aren't listed.
        assert!(idx.search("sky", 10, &|id| id == "en").is_empty());
    }

    #[test]
    fn hidden_lyrics_are_not_indexed() {
        let dir = tempfile::tempdir().unwrap();
        let l = crate::lyrics::from_pasted("[00:01.00]the rain is falling today").unwrap();
        crate::lyrics::pick(dir.path(), "s1", &l);
        assert_eq!(cached_lines(dir.path(), "s1", "Test Song").unwrap(), [(1.0, "the rain is falling today".to_string())]);
        crate::lyrics::hide(dir.path(), "s1");
        assert!(cached_lines(dir.path(), "s1", "Test Song").is_none());
        // Lyrics from the song's own file come in through `seen`; hidden ones have no source.
        assert!(lines_of(&crate::lyrics::Lyrics { lookup: "hidden".into(), ..Default::default() }).is_empty());
    }

    #[test]
    fn let_go_of_when_idle_and_written_in_batches() {
        let min = |m: u64| Some(std::time::Duration::from_secs(m * 60));
        assert!(!idle_too_long(true, false, min(1)), "searched a minute ago: kept");
        assert!(idle_too_long(true, false, min(16)));
        assert!(idle_too_long(true, false, None), "loaded but never searched this session");
        assert!(!idle_too_long(true, true, min(60)), "never while it's being made");
        assert!(!idle_too_long(false, false, min(60)));
        assert!(!save_due(0, min(60), true), "nothing to write");
        assert!(!save_due(3, min(1), false), "a few changes wait");
        assert!(save_due(SAVE_AFTER_SONGS, min(1), false));
        assert!(save_due(3, min(11), false));
        assert!(save_due(1, min(1), true), "always before it's let go of");
    }

    #[test]
    fn kept_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lyric-index.txt");
        let idx = small();
        idx.save(&path).unwrap();
        let back = Index::load(&path).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back.stamp("id"), Some("f:x"));
        assert_eq!(back.search("ame ga", 10, &none), idx.search("ame ga", 10, &none));
        assert_eq!(back.search("window", 10, &none), idx.search("window", 10, &none));
        // Another version or a damaged file: made again.
        std::fs::write(&path, "something else\n").unwrap();
        assert!(Index::load(&path).is_none());
    }

    /// A library like the owner's: 7,000 songs with lyrics, 43 lines each (~300,000 lines), a
    /// third in Japanese (with readings), the rest in Latin letters. Every query stays far under
    /// the time a key press allows, in a debug build.
    #[test]
    fn query_time_on_300k_lines() {
        let corpus = synthetic(7_000, 43);
        let started = std::time::Instant::now();
        let mut idx = Index::default();
        for (id, rows) in &corpus {
            idx.put(id, "c", rows);
        }
        let built = started.elapsed();
        assert_eq!(idx.lines(), 7_000 * 43);
        let queries = ["window today", "falling slowly", "ame ga", "空を", "zzzz never", "song 6998 line 42", "the", "blue sky number"];
        let mut worst = std::time::Duration::ZERO;
        for q in queries {
            let t = std::time::Instant::now();
            let hits = idx.search(q, 20, &none);
            worst = worst.max(t.elapsed());
            assert!(hits.len() <= 20);
        }
        assert_eq!(idx.search("song 6998 line 42", 5, &none)[0].track_id, "6998");
        println!("lyric index: {} lines, {} KB, built in {built:?}; slowest query {worst:?}", idx.lines(), idx.bytes() / 1024);
        assert!(worst < std::time::Duration::from_millis(250), "slowest query {worst:?}");
    }

    pub(super) fn synthetic(songs: usize, lines: usize) -> Vec<(String, Vec<(f64, String, String)>)> {
        let en = ["the rain is falling today", "I opened the window slowly", "we walk to school together", "the blue sky number", "a cup of tea at noon"];
        let ja = [("青い空を見上げた", "aoi sora wo miageta"), ("今日は雨が降る", "kyou wa ame ga furu"), ("窓を開けた", "mado wo aketa")];
        (0..songs)
            .map(|s| {
                let rows = (0..lines)
                    .map(|l| {
                        let t = l as f64 * 4.0;
                        if s % 3 == 0 {
                            let (text, read) = ja[(s + l) % ja.len()];
                            (t, format!("{text} {s} {l}"), format!("{read} {s} {l}"))
                        } else {
                            (t, format!("{} song {s} line {l}", en[(s + l) % en.len()]), String::new())
                        }
                    })
                    .collect();
                (s.to_string(), rows)
            })
            .collect()
    }
}
