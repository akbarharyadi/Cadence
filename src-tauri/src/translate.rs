//! Translations of a song's lyrics, shown under each line when the listener turns them on.
//!
//! In order: none when the lyrics are already in the listener's language; the translation that
//! came with the lyrics (`Lyrics::translation`) when it's in that language; one made earlier
//! (kept in `lyrics/<id>.tr.json`, a cache: not backed up); else the AI, if it's set up. The AI
//! gets the song's title and artist and its distinct lines as a numbered JSON array, and has to
//! give back exactly one translation per number, so a translation can never drift onto another
//! line: an answer with the wrong count is asked for once more, then given up.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The languages translations can be into: code, name for the AI and the window.
pub const LANGS: [(&str, &str); 5] = [("en", "English"), ("id", "Indonesian"), ("ja", "Japanese"), ("ko", "Korean"), ("zh", "Chinese")];

pub fn lang_name(code: &str) -> Option<&'static str> {
    LANGS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// What the window shows under the lines.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Translation {
    pub lang: String,
    /// "lyrics" (came with them), "ai" (made by the AI, now or earlier), "same" (the lyrics are
    /// in that language already: nothing to show), "needs-ai" (the AI isn't set up), "none" (no
    /// lines to translate).
    pub from: String,
    /// One per line of the lyrics as shown ("" where a line has none); empty unless `from` is
    /// "lyrics" or "ai".
    pub lines: Vec<String>,
}

/// Asks a model for one JSON answer (the AI in the app; a fixture in tests).
pub trait Model {
    fn json(&self, prompt: &str) -> Result<Value, String>;
}

/// The song as the translation sees it.
pub struct Song<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    /// The lines as the lyrics view shows them (timed lines, or the plain text's lines).
    pub lines: &'a [String],
    /// The translation that came with the lyrics, and its language.
    pub bundled: &'a [String],
    pub bundled_lang: &'a str,
}

// Common short words of English and Indonesian: the two Latin-letter languages to tell apart.
const EN: &[&str] = &[
    "the", "and", "you", "i", "to", "of", "is", "in", "my", "me", "it", "your", "we", "with", "so", "they", "be", "are", "on", "for", "all", "this", "that",
    "can", "will", "never", "just", "like", "what", "when", "at", "a", "do", "don't", "i'm", "not", "but", "know", "was", "she", "he", "our",
];
const ID: &[&str] = &[
    "yang", "dan", "aku", "kau", "kamu", "di", "ini", "itu", "tak", "tidak", "ke", "dengan", "untuk", "akan", "ku", "mu", "dia", "kita", "kami", "sudah", "bisa",
    "ada", "dalam", "jangan", "saja", "hanya", "karena", "selalu", "pernah", "lagi", "mau", "hari", "pada", "juga", "semua", "telah", "engkau",
];

/// The language of some lines ("ja", "ko", "zh", "en", "id"), None when it can't be told (Latin
/// letters in another language than English or Indonesian, or nothing to go by).
pub fn lang_of(lines: &[String]) -> Option<&'static str> {
    let text = lines.join("\n");
    match crate::lyrics::script_of(&text)? {
        crate::lyrics::Script::Kana => Some("ja"),
        crate::lyrics::Script::Hangul => Some("ko"),
        crate::lyrics::Script::Han => Some("zh"),
        crate::lyrics::Script::Latin => {
            let words: Vec<String> = text.split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
            if words.is_empty() {
                return None;
            }
            let share = |list: &[&str]| words.iter().filter(|w| list.contains(&w.as_str())).count() as f64 / words.len() as f64;
            let (en, id) = (share(EN), share(ID));
            if id >= 0.12 && id > en {
                Some("id")
            } else if en >= 0.15 {
                Some("en")
            } else {
                None
            }
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    langs: HashMap<String, Entry>,
    /// Chunks of a translation not finished yet ("<lang>:<the chunk's version>" → its lines): a
    /// song whose later chunk failed keeps what was already answered (and paid for), and asks
    /// only for the rest next time.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    parts: HashMap<String, Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    /// Which lyrics it's for (`version`).
    hash: String,
    lines: Vec<String>,
}

fn cache_file(dir: &Path, track_id: &str) -> PathBuf {
    dir.join(format!("{track_id}.tr.json"))
}

/// The lyrics' version: their lines (other lyrics, or the same ones edited, are translated anew).
fn version(lines: &[String]) -> String {
    crate::library::short_hash(&lines.join("\n"))
}

/// Translated before, for these very lyrics.
fn cached(dir: &Path, track_id: &str, lang: &str, lines: &[String]) -> Option<Vec<String>> {
    let c: Cache = serde_json::from_slice(&std::fs::read(cache_file(dir, track_id)).ok()?).ok()?;
    let e = c.langs.get(lang)?;
    (e.hash == version(lines) && e.lines.len() == lines.len()).then(|| e.lines.clone())
}

/// One change of a song's translations file at a time (two translations of a song finishing
/// together would otherwise lose one), written through the atomic save.
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn change_cache(dir: &Path, track_id: &str, f: impl FnOnce(&mut Cache)) {
    let _one = FILE_LOCK.lock();
    let path = cache_file(dir, track_id);
    let mut c: Cache = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    f(&mut c);
    if let Ok(b) = serde_json::to_vec(&c) {
        // A cache: no .bak.
        let _ = crate::state::write_file(&path, &b, false);
    }
}

fn part_key(lang: &str, chunk: &[String]) -> String {
    format!("{lang}:{}", version(chunk))
}

/// A chunk answered before (of a translation that didn't finish).
fn cached_part(dir: &Path, track_id: &str, lang: &str, chunk: &[String]) -> Option<Vec<String>> {
    let c: Cache = serde_json::from_slice(&std::fs::read(cache_file(dir, track_id)).ok()?).ok()?;
    c.parts.get(&part_key(lang, chunk)).filter(|l| l.len() == chunk.len()).cloned()
}

fn keep_part(dir: &Path, track_id: &str, lang: &str, chunk: &[String], out: &[String]) {
    change_cache(dir, track_id, |c| {
        c.parts.insert(part_key(lang, chunk), out.to_vec());
    });
}

fn keep(dir: &Path, track_id: &str, lang: &str, lines: &[String], out: &[String]) {
    change_cache(dir, track_id, |c| {
        c.langs.insert(lang.to_string(), Entry { hash: version(lines), lines: out.to_vec() });
        // The whole translation is kept: its chunks are no longer needed.
        let prefix = format!("{lang}:");
        c.parts.retain(|k, _| !k.starts_with(&prefix));
    });
}

/// Lines worth translating: with letters in them (not "♪", "...", empty).
fn has_words(line: &str) -> bool {
    line.chars().any(char::is_alphanumeric)
}

/// Lines per request: a long song's answer stays well under any model's output limit.
const CHUNK: usize = 80;

/// What the AI is asked (no library data: the song's title and artist, and the lines).
pub fn prompt(title: &str, artist: &str, lang: &str, lines: &[String]) -> String {
    let numbered: Vec<Value> = lines.iter().enumerate().map(|(i, l)| Value::from(vec![Value::from(i + 1), Value::from(l.as_str())])).collect();
    format!(
        "Translate the lyrics of the song \"{title}\" by {artist} into {lang}.\n\
INPUT is a JSON array of {n} lines, each [number, line].\n\
Translate the meaning naturally, the way a listener singing along would want to understand each line: \
not word by word. Each line is translated on its own: never merge, split or reorder lines. \
Leave interjections, sounds and names (oh, yeah, la la) as they are. A line already in {lang} stays as it is.\n\
Return JSON only: {{\"lines\":[[1,\"…\"],[2,\"…\"]]}} with exactly {n} entries, one per input number, in order.\n\
INPUT: {input}",
        title = title.replace('"', "'"),
        artist = artist.replace('"', "'"),
        n = lines.len(),
        input = Value::Array(numbered),
    )
}

/// The translations in an answer: exactly `n`, by number ([number, text]) or in order (plain
/// strings). Err(how many there were) otherwise.
pub fn read_answer(v: &Value, n: usize) -> Result<Vec<String>, usize> {
    let list = v["lines"].as_array().or_else(|| v.as_array()).ok_or(0usize)?;
    let mut by_number: Vec<Option<String>> = vec![None; n];
    let mut positional = Vec::new();
    for item in list {
        match item {
            Value::String(s) => positional.push(s.trim().to_string()),
            Value::Array(a) => {
                let (Some(k), Some(t)) = (a.first().and_then(Value::as_u64), a.get(1).and_then(Value::as_str)) else { return Err(list.len()) };
                let slot = (k as usize).checked_sub(1).and_then(|i| by_number.get_mut(i)).ok_or(list.len())?;
                if slot.replace(t.trim().to_string()).is_some() {
                    return Err(list.len());
                }
            }
            _ => return Err(list.len()),
        }
    }
    if !positional.is_empty() {
        return if positional.len() == n && by_number.iter().all(Option::is_none) { Ok(positional) } else { Err(list.len()) };
    }
    if list.len() != n {
        return Err(list.len());
    }
    by_number.into_iter().collect::<Option<Vec<_>>>().ok_or(list.len())
}

/// One chunk of distinct lines: asked once, and once more when the answer doesn't line up.
fn ask(model: &dyn Model, song: &Song, lang: &str, lines: &[String], cancelled: &dyn Fn() -> bool) -> Result<Vec<String>, String> {
    let base = prompt(song.title, song.artist, lang, lines);
    let mut p = base.clone();
    for attempt in 0..2 {
        let v = model.json(&p)?;
        match read_answer(&v, lines.len()) {
            Ok(out) => return Ok(out),
            Err(got) => {
                log::info!("translation answer had {got} lines for {}; attempt {}", lines.len(), attempt + 1);
                if cancelled() {
                    return Err("cancelled".into());
                }
                p = format!("{base}\nYour previous answer had {got} entries. It must have exactly {} entries, numbered 1 to {}.", lines.len(), lines.len());
            }
        }
    }
    Err("The AI’s translation didn’t line up with the lyrics. Try again later.".into())
}

/// The translation of a song's lyrics into `lang` (see the module's notes). `model`: None when
/// the AI isn't set up. `cancelled`: the song changed; nothing more is asked.
pub fn translate(cache_dir: &Path, track_id: &str, song: &Song, lang: &str, model: Option<&dyn Model>, cancelled: &dyn Fn() -> bool) -> Result<Translation, String> {
    let name = lang_name(lang).ok_or_else(|| format!("Cadence doesn’t translate into “{lang}”."))?;
    let done = |from: &str, lines: Vec<String>| Ok(Translation { lang: lang.to_string(), from: from.to_string(), lines });
    if !song.lines.iter().any(|l| has_words(l)) {
        return done("none", Vec::new());
    }
    if lang_of(song.lines) == Some(lang) {
        return done("same", Vec::new());
    }
    if song.bundled_lang == lang && song.bundled.len() == song.lines.len() && song.bundled.iter().any(|l| !l.is_empty()) {
        return done("lyrics", song.bundled.to_vec());
    }
    if let Some(lines) = cached(cache_dir, track_id, lang, song.lines) {
        return done("ai", lines);
    }
    let Some(model) = model else { return done("needs-ai", Vec::new()) };
    // Each distinct line once (a chorus comes back many times), mapped back afterwards.
    let mut distinct: Vec<String> = Vec::new();
    let mut at: HashMap<&str, usize> = HashMap::new();
    for l in song.lines {
        let t = l.trim();
        if has_words(t) && !at.contains_key(t) {
            at.insert(t, distinct.len());
            distinct.push(t.to_string());
        }
    }
    let mut out_distinct = Vec::with_capacity(distinct.len());
    for chunk in distinct.chunks(CHUNK) {
        if let Some(done) = cached_part(cache_dir, track_id, lang, chunk) {
            out_distinct.extend(done);
            continue;
        }
        if cancelled() {
            return Err("cancelled".into());
        }
        let answered = ask(model, song, name, chunk, cancelled)?;
        // Kept at once: a later chunk failing doesn't throw this one away.
        keep_part(cache_dir, track_id, lang, chunk, &answered);
        out_distinct.extend(answered);
    }
    let lines: Vec<String> = song.lines.iter().map(|l| at.get(l.trim()).map(|&i| out_distinct[i].clone()).unwrap_or_default()).collect();
    keep(cache_dir, track_id, lang, song.lines, &lines);
    done("ai", lines)
}

// ---------- in the app ----------

/// The song whose translation is wanted now; another song (or none) cancels a running one.
static WANTED: Mutex<Option<String>> = Mutex::new(None);

pub fn begin(track_id: &str) {
    *WANTED.lock() = Some(track_id.to_string());
}

pub fn cancel(track_id: &str) {
    let mut w = WANTED.lock();
    if w.as_deref() == Some(track_id) {
        *w = None;
    }
}

pub fn is_wanted(track_id: &str) -> bool {
    WANTED.lock().as_deref() == Some(track_id)
}

/// The app's AI, as a `Model`.
pub struct Ai(pub crate::ai::AiSettings);

impl Model for Ai {
    fn json(&self, prompt: &str) -> Result<Value, String> {
        crate::ai::translate_json(&self.0, prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    /// Answers from a list, one per request, and keeps the prompts.
    struct Fixture {
        answers: RefCell<Vec<Value>>,
        prompts: RefCell<Vec<String>>,
    }
    impl Fixture {
        fn new(answers: Vec<Value>) -> Self {
            Self { answers: RefCell::new(answers), prompts: RefCell::new(Vec::new()) }
        }
        fn asked(&self) -> usize {
            self.prompts.borrow().len()
        }
    }
    impl Model for Fixture {
        fn json(&self, prompt: &str) -> Result<Value, String> {
            self.prompts.borrow_mut().push(prompt.to_string());
            let mut a = self.answers.borrow_mut();
            if a.is_empty() { Err("no more answers".into()) } else { Ok(a.remove(0)) }
        }
    }

    // Invented lines (no real lyrics in fixtures).
    fn japanese() -> Vec<String> {
        ["青い空を見上げた", "今日は雨が降る", "", "青い空を見上げた", "♪"].iter().map(|s| s.to_string()).collect()
    }
    fn song(lines: &[String]) -> Song<'_> {
        Song { title: "Test Song", artist: "Example Artist", lines, bundled: &[], bundled_lang: "" }
    }
    fn never() -> bool {
        false
    }

    #[test]
    fn one_translation_per_line_mapped_back_onto_repeats() {
        let dir = tempfile::tempdir().unwrap();
        let lines = japanese();
        // Two distinct lines are asked for; the repeat and the empty ones are filled in here.
        let m = Fixture::new(vec![json!({ "lines": [[1, "I looked up at the blue sky"], [2, "it is raining today"]] })]);
        let t = translate(dir.path(), "s1", &song(&lines), "en", Some(&m), &never).unwrap();
        assert_eq!(t.from, "ai");
        assert_eq!(t.lines, ["I looked up at the blue sky", "it is raining today", "", "I looked up at the blue sky", ""]);
        let p = &m.prompts.borrow()[0];
        assert!(p.contains("\"Test Song\" by Example Artist into English"), "{p}");
        assert!(p.contains(r#"[[1,"青い空を見上げた"],[2,"今日は雨が降る"]]"#), "{p}");
        assert!(p.contains("exactly 2 entries"));
    }

    #[test]
    fn a_wrong_count_is_asked_again_once_then_given_up() {
        let dir = tempfile::tempdir().unwrap();
        let lines = japanese();
        // One line short, then right: the second answer is used.
        let m = Fixture::new(vec![json!({ "lines": [[1, "only one"]] }), json!({ "lines": ["I looked up at the blue sky", "it is raining today"] })]);
        let t = translate(dir.path(), "s1", &song(&lines), "en", Some(&m), &never).unwrap();
        assert_eq!(m.asked(), 2);
        assert!(m.prompts.borrow()[1].contains("previous answer had 1 entries"));
        assert_eq!(t.lines[1], "it is raining today");
        // Wrong twice: an error, nothing kept, no third request.
        let dir = tempfile::tempdir().unwrap();
        let m = Fixture::new(vec![json!({ "lines": ["a", "b", "c"] }), json!({ "lines": [[1, "a"], [1, "b"]] }), json!({ "lines": ["x", "y"] })]);
        assert!(translate(dir.path(), "s1", &song(&lines), "en", Some(&m), &never).is_err());
        assert_eq!(m.asked(), 2);
        assert!(!cache_file(dir.path(), "s1").exists());
        // Cancelled after a bad answer (the song changed): no second request.
        let m = Fixture::new(vec![json!({ "lines": [] }), json!({ "lines": ["x", "y"] })]);
        assert!(translate(dir.path(), "s1", &song(&lines), "en", Some(&m), &|| m_cancel()).is_err());
        assert_eq!(m.asked(), 1);
    }
    fn m_cancel() -> bool {
        // Cancelled once the first answer is in (the first check, before any request, passes).
        thread_local!(static N: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });
        N.with(|n| {
            n.set(n.get() + 1);
            n.get() > 1
        })
    }

    #[test]
    fn kept_translations_are_not_asked_for_again() {
        let dir = tempfile::tempdir().unwrap();
        let lines = japanese();
        let m = Fixture::new(vec![json!({ "lines": ["I looked up at the blue sky", "it is raining today"] })]);
        translate(dir.path(), "s1", &song(&lines), "en", Some(&m), &never).unwrap();
        // Again, even without the AI: from the cache, no request.
        let again = translate(dir.path(), "s1", &song(&lines), "en", None, &never).unwrap();
        assert_eq!((again.from.as_str(), again.lines[1].as_str()), ("ai", "it is raining today"));
        let m2 = Fixture::new(vec![]);
        translate(dir.path(), "s1", &song(&lines), "en", Some(&m2), &never).unwrap();
        assert_eq!(m2.asked(), 0);
        // Other lyrics for the song: translated anew.
        let other: Vec<String> = vec!["窓を開けた".into()];
        let m3 = Fixture::new(vec![json!({ "lines": ["I opened the window"] })]);
        assert_eq!(translate(dir.path(), "s1", &song(&other), "en", Some(&m3), &never).unwrap().lines, ["I opened the window"]);
        assert_eq!(m3.asked(), 1);
        // Another language for the same lyrics: its own entry, the first one kept.
        let m4 = Fixture::new(vec![json!({ "lines": ["Aku membuka jendela"] })]);
        assert_eq!(translate(dir.path(), "s1", &song(&other), "id", Some(&m4), &never).unwrap().lines, ["Aku membuka jendela"]);
        assert_eq!(translate(dir.path(), "s1", &song(&other), "en", None, &never).unwrap().lines, ["I opened the window"]);
    }

    #[test]
    fn chunks_already_answered_are_kept_when_a_later_one_fails() {
        let dir = tempfile::tempdir().unwrap();
        // More distinct lines than one request takes: two chunks.
        let lines: Vec<String> = (0..CHUNK + 5).map(|i| format!("架空の行 {i}")).collect();
        let first: Vec<String> = (0..CHUNK).map(|i| format!("made-up line {i}")).collect();
        let second: Vec<String> = (CHUNK..CHUNK + 5).map(|i| format!("made-up line {i}")).collect();
        // The first chunk is answered; the second fails twice.
        let m = Fixture::new(vec![json!({ "lines": first.clone() }), json!({ "lines": ["x"] }), json!({ "lines": ["y"] })]);
        assert!(translate(dir.path(), "s1", &song(&lines), "en", Some(&m), &never).is_err());
        assert_eq!(m.asked(), 3);
        // Next time only the second chunk is asked for.
        let m2 = Fixture::new(vec![json!({ "lines": second.clone() })]);
        let t = translate(dir.path(), "s1", &song(&lines), "en", Some(&m2), &never).unwrap();
        assert_eq!(m2.asked(), 1);
        assert!(m2.prompts.borrow()[0].contains("exactly 5 entries"));
        assert_eq!(t.lines[0], "made-up line 0");
        assert_eq!(t.lines[CHUNK + 4], format!("made-up line {}", CHUNK + 4));
        // Finished: kept whole, the chunks let go of.
        let c: Cache = serde_json::from_slice(&std::fs::read(cache_file(dir.path(), "s1")).unwrap()).unwrap();
        assert!(c.parts.is_empty() && c.langs.contains_key("en"));
        // Two translations of one song kept at the same time: neither is lost.
        let one: Vec<String> = vec!["窓を開けた".into()];
        std::thread::scope(|s| {
            for lang in ["id", "ko"] {
                let dir = dir.path();
                let one = &one;
                s.spawn(move || keep(dir, "s1", lang, one, &[format!("{lang} line")]));
            }
        });
        let c: Cache = serde_json::from_slice(&std::fs::read(cache_file(dir.path(), "s1")).unwrap()).unwrap();
        assert!(["en", "id", "ko"].iter().all(|l| c.langs.contains_key(*l)), "{:?}", c.langs.keys().collect::<Vec<_>>());
    }

    #[test]
    fn nothing_is_asked_when_it_isnt_needed() {
        let dir = tempfile::tempdir().unwrap();
        let m = Fixture::new(vec![]);
        // Already in the listener's language.
        for (lines, lang) in [
            (vec!["the rain is falling today", "I opened the window and the sky is blue"], "en"),
            (vec!["Hari ini hujan turun", "aku membuka jendela dan langit biru"], "id"),
            (vec!["青い空を見上げた", "今日は雨が降る"], "ja"),
            (vec!["오늘은 비가 온다"], "ko"),
            (vec!["今天下雨了"], "zh"),
        ] {
            let lines: Vec<String> = lines.into_iter().map(String::from).collect();
            let t = translate(dir.path(), "s", &song(&lines), lang, Some(&m), &never).unwrap();
            assert_eq!(t.from, "same", "{lang}");
            assert!(t.lines.is_empty());
        }
        // A translation that came with the lyrics, in that language.
        let lines = japanese();
        let bundled: Vec<String> = ["I looked up at the blue sky", "it is raining today", "", "I looked up at the blue sky", ""].iter().map(|s| s.to_string()).collect();
        let s = Song { bundled: &bundled, bundled_lang: "en", ..song(&lines) };
        assert_eq!(translate(dir.path(), "s", &s, "en", Some(&m), &never).unwrap().from, "lyrics");
        // AI not set up: said so, nothing asked.
        assert_eq!(translate(dir.path(), "s", &s, "id", None, &never).unwrap().from, "needs-ai");
        // Nothing to translate.
        let empty: Vec<String> = vec!["♪".into(), String::new()];
        assert_eq!(translate(dir.path(), "s", &song(&empty), "en", Some(&m), &never).unwrap().from, "none");
        assert_eq!(m.asked(), 0);
        assert!(translate(dir.path(), "s", &song(&lines), "fr", Some(&m), &never).is_err());
    }

    #[test]
    fn answers_are_read_strictly() {
        let ab = || Ok(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(read_answer(&json!({ "lines": [[2, "b"], [1, "a"]] }), 2), ab());
        assert_eq!(read_answer(&json!(["a", "b"]), 2), ab());
        assert_eq!(read_answer(&json!({ "lines": [[1, "a"], [3, "c"]] }), 2), Err(2));
        assert_eq!(read_answer(&json!({ "lines": [[1, "a"], "b"] }), 2), Err(2));
        assert_eq!(read_answer(&json!({ "text": "a" }), 1), Err(0));
    }

    #[test]
    fn languages() {
        let l = |s: &[&str]| lang_of(&s.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(l(&["the rain is falling today"]), Some("en"));
        assert_eq!(l(&["Hari ini hujan turun dan aku di rumah"]), Some("id"));
        assert_eq!(l(&["青い空を見上げた"]), Some("ja"));
        assert_eq!(l(&["今天下雨了"]), Some("zh"));
        assert_eq!(l(&["오늘은 비가 온다"]), Some("ko"));
        assert_eq!(l(&["Hoy llueve mucho"]), None);
        assert_eq!(l(&["♪"]), None);
    }
}
