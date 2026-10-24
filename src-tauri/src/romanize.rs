//! Lyrics in Latin letters ("pronunciation"), for songs in scripts the listener can't read:
//! Japanese to Hepburn romaji (kakasi), Chinese to pinyin with tones, Korean to Revised
//! Romanization. Everything happens on the device.

use pinyin::ToPinyin;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Script {
    Japanese,
    Chinese,
    Korean,
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' | '\u{ff66}'..='\u{ff9f}')
}
fn is_han(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}
fn is_hangul(c: char) -> bool {
    matches!(c, '\u{ac00}'..='\u{d7a3}')
}

/// Which script the song is in. Kanji and hanzi look alike, so any kana in the song means
/// Japanese; Han characters without kana mean Chinese.
fn script(lines: &[String]) -> Option<Script> {
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
                Script::Japanese => join_endings(&kakasi::convert(line).romaji),
                Script::Chinese => chinese(line),
                Script::Korean => korean(line),
            };
            tidy(&out)
        })
        .collect()
}

/// kakasi writes a verb or adjective ending that follows its kanji as a word of its own
/// ("yasashi ku", "nagare ru"); these endings are never words by themselves.
fn join_endings(romaji: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in romaji.split(' ') {
        match out.last_mut() {
            Some(prev) if matches!(word, "ku" | "ru") && prev.chars().last().is_some_and(|c| c.is_ascii_alphabetic()) => {
                prev.push_str(word)
            }
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
    const INITIAL: [&str; 19] = ["g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];
    const MEDIAL: [&str; 21] = [
        "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we", "wi", "yu", "eu", "ui", "i",
    ];
    const FINAL: [&str; 28] = [
        "", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p", "t", "t", "ng", "t", "t", "k", "t", "p", "t",
    ];
    let mut out = String::new();
    for c in line.chars() {
        if is_hangul(c) {
            let code = c as u32 - 0xac00;
            out.push_str(INITIAL[(code / 588) as usize]);
            out.push_str(MEDIAL[((code % 588) / 28) as usize]);
            out.push_str(FINAL[(code % 28) as usize]);
        } else {
            out.push(c);
        }
    }
    out
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
    use super::*;

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
