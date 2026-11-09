//! Lyrics in Latin letters ("pronunciation"), for songs in scripts the listener can't read:
//! Japanese to Hepburn romaji, Chinese to pinyin with tones, Korean to Revised Romanization.
//! Everything happens on the device.
//!
//! Japanese is split into words by a morphological analyser (lindera, IPADIC), which reads each
//! word in its context: 君 alone is "kimi", after a name "kun"; 座って is "suwatte". kakasi, which
//! only matches dictionary entries, is the fallback for words the dictionary doesn't know.

use std::borrow::Cow;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use pinyin::ToPinyin;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Script {
    Japanese,
    Chinese,
    Korean,
}

pub(crate) fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' | '\u{ff66}'..='\u{ff9f}')
}
pub(crate) fn is_han(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}
pub(crate) fn is_hangul(c: char) -> bool {
    matches!(c, '\u{ac00}'..='\u{d7a3}')
}

/// Which script the song is in. Kanji and hanzi look alike, so any kana in the song means
/// Japanese; Han characters without kana mean Chinese.
pub(crate) fn script(lines: &[String]) -> Option<Script> {
    let all = || lines.iter().flat_map(|l| l.chars());
    if all().any(is_kana) {
        Some(Script::Japanese)
    } else if all().any(is_hangul) {
        Some(Script::Korean)
    } else if all().any(is_han) {
        Some(Script::Chinese)
    } else {
        None
    }
}

/// Each line in Latin letters; empty for lines that are already Latin (or empty).
pub fn lines(lines: &[String]) -> Vec<String> {
    let Some(script) = script(lines) else {
        return vec![String::new(); lines.len()];
    };
    lines
        .iter()
        .map(|line| {
            if !line.chars().any(|c| is_kana(c) || is_han(c) || is_hangul(c)) {
                return String::new();
            }
            let out = match script {
                Script::Japanese => japanese(line),
                Script::Chinese => chinese(line),
                Script::Korean => korean(line),
            };
            tidy(&out)
        })
        .collect()
}

// ---------- Japanese ----------

fn segmenter() -> Option<&'static Segmenter> {
    static SEG: OnceLock<Option<Segmenter>> = OnceLock::new();
    SEG.get_or_init(|| match load_dictionary("embedded://ipadic") {
        Ok(d) => Some(Segmenter::new(Mode::Normal, d, None)),
        Err(e) => {
            log::warn!("no Japanese dictionary: {e}");
            None
        }
    })
    .as_ref()
}

/// A word of a Japanese line: as written, its reading in katakana (None: the dictionary doesn't
/// know it), whether it attaches to the word before (a verb's or adjective's ending, a
/// conjunctive particle like て, a suffix), and whether it's a particle.
struct Word {
    surface: String,
    reading: Option<String>,
    attached: bool,
    /// After a name, joined with a hyphen: Tanaka-kun, Yuki-chan.
    honorific: bool,
    /// Written after a space (the analyser leaves spaces out).
    spaced: bool,
    particle: bool,
}

fn words(line: &str) -> Option<Vec<Word>> {
    let seg = segmenter()?;
    let mut tokens = seg.segment(Cow::Borrowed(line)).ok()?;
    // The word before was a prefix (薄 in 薄明かり): this one joins it.
    let mut after_prefix = false;
    let mut last_end = 0;
    Some(
        tokens
            .iter_mut()
            .map(|t| {
                let surface = t.surface.as_ref().to_string();
                let spaced = line.get(last_end..t.byte_start).is_some_and(|gap| gap.chars().any(char::is_whitespace));
                last_end = t.byte_end;
                let d = t.details();
                let at = |i: usize| d.get(i).copied().filter(|s| *s != "*").unwrap_or("");
                let (pos, sub) = (at(0), at(1));
                let reading = Some(at(7)).filter(|r| !r.is_empty()).map(str::to_string);
                // て, ば and the like belong to the verb before them; から, けど, のに are words.
                let joins_verb = pos == "助詞" && sub == "接続助詞" && matches!(surface.as_str(), "て" | "で" | "ば" | "たって" | "だって" | "ちゃ" | "じゃ");
                // The explaining ん of 行くんだ.
                let explaining_n = sub == "非自立" && surface == "ん";
                // Casual endings: してる, 見ちゃう, 書いとく.
                let casual = pos == "動詞"
                    && sub == "非自立"
                    && ["てる", "てた", "てて", "でる", "でた", "ちゃ", "じゃ", "とく", "とい", "どく"].iter().any(|e| surface.starts_with(e));
                let attached = pos == "助動詞" || joins_verb || explaining_n || casual || sub == "接尾" || after_prefix;
                after_prefix = pos == "接頭詞";
                Word {
                    spaced,
                    attached,
                    honorific: sub == "接尾" && matches!(surface.as_str(), "君" | "さん" | "ちゃん" | "様" | "さま" | "くん" | "殿"),
                    particle: pos == "助詞",
                    surface,
                    reading,
                }
            })
            .collect(),
    )
}

/// How a piece of Japanese is read, in hiragana: each word as the dictionary reads it in
/// context, kakasi for what it doesn't know.
pub(crate) fn reading(text: &str) -> String {
    match words(text) {
        Some(ws) => ws
            .iter()
            .map(|w| match &w.reading {
                Some(r) => to_hiragana(r),
                None => kakasi::convert(&w.surface).hiragana,
            })
            .collect(),
        None => kakasi::convert(text).hiragana,
    }
}

fn japanese(line: &str) -> String {
    let Some(ws) = words(line) else {
        return join_endings(&kakasi::convert(line).romaji);
    };
    // Each written word in kana (converted at the end, so a small っ that ends one part doubles the
    // consonant of the next: スワッ + テ = suwatte), or as it is (Latin letters, punctuation).
    enum Piece {
        Kana(String),
        Other(String),
        Space,
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for w in &ws {
        if w.spaced {
            pieces.push(Piece::Space);
        }
        if w.surface.trim().is_empty() {
            pieces.push(Piece::Space);
            continue;
        }
        let kana = match (w.particle, w.surface.as_str()) {
            // Particles are written as said.
            (true, "は") => Some("ワ".to_string()),
            (true, "へ") => Some("エ".to_string()),
            (true, "を") => Some("オ".to_string()),
            _ => match &w.reading {
                Some(r) => Some(r.clone()),
                None if w.surface.chars().all(|c| is_kana(c) || c == 'ー') => Some(w.surface.clone()),
                None if w.surface.chars().any(|c| is_han(c) || is_kana(c)) => Some(kakasi::convert(&w.surface).hiragana),
                None => None,
            },
        };
        let piece = match kana {
            Some(k) => Piece::Kana(k),
            // Latin letters, digits, punctuation: as they are.
            None => Piece::Other(w.surface.clone()),
        };
        match (pieces.last_mut(), &piece) {
            (Some(Piece::Kana(prev)), Piece::Kana(k)) if w.attached && !w.honorific => prev.push_str(k),
            (Some(Piece::Kana(prev)), Piece::Kana(k)) if w.honorific => {
                prev.push('-');
                prev.push_str(k);
            }
            // Latin text the analyser cut up (don ' t): back together, as written.
            (Some(Piece::Other(prev)), Piece::Other(s)) => prev.push_str(s),
            _ => pieces.push(piece),
        }
    }
    let mut out = String::new();
    for p in pieces {
        let (text, punctuation) = match p {
            Piece::Space => {
                out.push(' ');
                continue;
            }
            Piece::Kana(k) => (k.split('-').map(kana_romaji).collect::<Vec<_>>().join("-"), false),
            Piece::Other(s) => {
                let punctuation = s.chars().next().is_some_and(|c| !c.is_alphanumeric());
                (s, punctuation)
            }
        };
        if !(punctuation || out.is_empty() || out.ends_with(' ')) {
            out.push(' ');
        }
        out.push_str(&text);
    }
    out
}

/// Katakana to hiragana (other characters kept).
fn to_hiragana(s: &str) -> String {
    s.chars().map(|c| if ('\u{30a1}'..='\u{30f6}').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) } else { c }).collect()
}

/// Kana to Hepburn romaji: きゃ kya, しゃ sha, ちゃ cha; っ doubles the next consonant (っち tch),
/// ー repeats the vowel, ん before a vowel or y is "n'".
pub(crate) fn kana_romaji(kana: &str) -> String {
    let chars: Vec<char> = to_hiragana(kana).chars().collect();
    let mut out = String::new();
    let mut double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let (syllable, used) = match chars.get(i + 1).and_then(|&n| digraph(c, n)) {
            Some(s) => (s.to_string(), 2),
            None => match c {
                'っ' => {
                    double = true;
                    i += 1;
                    continue;
                }
                'ー' => {
                    if let Some(v) = out.chars().last().filter(|v| "aiueo".contains(*v)) {
                        out.push(v);
                    }
                    i += 1;
                    continue;
                }
                'ん' => {
                    let vowel_next = chars.get(i + 1).is_some_and(|n| "あいうえおやゆよ".contains(*n));
                    ((if vowel_next { "n'" } else { "n" }).to_string(), 1)
                }
                _ => (mono(c).map(str::to_string).unwrap_or_else(|| c.to_string()), 1),
            },
        };
        if std::mem::take(&mut double) {
            if syllable.starts_with("ch") {
                out.push('t');
            } else if let Some(f) = syllable.chars().next().filter(|f| f.is_ascii_alphabetic() && !"aiueon".contains(*f)) {
                out.push(f);
            }
        }
        out.push_str(&syllable);
        i += used;
    }
    out
}

fn mono(c: char) -> Option<&'static str> {
    const KANA: &str = "あいうえおかきくけこがぎぐげごさしすせそざじずぜぞたちつてとだぢづでどなにぬねのはひふへほばびぶべぼぱぴぷぺぽまみむめもやゆよらりるれろわゐゑをゔぁぃぅぇぉゃゅょゎ";
    const LATIN: [&str; 82] = [
        "a", "i", "u", "e", "o", "ka", "ki", "ku", "ke", "ko", "ga", "gi", "gu", "ge", "go", "sa", "shi", "su", "se", "so", "za", "ji", "zu", "ze", "zo", "ta",
        "chi", "tsu", "te", "to", "da", "ji", "zu", "de", "do", "na", "ni", "nu", "ne", "no", "ha", "hi", "fu", "he", "ho", "ba", "bi", "bu", "be", "bo", "pa",
        "pi", "pu", "pe", "po", "ma", "mi", "mu", "me", "mo", "ya", "yu", "yo", "ra", "ri", "ru", "re", "ro", "wa", "i", "e", "o", "vu", "a", "i", "u", "e",
        "o", "ya", "yu", "yo", "wa",
    ];
    KANA.chars().position(|k| k == c).map(|i| LATIN[i])
}

/// A kana with a small one after it: きゃ kya, ふぁ fa, てぃ ti…
fn digraph(c: char, small: char) -> Option<&'static str> {
    let y = match small {
        'ゃ' => 0,
        'ゅ' => 1,
        'ょ' => 2,
        _ => {
            return match (c, small) {
                ('ふ', 'ぁ') => Some("fa"),
                ('ふ', 'ぃ') => Some("fi"),
                ('ふ', 'ぇ') => Some("fe"),
                ('ふ', 'ぉ') => Some("fo"),
                ('て', 'ぃ') => Some("ti"),
                ('で', 'ぃ') => Some("di"),
                ('と', 'ぅ') => Some("tu"),
                ('ど', 'ぅ') => Some("du"),
                ('ち', 'ぇ') => Some("che"),
                ('し', 'ぇ') => Some("she"),
                ('じ', 'ぇ') => Some("je"),
                ('う', 'ぃ') => Some("wi"),
                ('う', 'ぇ') => Some("we"),
                ('う', 'ぉ') => Some("wo"),
                ('ゔ', 'ぁ') => Some("va"),
                ('ゔ', 'ぃ') => Some("vi"),
                ('ゔ', 'ぇ') => Some("ve"),
                ('ゔ', 'ぉ') => Some("vo"),
                _ => None,
            };
        }
    };
    let row: [&'static str; 3] = match c {
        'き' => ["kya", "kyu", "kyo"],
        'ぎ' => ["gya", "gyu", "gyo"],
        'し' => ["sha", "shu", "sho"],
        'じ' | 'ぢ' => ["ja", "ju", "jo"],
        'ち' => ["cha", "chu", "cho"],
        'に' => ["nya", "nyu", "nyo"],
        'ひ' => ["hya", "hyu", "hyo"],
        'び' => ["bya", "byu", "byo"],
        'ぴ' => ["pya", "pyu", "pyo"],
        'み' => ["mya", "myu", "myo"],
        'り' => ["rya", "ryu", "ryo"],
        _ => return None,
    };
    Some(row[y])
}

/// kakasi writes a verb or adjective ending that follows its kanji as a word of its own
/// ("yasashi ku", "nagare ru"); these endings are never words by themselves.
fn join_endings(romaji: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in romaji.split(' ') {
        match out.last_mut() {
            Some(prev) if matches!(word, "ku" | "ru") && prev.chars().last().is_some_and(|c| c.is_ascii_alphabetic()) => prev.push_str(word),
            _ => out.push(word.to_string()),
        }
    }
    out.join(" ")
}

/// Hanzi to pinyin with tone marks, a space between syllables; other characters kept.
fn chinese(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars() {
        match c.to_pinyin() {
            Some(p) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(p.with_tone());
                out.push(' ');
            }
            None => out.push(c),
        }
    }
    out
}

/// Hangul syllables to Revised Romanization (letter by letter, without the sound changes
/// between syllables).
fn korean(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars() {
        match hangul_syllable(c) {
            Some(s) => out.push_str(&s),
            None => out.push(c),
        }
    }
    out
}

/// One Hangul syllable in Revised Romanization (letter by letter); None for other characters.
pub(crate) fn hangul_syllable(c: char) -> Option<String> {
    const INITIAL: [&str; 19] = ["g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];
    const MEDIAL: [&str; 21] = ["a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we", "wi", "yu", "eu", "ui", "i"];
    const FINAL: [&str; 28] =
        ["", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p", "t", "t", "ng", "t", "t", "k", "t", "p", "t"];
    if !is_hangul(c) {
        return None;
    }
    let code = c as u32 - 0xac00;
    Some(format!("{}{}{}", INITIAL[(code / 588) as usize], MEDIAL[((code % 588) / 28) as usize], FINAL[(code % 28) as usize]))
}

/// Single spaces, full-width punctuation as ASCII, no space before punctuation.
fn tidy(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '、' | '，' => ',',
            '。' => '.',
            '！' => '!',
            '？' => '?',
            '「' | '」' | '『' | '』' => '"',
            c => c,
        })
        .collect();
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.replace(" ,", ",").replace(" .", ".").replace(" !", "!").replace(" ?", "?")
}

#[cfg(test)]
mod tests {
    fn r(lines: &[&str]) -> Vec<String> {
        super::lines(&lines.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn japanese_lines() {
        let out = r(&["祈りを束ね 空に放て", "この心を流れる", "Hello"]);
        println!("{out:?}");
        assert!(out[0].to_lowercase().contains("inori"));
        assert!(out[1].to_lowercase().contains("kokoro"));
        assert!(out[1].contains("nagareru"), "{}", out[1]);
        assert!(r(&["優しく呼んだその声が"])[0].starts_with("yasashiku"));
        assert_eq!(out[2], "");
    }

    #[test]
    fn japanese_words_read_in_context() {
        let out = r(&["君と座ってバス停見上げた", "それでも分かるから", "田中君は今日も来た", "何も言わなくたって", "貴方の側に"]);
        println!("{out:?}");
        assert_eq!(out[0], "kimi to suwatte basutei miageta");
        assert_eq!(out[1], "soredemo wakaru kara");
        assert!(out[2].starts_with("tanaka-kun wa kyou mo"), "{}", out[2]);
        assert_eq!(out[3], "nani mo iwanakutatte");
        assert_eq!(out[4], "anata no gawa ni");
    }

    #[test]
    fn kana_to_hepburn() {
        use super::kana_romaji;
        assert_eq!(kana_romaji("キャッチ"), "kyatchi");
        assert_eq!(kana_romaji("ラーメン"), "raamen");
        assert_eq!(kana_romaji("シンヨウ"), "shin'you");
        assert_eq!(kana_romaji("ずっと"), "zutto");
        assert_eq!(kana_romaji("ティー"), "tii");
    }

    #[test]
    fn chinese_lines() {
        let out = r(&["命运在人之侧"]);
        println!("{out:?}");
        assert_eq!(out[0], "mìng yùn zài rén zhī cè");
    }

    #[test]
    fn korean_lines() {
        let out = r(&["사랑해 요"]);
        assert_eq!(out[0], "saranghae yo");
    }

    #[test]
    fn latin_songs_need_nothing() {
        assert_eq!(r(&["Simple and clean"]), vec![String::new()]);
    }
}
/// A saved song's lyrics with their romaji, to read through:
/// `LYRICS_JSON=<data dir>/lyrics/<id>.json cargo test --lib dump_song -- --ignored --nocapture`.
#[test]
#[ignore]
fn dump_song() {
    let Ok(f) = std::env::var("LYRICS_JSON") else {
        return;
    };
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(f).unwrap()).unwrap();
    let text: Vec<String> =
        v["lyrics"]["synced"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap_or("").to_string()).filter(|l| !l.is_empty()).collect();
    for (l, r) in text.iter().zip(lines(&text)) {
        println!("{l}\n   {r}");
    }
}
