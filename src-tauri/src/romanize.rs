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

static SEG: OnceLock<Option<Segmenter>> = OnceLock::new();

fn segmenter() -> Option<&'static Segmenter> {
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
    words_with(segmenter()?, line)
}

fn words_with(seg: &Segmenter, line: &str) -> Option<Vec<Word>> {
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

/// How each piece of Japanese is read, in Latin letters (Hepburn, folded later by the caller),
/// for search: one dictionary for the whole batch. The lyrics view's dictionary is used when it's
/// loaded already; otherwise one is loaded for the batch and let go after it (it's large).
pub(crate) fn readings_for_search(texts: &[String]) -> Vec<String> {
    let local;
    let seg = match SEG.get().and_then(Option::as_ref) {
        Some(s) => s,
        None => match load_dictionary("embedded://ipadic") {
            Ok(d) => {
                local = Segmenter::new(Mode::Normal, d, None);
                &local
            }
            Err(e) => {
                log::warn!("no Japanese dictionary: {e}");
                return texts.iter().map(|t| kana_romaji(&kakasi::convert(t).hiragana)).collect();
            }
        },
    };
    texts
        .iter()
        .map(|t| {
            let kana: String = match words_with(seg, t) {
                Some(ws) => ws
                    .iter()
                    .map(|w| match &w.reading {
                        Some(r) => to_hiragana(r),
                        None if w.surface.chars().any(|c| is_han(c) || is_kana(c)) => kakasi::convert(&w.surface).hiragana,
                        None => format!(" {} ", w.surface),
                    })
                    .collect(),
                None => kakasi::convert(t).hiragana,
            };
            kana_romaji(&kana)
        })
        .collect()
}

/// Hanzi as toneless pinyin, a space between syllables ("东京" → "dong jing"); None without hanzi.
pub(crate) fn pinyin_plain(s: &str) -> Option<String> {
    if !s.chars().any(is_han) {
        return None;
    }
    let mut out = String::new();
    for c in s.chars() {
        match c.to_pinyin() {
            Some(p) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(p.plain());
                out.push(' ');
            }
            None => out.push(c),
        }
    }
    Some(out.split_whitespace().collect::<Vec<_>>().join(" "))
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

// ---------- readings above the characters (furigana, pinyin) ----------

/// A piece of a line: its text, and the reading shown above it (None: nothing above). A line's
/// pieces together are the line, character for character.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct RubyPart {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruby: Option<String>,
}

fn part(text: &str, ruby: Option<String>) -> RubyPart {
    RubyPart { text: text.to_string(), ruby }
}

/// Kanji for this purpose, with the repeat mark 々 and the counter ヶ written between them.
fn kanji_like(c: char) -> bool {
    is_han(c) || c == '々' || c == '〆'
}

/// Readings above the characters of each line: in Japanese, hiragana above the kanji of each word
/// as the dictionary reads it in context (only the kanji: 優しく is 優(やさ)しく); in Chinese, each
/// hanzi's pinyin. An empty list for a line that needs none (kana, Latin letters, digits, Korean).
pub fn ruby(lines: &[String]) -> Vec<Vec<RubyPart>> {
    match script(lines) {
        Some(Script::Japanese) => {
            let seg = segmenter();
            lines.iter().map(|l| if l.chars().any(kanji_like) { japanese_ruby(seg, l) } else { Vec::new() }).collect()
        }
        Some(Script::Chinese) => lines.iter().map(|l| chinese_ruby(l)).collect(),
        _ => vec![Vec::new(); lines.len()],
    }
}

fn chinese_ruby(line: &str) -> Vec<RubyPart> {
    if !line.chars().any(is_han) {
        return Vec::new();
    }
    let mut out: Vec<RubyPart> = Vec::new();
    for c in line.chars() {
        match c.to_pinyin() {
            Some(p) => out.push(RubyPart { text: c.to_string(), ruby: Some(p.with_tone().to_string()) }),
            None => match out.last_mut() {
                Some(last) if last.ruby.is_none() => last.text.push(c),
                _ => out.push(RubyPart { text: c.to_string(), ruby: None }),
            },
        }
    }
    out
}

fn japanese_ruby(seg: Option<&Segmenter>, line: &str) -> Vec<RubyPart> {
    let mut out: Vec<RubyPart> = Vec::new();
    let mut push = |p: RubyPart| match (out.last_mut(), &p.ruby) {
        (Some(last), None) if last.ruby.is_none() => last.text.push_str(&p.text),
        _ => out.push(p),
    };
    let tokens = seg.and_then(|s| s.segment(Cow::Borrowed(line)).ok());
    let Some(mut tokens) = tokens else {
        // No dictionary: kakasi's reading of the whole line can't be split up safely.
        push(part(line, None));
        return out;
    };
    let mut at = 0;
    for t in tokens.iter_mut() {
        let (start, end) = (t.byte_start, t.byte_end);
        if start < at || end > line.len() || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
            continue;
        }
        if start > at {
            push(part(&line[at..start], None));
        }
        let surface = &line[start..end];
        at = end;
        if !surface.chars().any(kanji_like) {
            push(part(surface, None));
            continue;
        }
        let d = t.details();
        let reading = d.get(7).copied().filter(|r| !r.is_empty() && *r != "*").map(to_hiragana).or_else(|| {
            let k = kakasi::convert(surface).hiragana;
            (!k.chars().any(kanji_like)).then_some(k)
        });
        match reading {
            Some(r) => {
                for p in split_okurigana(surface, &r) {
                    push(p);
                }
            }
            None => push(part(surface, None)),
        }
    }
    if at < line.len() {
        push(part(&line[at..], None));
    }
    out
}

/// A word and its reading, with the reading over the kanji only: the kana written in the word
/// (okurigana and the like) match the same kana in the reading, and the kanji between them take
/// the rest (取り扱い とりあつかい: 取(と)り扱(あつか)い). When they can't be matched up, the
/// whole word carries the reading.
pub(crate) fn split_okurigana(surface: &str, reading: &str) -> Vec<RubyPart> {
    let reading: Vec<char> = to_hiragana(reading).chars().collect();
    // Runs of kanji and of everything else.
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in surface.chars() {
        let k = kanji_like(c);
        match runs.last_mut() {
            Some((kk, s)) if *kk == k => s.push(c),
            _ => runs.push((k, c.to_string())),
        }
    }
    if to_hiragana(surface).chars().eq(reading.iter().copied()) {
        return vec![part(surface, None)];
    }
    fn fit(runs: &[(bool, String)], reading: &[char], out: &mut Vec<(usize, usize)>) -> bool {
        let Some(((kanji, text), rest)) = runs.split_first() else { return reading.is_empty() };
        if !kanji {
            let kana: Vec<char> = to_hiragana(text).chars().collect();
            if reading.len() >= kana.len() && reading[..kana.len()] == kana[..] {
                out.push((0, kana.len()));
                if fit(rest, &reading[kana.len()..], out) {
                    return true;
                }
                out.pop();
            }
            return false;
        }
        // At least one kana per kanji run, leaving one for each later run.
        let later = rest.len();
        for n in 1..=reading.len().saturating_sub(later) {
            out.push((0, n));
            if fit(rest, &reading[n..], out) {
                return true;
            }
            out.pop();
        }
        false
    }
    let mut spans = Vec::new();
    if !fit(&runs, &reading, &mut spans) {
        return vec![part(surface, Some(reading.iter().collect()))];
    }
    let mut pos = 0;
    runs.iter()
        .zip(spans)
        .map(|((kanji, text), (_, n))| {
            let r: String = reading[pos..pos + n].iter().collect();
            pos += n;
            part(text, kanji.then_some(r))
        })
        .collect()
}

/// Each song's lines read in Latin letters, for finding a song by its lyrics (`lyricindex`):
/// Japanese with one dictionary for the whole batch (the lyrics view's when it's loaded already,
/// else one let go after the batch), Chinese as toneless pinyin, Korean romanized; "" for lines
/// that need none.
pub(crate) fn readings_for_index(songs: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = songs.iter().map(|s| vec![String::new(); s.len()]).collect();
    let mut japanese_lines: Vec<String> = Vec::new();
    let mut japanese_at: Vec<(usize, usize)> = Vec::new();
    for (si, lines) in songs.iter().enumerate() {
        let script = script(lines);
        for (li, l) in lines.iter().enumerate() {
            if !l.chars().any(|c| is_kana(c) || is_han(c) || is_hangul(c)) {
                continue;
            }
            match script {
                Some(Script::Japanese) => {
                    japanese_lines.push(l.clone());
                    japanese_at.push((si, li));
                }
                Some(Script::Chinese) => out[si][li] = pinyin_plain(l).unwrap_or_default(),
                Some(Script::Korean) => out[si][li] = tidy(&korean(l)),
                None => {}
            }
        }
    }
    if !japanese_lines.is_empty() {
        for ((si, li), r) in japanese_at.into_iter().zip(readings_for_search(&japanese_lines)) {
            out[si][li] = r;
        }
    }
    out
}

#[cfg(test)]
mod ruby_tests {
    use super::*;

    fn r(line: &str) -> Vec<(String, Option<String>)> {
        ruby(&[line.to_string()]).remove(0).into_iter().map(|p| (p.text, p.ruby)).collect()
    }
    fn s(t: &str, r: Option<&str>) -> (String, Option<String>) {
        (t.to_string(), r.map(str::to_string))
    }

    // Invented lines (no real lyrics in fixtures).
    #[test]
    fn kanji_with_okurigana() {
        assert_eq!(split_okurigana("優しく", "ヤサシク").into_iter().map(|p| (p.text, p.ruby)).collect::<Vec<_>>(), [s("優", Some("やさ")), s("しく", None)]);
        assert_eq!(
            split_okurigana("取り扱い", "トリアツカイ").into_iter().map(|p| (p.text, p.ruby)).collect::<Vec<_>>(),
            [s("取", Some("と")), s("り", None), s("扱", Some("あつか")), s("い", None)]
        );
        assert_eq!(split_okurigana("お酒", "オサケ").into_iter().map(|p| (p.text, p.ruby)).collect::<Vec<_>>(), [s("お", None), s("酒", Some("さけ"))]);
        // Can't be matched up: the whole word carries the reading.
        assert_eq!(split_okurigana("今日", "キョウ").into_iter().map(|p| (p.text, p.ruby)).collect::<Vec<_>>(), [s("今日", Some("きょう"))]);
        assert_eq!(split_okurigana("見え", "ミル").into_iter().map(|p| (p.text, p.ruby)).collect::<Vec<_>>(), [s("見え", Some("みる"))]);
    }

    #[test]
    fn readings_go_above_kanji_only() {
        let parts = r("今日は雨が降る");
        let joined: String = parts.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(joined, "今日は雨が降る", "the pieces are the line");
        assert!(parts.contains(&s("今日", Some("きょう"))), "{parts:?}");
        assert!(parts.contains(&s("雨", Some("あめ"))), "{parts:?}");
        assert!(parts.contains(&s("降", Some("ふ"))), "{parts:?}");
        // Kana between them carry nothing.
        assert!(parts.iter().filter(|(_, r)| r.is_none()).all(|(t, _)| !t.chars().any(is_han)));
        // Mixed with Latin letters and digits: only the kanji.
        let parts = r("Test 2024 青い空を見上げた");
        assert_eq!(parts.iter().map(|(t, _)| t.as_str()).collect::<String>(), "Test 2024 青い空を見上げた");
        assert_eq!(parts[0], s("Test 2024 ", None));
        assert!(parts.contains(&s("空", Some("そら"))), "{parts:?}");
    }

    #[test]
    fn kana_latin_and_numbers_need_nothing() {
        let lines: Vec<String> = ["ひらがなだけ", "カタカナ", "Hello 123", "青い空"].iter().map(|s| s.to_string()).collect();
        let out = ruby(&lines);
        assert!(out[0].is_empty() && out[1].is_empty() && out[2].is_empty());
        assert!(!out[3].is_empty());
        assert!(ruby(&["오늘은 비가 온다".to_string()])[0].is_empty());
    }

    #[test]
    fn pinyin_above_hanzi() {
        let out = ruby(&["今天下雨了 ok".to_string()]).remove(0);
        assert_eq!(out[0], RubyPart { text: "今".into(), ruby: Some("jīn".into()) });
        assert_eq!(out.last().unwrap(), &RubyPart { text: " ok".into(), ruby: None });
        assert_eq!(out.iter().map(|p| p.text.as_str()).collect::<String>(), "今天下雨了 ok");
    }

    #[test]
    fn index_readings_per_script() {
        let songs = vec![vec!["今日は雨が降る".to_string(), "rain".to_string()], vec!["今天下雨了".to_string()], vec!["오늘은 비가 온다".to_string()]];
        let out = readings_for_index(&songs);
        assert!(out[0][0].contains("ame"), "{:?}", out[0]);
        assert_eq!(out[0][1], "");
        assert_eq!(out[1][0], "jin tian xia yu le");
        assert_eq!(out[2][0], "oneuleun biga onda");
    }
}
