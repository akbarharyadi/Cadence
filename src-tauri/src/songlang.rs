//! Which language a song is sung in, worked out on this computer for the DJ's retriever and Your
//! Stats. Limited to the languages this library is in: Japanese, Korean, Chinese, Indonesian,
//! English and Malay; anything else (or too little to go on) is "unknown", never a wrong guess.
//!
//! What counts, strongest first:
//! 1. a language tag in the title ("[JP]", "(KR)");
//! 2. the song's saved lyrics: their script (kana → Japanese, Hangul → Korean, Han only →
//!    Chinese), and for Latin letters a small word-list detector (`latin`) that tells Indonesian,
//!    Malay and English apart and calls romanized Japanese or Korean "unknown";
//! 3. the title, album and artist name's script (Han only is Japanese when the artist is known to
//!    be Japanese or the album has kana somewhere, else Chinese, less sure);
//! 4. the artist's country (MusicBrainz, through `identity`);
//! 5. then songs still unknown take their album's majority, then their artist's.
//!
//! The lyrics' languages are kept in song-languages.json (a cache, not backed up) by the lyrics
//! file's size and time, so a library change reads only lyrics that changed.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::romanize::{is_han, is_hangul, is_kana};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ja,
    Ko,
    Zh,
    Id,
    En,
    Ms,
    #[default]
    #[serde(other)]
    Unknown,
}

impl Lang {
    pub const KNOWN: [Lang; 6] = [Lang::Ja, Lang::Ko, Lang::Zh, Lang::Id, Lang::En, Lang::Ms];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::Ko => "ko",
            Lang::Zh => "zh",
            Lang::Id => "id",
            Lang::En => "en",
            Lang::Ms => "ms",
            Lang::Unknown => "unknown",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Ja => "Japanese",
            Lang::Ko => "Korean",
            Lang::Zh => "Chinese",
            Lang::Id => "Indonesian",
            Lang::En => "English",
            Lang::Ms => "Malay",
            Lang::Unknown => "Unknown",
        }
    }

    /// A code or a name ("id", "Indonesian", "bahasa"); None for anything else.
    pub fn parse(s: &str) -> Option<Lang> {
        Some(match s.trim().to_lowercase().as_str() {
            "ja" | "jp" | "jpn" | "japanese" => Lang::Ja,
            "ko" | "kr" | "kor" | "korean" => Lang::Ko,
            "zh" | "cn" | "chi" | "chinese" | "mandarin" | "cantonese" => Lang::Zh,
            "id" | "ind" | "indonesian" | "bahasa" | "bahasa indonesia" => Lang::Id,
            "en" | "eng" | "english" => Lang::En,
            "ms" | "my" | "malay" | "bahasa melayu" | "melayu" => Lang::Ms,
            _ => return None,
        })
    }
}

/// A language with how sure it is (0–1).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Guess {
    pub lang: Lang,
    pub conf: f32,
}

impl Guess {
    pub const UNKNOWN: Guess = Guess { lang: Lang::Unknown, conf: 0.0 };
    fn new(lang: Lang, conf: f32) -> Guess {
        Guess { lang, conf: conf.clamp(0.0, 1.0) }
    }
}

// ---------- Latin letters: Indonesian, Malay, English, or romanized Japanese / Korean ----------

const EN: &[&str] = &[
    "the", "and", "you", "i", "to", "of", "is", "in", "my", "me", "it", "your", "we", "with", "so", "they", "be", "are", "on", "for", "all", "this", "that",
    "can", "will", "never", "just", "like", "what", "when", "at", "a", "do", "don't", "i'm", "not", "but", "know", "was", "she", "he", "our", "love", "feel",
    "want", "heart", "night", "tonight", "been", "there", "from", "how", "if", "now", "say", "said", "gonna", "wanna", "can't", "won't", "i'll", "you're",
    "it's", "let", "come", "time", "way", "through", "still", "every", "one", "were", "her", "his", "who", "why", "where", "have", "has", "had", "would",
    "could", "nothing", "everything", "away", "again", "into", "up", "out", "down", "only", "same", "since", "before", "after", "an", "am", "us", "them",
];
/// Words Indonesian and Malay share.
const MALAYIC: &[&str] = &[
    "yang", "dan", "aku", "kau", "di", "ini", "itu", "tak", "tidak", "ke", "dengan", "untuk", "akan", "dia", "kita", "kami", "sudah", "ada", "dalam", "pada",
    "dari", "cinta", "hati", "hatiku", "rindu", "sayang", "mimpi", "malam", "hujan", "jalan", "kasih", "selalu", "lagi", "masih", "semua", "hanya", "jangan",
    "pergi", "mana", "apa", "siapa", "bila", "jika", "kalau", "tapi", "tetapi", "juga", "seperti", "bersama", "sampai", "telah", "ingin", "mungkin", "setiap",
    "tanpa", "harus", "biar", "pernah", "begitu", "saat", "waktu", "hari", "kini", "nanti", "dulu", "oleh", "kepada", "padamu", "untukmu", "diriku", "dirimu",
    "engkau", "kekasih", "pagi", "sepi", "sendiri", "berdua", "bawah", "atas", "pulang", "rasa", "mata", "air", "angin", "ingat", "lupa", "tahu", "betapa",
    "suara", "dengar", "datang", "tiba", "hilang", "kenangan", "pun", "lah", "nya", "namaku", "kotaku", "selamanya", "kenapa", "mengapa", "sungguh",
];
const ID_ONLY: &[&str] = &[
    "karena", "mau", "nggak", "gak", "enggak", "banget", "aja", "sih", "dong", "kok", "kayak", "bisa", "cuma", "udah", "kamu", "temani", "sama", "saja",
    "lho", "deh", "bikin", "gimana", "tau", "padahal", "apakah", "besok", "kukatakan", "menunggumu", "pikiranku", "untukmu", "perlahan", "lelah", "bertahan",
];
const MS_ONLY: &[&str] = &[
    "awak", "sahaja", "kerana", "mahu", "betul", "cakap", "nak", "dah", "boleh", "macam", "hendak", "kereta", "duit", "percuma", "bilik", "faham", "penat",
    "kampung", "rindunya", "beta", "tengok", "jom",
];
const JA_ROMAJI: &[&str] = &[
    "wa", "wo", "ga", "ni", "kimi", "boku", "watashi", "ore", "anata", "desu", "masu", "dake", "kara", "kokoro", "yume", "sora", "yoru", "ashita", "kitto",
    "zutto", "mada", "mou", "demo", "nai", "naku", "nante", "sekai", "koe", "hikari", "namida", "itsumo", "ima", "shite", "miteiru", "aruku", "michi",
    "onaji", "naka", "sakebu", "kikoeru", "sayonara", "arigatou", "daisuki", "suki", "hitori", "futari", "mirai", "kono", "sono", "ano", "tte", "kedo",
];
const KO_ROMAJI: &[&str] = &[
    "neo", "neoui", "naega", "nae", "neol", "nal", "uri", "saranghae", "saranghaneun", "sarang", "isseulkka", "eopseo", "haneun", "oneul", "bam", "bamedo",
    "dasi", "mannal", "geurae", "jigeum", "maeil", "hamkke", "nareul", "neoreul", "moksori", "moksoriga", "deullyeo", "mankeum", "gateun", "sipeo", "naneun",
    "neoneun", "geudae", "nan", "ije", "cheoeum",
];

/// The language of some Latin-letter text (lyrics, a title): Indonesian, Malay or English, with
/// how sure; romanized Japanese or Korean, or too few telling words, is unknown.
pub fn latin(text: &str) -> Guess {
    let words: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphabetic() || c == '\''))
        .collect();
    let n = words.len();
    if n < 4 {
        return Guess::UNKNOWN;
    }
    let count = |list: &[&str]| words.iter().filter(|w| list.contains(&w.as_str())).count() as f32;
    let en = count(EN);
    let (shared, id, ms) = (count(MALAYIC), count(ID_ONLY), count(MS_ONLY));
    let ja = count(JA_ROMAJI);
    // Korean romanization's vowels (eo, eu) hardly appear in the others' words.
    let ko = count(KO_ROMAJI) + 0.5 * words.iter().filter(|w| (w.contains("eo") || w.contains("eu")) && !EN.contains(&w.as_str())).count() as f32;
    let malayic = shared + id + ms;
    let all = en + malayic + ja + ko;
    let (best, which) = [(en, 0), (malayic, 1), (ja, 2), (ko, 3)].into_iter().fold((0.0f32, 0), |a, b| if b.0 > a.0 { b } else { a });
    // A few telling words, and enough of the text: otherwise there's nothing to go on.
    if best < 2.0 || best / (n as f32) < 0.12 {
        return Guess::UNKNOWN;
    }
    let margin = best / all.max(1.0);
    let conf = margin * (0.55 + (best / 12.0).min(0.4));
    match which {
        0 => Guess::new(Lang::En, conf),
        1 if ms > id => Guess::new(Lang::Ms, conf * if id == 0.0 { 1.0 } else { 0.8 }),
        // Without a word only Indonesian uses, it's still most likely Indonesian here, less sure.
        1 => Guess::new(Lang::Id, conf * if id > 0.0 { 1.0 } else { 0.85 }),
        _ => Guess { lang: Lang::Unknown, conf: 0.0 },
    }
}

// ---------- a song ----------

/// What the artist is known to be (from `identity`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArtistHint {
    /// The artist's language as `identity` has it (MusicBrainz country, kana or Hangul names).
    pub lang: Option<Lang>,
    /// From the artist's country alone (Indonesia, Malaysia, an English-speaking country).
    pub country: Option<Lang>,
}

/// The language a MusicBrainz country suggests.
pub fn lang_of_country(c: &str) -> Option<Lang> {
    Some(match c {
        "JP" => Lang::Ja,
        "KR" => Lang::Ko,
        "CN" | "TW" | "HK" | "MO" => Lang::Zh,
        "ID" => Lang::Id,
        "MY" | "BN" => Lang::Ms,
        "US" | "GB" | "CA" | "AU" | "IE" | "NZ" => Lang::En,
        _ => return None,
    })
}

/// One song as the tagger sees it.
pub struct Song<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub album_id: &'a str,
    /// Who leads the credit (`identity`'s key), for the artist's majority.
    pub artist_key: &'a str,
    pub hint: ArtistHint,
    /// What the song's saved lyrics say, if it has any (`of_lyrics`).
    pub lyrics: Option<Guess>,
}

#[derive(Default)]
struct Scripts {
    han: usize,
    kana: usize,
    hangul: usize,
    latin: usize,
}

fn scripts(s: &str) -> Scripts {
    let mut out = Scripts::default();
    for c in s.chars() {
        if is_kana(c) {
            out.kana += 1;
        } else if is_hangul(c) {
            out.hangul += 1;
        } else if is_han(c) {
            out.han += 1;
        } else if c.is_ascii_alphabetic() {
            out.latin += 1;
        }
    }
    out
}

/// The language of a song's lyrics text: by script, and for Latin letters by `latin`.
/// `japanese_artist`: Han-only lyrics of a Japanese artist are still Japanese (short kanji lines).
pub fn of_lyrics(text: &str, japanese_artist: bool) -> Guess {
    match crate::lyrics::script_of(text) {
        Some(crate::lyrics::Script::Kana) => Guess::new(Lang::Ja, 0.95),
        Some(crate::lyrics::Script::Hangul) => Guess::new(Lang::Ko, 0.95),
        Some(crate::lyrics::Script::Han) if japanese_artist => Guess::new(Lang::Ja, 0.75),
        Some(crate::lyrics::Script::Han) => Guess::new(Lang::Zh, 0.85),
        Some(crate::lyrics::Script::Latin) => latin(text),
        None => Guess::UNKNOWN,
    }
}

/// The language of each song (see the module), the album's and artist's majority filling gaps.
pub fn tag(songs: &[Song]) -> Vec<Guess> {
    // Albums with kana anywhere: their Han-only titles are Japanese.
    let mut album_kana: HashMap<&str, bool> = HashMap::new();
    for s in songs {
        let k = s.title.chars().chain(s.album.chars()).any(is_kana);
        *album_kana.entry(s.album_id).or_default() |= k;
    }
    let mut out: Vec<Guess> = songs
        .iter()
        .map(|s| {
            let mut best = Guess::UNKNOWN;
            let mut consider = |g: Guess| {
                if g.lang != Lang::Unknown && g.conf > best.conf {
                    best = g;
                }
            };
            // 1. A language tag in the title.
            match crate::lyrics::wanted_script(s.title) {
                Some(crate::lyrics::Script::Kana) => consider(Guess::new(Lang::Ja, 0.9)),
                Some(crate::lyrics::Script::Hangul) => consider(Guess::new(Lang::Ko, 0.9)),
                Some(crate::lyrics::Script::Han) => consider(Guess::new(Lang::Zh, 0.9)),
                Some(crate::lyrics::Script::Latin) => consider(Guess::new(Lang::En, 0.7)),
                None => {}
            }
            // 2. The lyrics.
            if let Some(g) = s.lyrics {
                consider(g);
            }
            // 3. The title, album and artist's script.
            let japanese = s.hint.lang == Some(Lang::Ja) || album_kana.get(s.album_id).copied().unwrap_or(false);
            let words = scripts(&format!("{} {}", s.title, s.album));
            let who = scripts(s.artist);
            if words.kana > 0 {
                consider(Guess::new(Lang::Ja, 0.85));
            } else if words.hangul > 0 {
                consider(Guess::new(Lang::Ko, 0.85));
            } else if words.han > 0 {
                match s.hint.lang {
                    _ if japanese => consider(Guess::new(Lang::Ja, 0.75)),
                    Some(Lang::Zh) => consider(Guess::new(Lang::Zh, 0.8)),
                    Some(Lang::Ko) => consider(Guess::new(Lang::Ko, 0.5)),
                    _ => consider(Guess::new(Lang::Zh, 0.6)),
                }
            } else if words.latin > 0 {
                // A Latin title says little on its own: a weaker version of the lyrics' detector.
                let g = latin(&format!("{} {}", s.title, s.album));
                consider(Guess::new(g.lang, g.conf * 0.6));
            }
            if who.kana > 0 {
                consider(Guess::new(Lang::Ja, 0.6));
            } else if who.hangul > 0 {
                consider(Guess::new(Lang::Ko, 0.6));
            }
            // 4. What the artist is known to be.
            if let Some(l) = s.hint.lang {
                consider(Guess::new(l, 0.6));
            }
            if let Some(l) = s.hint.country {
                consider(Guess::new(l, 0.55));
            }
            best
        })
        .collect();
    // 5. The album's majority, then the artist's, for songs still unsure.
    for by in [0, 1] {
        let key = |s: &Song| if by == 0 { s.album_id.to_string() } else { s.artist_key.to_string() };
        let mut votes: HashMap<String, HashMap<Lang, u32>> = HashMap::new();
        for (s, g) in songs.iter().zip(&out) {
            if g.lang != Lang::Unknown && g.conf >= 0.5 && !key(s).is_empty() {
                *votes.entry(key(s)).or_default().entry(g.lang).or_default() += 1;
            }
        }
        for (s, g) in songs.iter().zip(out.iter_mut()) {
            if g.lang != Lang::Unknown && g.conf >= 0.5 {
                continue;
            }
            let Some(v) = votes.get(&key(s)) else { continue };
            let total: u32 = v.values().sum();
            let Some((&lang, &n)) = v.iter().max_by_key(|(l, n)| (**n, std::cmp::Reverse(**l))) else { continue };
            let share = n as f32 / total as f32;
            if n >= 2 && share >= 0.6 {
                let conf = if by == 0 { 0.6 } else { 0.5 } * share;
                if conf > g.conf || g.lang == Lang::Unknown {
                    *g = Guess::new(lang, conf);
                }
            }
        }
    }
    out
}

// ---------- the lyrics' languages, kept ----------

#[derive(Default, Serialize, Deserialize)]
pub struct LyricCache {
    /// Track id → (the lyrics file's "size:modified", its language, sureness in hundredths).
    pub songs: HashMap<String, (String, Lang, u8)>,
}

/// Each listed song's lyrics language: from the cache when its lyrics file hasn't changed, else
/// read (`saved` lyrics only: no network). `japanese`: songs by Japanese artists. Saves the cache
/// when something changed.
pub fn lyric_languages(data_dir: &Path, songs: &[(&str, &str, bool)]) -> HashMap<String, Guess> {
    let dir = data_dir.join("lyrics");
    let file = data_dir.join("song-languages.json");
    let mut stamps: HashMap<String, String> = HashMap::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".json") else { continue };
            // (On Windows the size and time come with the folder listing: no extra file access.)
            let Ok(m) = e.metadata() else { continue };
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            stamps.insert(id.to_string(), format!("{}:{t}", m.len()));
        }
    }
    let mut cache: LyricCache = crate::state::read_json(&file);
    let mut changed = false;
    let mut out = HashMap::new();
    for &(id, title, japanese) in songs {
        let Some(stamp) = stamps.get(id) else { continue };
        if let Some((s, lang, conf)) = cache.songs.get(id)
            && s == stamp
        {
            out.insert(id.to_string(), Guess { lang: *lang, conf: *conf as f32 / 100.0 });
            continue;
        }
        let g = crate::lyrics::saved(&dir, id, title)
            .map(|l| {
                let text = if l.plain.trim().is_empty() { l.synced.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join("\n") } else { l.plain.clone() };
                of_lyrics(&text, japanese)
            })
            .unwrap_or(Guess::UNKNOWN);
        cache.songs.insert(id.to_string(), (stamp.clone(), g.lang, (g.conf * 100.0).round() as u8));
        changed = true;
        out.insert(id.to_string(), g);
    }
    // Songs whose lyrics went away.
    let before = cache.songs.len();
    cache.songs.retain(|id, _| stamps.contains_key(id));
    if changed || cache.songs.len() != before {
        crate::state::write_json_with(&file, &cache, false);
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    // Invented lines (not from any song), a few per language.
    pub const INDONESIAN: &[&str] = &[
        "aku tak bisa tidur malam ini karena kamu selalu ada di pikiranku",
        "hujan turun lagi di jalan yang sepi dan aku masih menunggumu",
        "jangan pergi dulu, masih banyak yang ingin kukatakan padamu",
        "kita berjalan berdua di bawah lampu kota yang mulai redup",
        "hatiku sudah lelah tapi aku tetap bertahan untukmu",
        "kalau besok tiba, apakah kau masih ingat namaku",
        "aku gak mau sendiri lagi, temani aku sampai pagi",
        "semua kenangan itu perlahan hilang bersama angin",
    ];
    pub const ENGLISH: &[&str] = &[
        "i keep driving through the city lights and thinking of you",
        "the rain is falling on the empty street tonight",
        "don't let me go before the morning comes",
        "we were dancing in the kitchen with the radio on",
        "you said forever but forever never came",
        "all the stars are fading and i'm still awake",
        "take my hand and run with me across the field",
        "nothing feels the same since you walked away",
    ];
    pub const MALAY: &[&str] = &[
        "awak tahu tak betapa rindunya aku kepada awak",
        "aku mahu pulang ke kampung kerana hati ini sudah penat",
        "jangan cakap begitu, aku sayang awak sahaja",
        "bila malam tiba aku nak dengar suara awak",
    ];
    pub const ROMAJI: &[&str] = &[
        "kimi no koe ga kikoeru yoru no sora ni",
        "boku wa mada yume wo miteiru dake sa",
        "ashita mo kitto onaji michi wo aruku",
        "kokoro no naka de zutto sakebu",
    ];
    pub const KOREAN_ROMANIZED: &[&str] = &["neoui moksoriga deullyeo oneul bamedo", "naega neol saranghaneun mankeum", "uri dasi mannal su isseulkka"];
    pub const TOO_LITTLE: &[&str] = &["la la la", "oh", "yeah yeah yeah", "", "Midnight"];

    /// Each sample's detected language, and the share right.
    pub fn accuracy() -> Vec<(&'static str, usize, usize)> {
        let sets: [(&str, &[&str], Lang); 6] = [
            ("Indonesian", INDONESIAN, Lang::Id),
            ("English", ENGLISH, Lang::En),
            ("Malay", MALAY, Lang::Ms),
            ("romanized Japanese → unknown", ROMAJI, Lang::Unknown),
            ("romanized Korean → unknown", KOREAN_ROMANIZED, Lang::Unknown),
            ("too little → unknown", TOO_LITTLE, Lang::Unknown),
        ];
        sets.iter().map(|(name, lines, want)| (*name, lines.iter().filter(|l| latin(l).lang == *want).count(), lines.len())).collect()
    }

    #[test]
    fn latin_lines_on_invented_samples() {
        let mut wrong_language = 0;
        for (name, right, total) in accuracy() {
            println!("{name}: {right}/{total}");
            assert!(right * 4 >= total * 3, "{name}: {right}/{total}");
        }
        // Never a wrong language for English or Indonesian (unknown is allowed, a swap is not).
        for l in INDONESIAN {
            if latin(l).lang == Lang::En {
                wrong_language += 1;
            }
        }
        for l in ENGLISH {
            if matches!(latin(l).lang, Lang::Id | Lang::Ms) {
                wrong_language += 1;
            }
        }
        assert_eq!(wrong_language, 0);
        // Whole lyrics (all lines together) are surer than one line.
        let all = latin(&INDONESIAN.join("\n"));
        assert_eq!(all.lang, Lang::Id);
        assert!(all.conf >= latin(INDONESIAN[0]).conf && all.conf >= 0.6, "{all:?}");
        assert_eq!(latin(&ENGLISH.join("\n")).lang, Lang::En);
        assert_eq!(latin(&ROMAJI.join("\n")).lang, Lang::Unknown);
    }

    fn song<'a>(title: &'a str, artist: &'a str, album_id: &'a str, hint: ArtistHint, lyrics: Option<Guess>) -> Song<'a> {
        Song { title, artist, album: "", album_id, artist_key: artist, hint, lyrics }
    }

    #[test]
    fn songs_by_script_lyrics_and_album() {
        let none = ArtistHint::default();
        let ja_artist = ArtistHint { lang: Some(Lang::Ja), country: None };
        let zh_artist = ArtistHint { lang: Some(Lang::Zh), country: None };
        let id_lyrics = Some(latin(&INDONESIAN.join("\n")));
        let en_lyrics = Some(latin(&ENGLISH.join("\n")));
        let songs = vec![
            // 0: kana title → Japanese.
            song("よるのうた", "Hoshino Band", "a1", none, None),
            // 1: Hangul → Korean.
            song("밤하늘", "Some Group", "a2", none, None),
            // 2: Han only, known-Japanese artist → Japanese.
            song("夜空", "架空楽団", "a3", ja_artist, None),
            // 3: Han only, Chinese artist → Chinese.
            song("夜空", "虚构乐队", "a4", zh_artist, None),
            // 4: Han only, nothing known → Chinese, less sure.
            song("月光", "无名", "a5", none, None),
            // 5, 6: Latin titles told apart by their lyrics.
            song("Lampu Kota", "Band Rekaan", "a6", none, id_lyrics),
            song("City Lights", "Made Up Band", "a7", none, en_lyrics),
            // 7: a Latin title with nothing else → unknown.
            song("Aurora", "Nobody", "a8", none, None),
            // 8: Han only on an album that has kana elsewhere → Japanese.
            song("花火", "Someone", "a9", none, None),
            song("はなびのあと", "Someone", "a9", none, None),
            // 10–12: an Indonesian album; 12 has no lyrics and takes the album's majority.
            song("Satu", "Grup Rekaan", "a10", none, id_lyrics),
            song("Dua", "Grup Rekaan", "a10", none, id_lyrics),
            song("Tiga", "Grup Rekaan", "a10", none, None),
            // 13: romanized Japanese lyrics on a Latin title: unknown, not a wrong guess…
            song("Yoru", "Nobody Else", "a11", none, Some(latin(&ROMAJI.join("\n")))),
            // 14: …but Japanese when the artist is known to be.
            song("Yoru", "Known Artist", "a12", ja_artist, Some(latin(&ROMAJI.join("\n")))),
        ];
        let g = tag(&songs);
        let langs: Vec<Lang> = g.iter().map(|g| g.lang).collect();
        assert_eq!(
            langs,
            [Lang::Ja, Lang::Ko, Lang::Ja, Lang::Zh, Lang::Zh, Lang::Id, Lang::En, Lang::Unknown, Lang::Ja, Lang::Ja, Lang::Id, Lang::Id, Lang::Id, Lang::Unknown, Lang::Ja]
        );
        assert!(g[4].conf < g[3].conf, "Han only with nothing known is less sure");
        assert!(g[12].conf < g[10].conf, "the album's majority is less sure than lyrics");
        assert_eq!(g[7].conf, 0.0);
    }
}
