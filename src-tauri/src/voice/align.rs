//! Places every word of the lyrics on the singing ("forced alignment").
//!
//! The listening model (`listen.rs`) says, every 20 ms, how likely each Latin letter is being sung.
//! The lyrics are spelled in those letters (romaji for Japanese, pinyin for Chinese, Revised
//! Romanization for Korean, the words themselves otherwise), and a Viterbi pass finds the most
//! likely moment for every letter, in order. A "blank" between letters takes the music, breaths
//! and silence.
//!
//! When the lyrics already have line times (LRC), they guide the search: each line may only land
//! near its own time (after the song's overall offset), and a line that still comes out far from
//! it keeps the LRC time. That keeps one hard line (a whispered ad-lib, a "la-la-la") from pulling
//! its neighbours away.

use unicode_normalization::UnicodeNormalization;

use crate::lyrics::{Line, Word};
use crate::romanize::{Script, hangul_syllable, is_han, is_hangul, is_kana};

/// Seconds per model frame.
pub const FRAME: f64 = 0.02;
/// The model's letters in output order; 0 is the CTC blank.
pub const VOCAB: &str = "-aienoutsrmkldghybpwcvjzf'qx";
/// Bumped when alignment changes, so saved results are redone (from the saved listening, fast).
pub const VERSION: u32 = 1;

/// How far (seconds) a line may land from its LRC time, after the song's offset.
const BAND: f64 = 2.5;
/// A line placed further than this from its LRC time keeps the LRC time.
const TRUST: f64 = 2.0;
/// Viterbi keeps 2 bits per frame and state; songs needing more than this many cells are skipped.
const MAX_CELLS: usize = 1_200_000_000;

/// Log-probabilities of every letter, `frames` rows of `VOCAB.len()`.
pub struct Emissions {
    pub frames: usize,
    pub logp: Vec<f32>,
}

impl Emissions {
    fn at(&self, t: usize, token: u8) -> f32 {
        self.logp[t * VOCAB.len() + token as usize]
    }
}

fn token(c: char) -> Option<u8> {
    VOCAB.char_indices().find(|&(_, v)| v == c).map(|(i, _)| i as u8).filter(|&i| i > 0)
}

/// Lowercase Latin letters of `s` as model tokens (accents dropped, other characters ignored).
fn letters(s: &str) -> Vec<u8> {
    s.nfkd()
        .flat_map(char::to_lowercase)
        .map(|c| if c == '’' || c == '`' { '\'' } else { c })
        .filter_map(token)
        .collect()
}

/// A piece of a line as it is shown, with the letters it's sung as. Pieces are words for Latin
/// text, morae (kana) or kanji groups for Japanese, and characters for Chinese and Korean, so the
/// sweep moves syllable by syllable.
#[derive(Debug, Clone)]
struct Unit {
    text: String,
    letters: Vec<u8>,
    /// Inside parentheses: backing vocals, often quiet or layered; left to the blank.
    aside: bool,
}

fn is_small_kana(c: char) -> bool {
    matches!(
        c,
        'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ'
    )
}

fn is_kanji(c: char) -> bool {
    is_han(c) || c == '々' || c == '〆'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() && !is_kana(c) && !is_kanji(c) && !is_hangul(c)
}

fn romaji(kana: &str) -> Vec<u8> {
    letters(&kakasi::convert(kana).romaji)
}

/// How a kanji group is read, using the kana after it (okurigana) to pick the reading.
fn kanji_reading(kanji: &str, okurigana: &str) -> String {
    if !okurigana.is_empty() {
        let both = kakasi::convert(format!("{kanji}{okurigana}")).hiragana;
        let tail = kakasi::convert(okurigana).hiragana;
        if let Some(r) = both.strip_suffix(tail.as_str())
            && !r.is_empty()
        {
            return r.to_string();
        }
    }
    kakasi::convert(kanji).hiragana
}

fn units(line: &str, script: Option<Script>) -> Vec<Unit> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<Unit> = Vec::new();
    // Characters that belong before the next unit (opening punctuation, a small tsu).
    let mut lead = String::new();
    let mut lead_letters = String::new();
    let mut depth = 0usize;
    fn push(out: &mut Vec<Unit>, lead: &mut String, text: &str, letters: Vec<u8>, depth: usize) {
        out.push(Unit {
            text: std::mem::take(lead) + text,
            letters,
            aside: depth > 0,
        });
    }
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_word_char(c) {
            let start = i;
            while i < chars.len()
                && (is_word_char(chars[i])
                    || (matches!(chars[i], '\'' | '’') && chars.get(i + 1).is_some_and(|&n| is_word_char(n))))
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            push(&mut out, &mut lead, &word, letters(&word), depth);
            continue;
        }
        if is_kanji(c) && script != Some(Script::Chinese) && script != Some(Script::Korean) {
            let start = i;
            while i < chars.len() && is_kanji(chars[i]) {
                i += 1;
            }
            let mut end = i;
            while end < chars.len() && ('\u{3041}'..='\u{3096}').contains(&chars[end]) {
                end += 1;
            }
            let kanji: String = chars[start..i].iter().collect();
            let okurigana: String = chars[i..end].iter().collect();
            let reading = kanji_reading(&kanji, &okurigana);
            push(&mut out, &mut lead, &kanji, romaji(&reading), depth);
            continue;
        }
        if is_kana(c) {
            match c {
                // Long vowel: the previous mora's vowel again.
                'ー' | '〜' | '～' => {
                    let vowel = out
                        .last()
                        .and_then(|u| u.letters.iter().rev().find(|&&t| matches!(t, 1 | 2 | 3 | 5 | 6)).copied());
                    if let Some(last) = out.last_mut() {
                        last.text.push(c);
                        last.letters.extend(vowel);
                    } else {
                        lead.push(c);
                    }
                    i += 1;
                    continue;
                }
                // Small tsu doubles the next consonant: it goes with the next mora.
                'っ' | 'ッ' => {
                    lead.push(c);
                    lead_letters.push(c);
                    i += 1;
                    continue;
                }
                _ => {}
            }
            let mut mora = c.to_string();
            i += 1;
            while i < chars.len() && is_small_kana(chars[i]) {
                mora.push(chars[i]);
                i += 1;
            }
            let spelled = format!("{}{mora}", std::mem::take(&mut lead_letters));
            push(&mut out, &mut lead, &mora, romaji(&spelled), depth);
            continue;
        }
        if is_kanji(c) {
            // Chinese (or hanja in a Korean song): one syllable per character.
            use pinyin::ToPinyin;
            let spelled = c.to_pinyin().map(|p| letters(p.plain())).unwrap_or_default();
            push(&mut out, &mut lead, &c.to_string(), spelled, depth);
            i += 1;
            continue;
        }
        if let Some(s) = hangul_syllable(c) {
            push(&mut out, &mut lead, &c.to_string(), letters(&s), depth);
            i += 1;
            continue;
        }
        // Spaces and punctuation stay with the unit before them (or the next one, at the start).
        match c {
            '(' | '（' | '[' => depth += 1,
            ')' | '）' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        let opening = matches!(c, '(' | '（' | '[' | '「' | '『' | '"' | '“' | '‘' | '¿' | '¡');
        match out.last_mut() {
            Some(last) if !opening => last.text.push(c),
            _ => lead.push(c),
        }
        i += 1;
    }
    if !lead.is_empty() {
        match out.last_mut() {
            Some(last) => last.text.push_str(&lead),
            None => out.push(Unit {
                text: lead,
                letters: Vec::new(),
                aside: false,
            }),
        }
    }
    out
}

/// Sounds rather than words ("la la la", "oh oh", "yeah"): sung loosely, if at all as written.
fn vocalising(units: &[Unit]) -> bool {
    const SOUNDS: &[&str] = &[
        "la", "na", "da", "ah", "aa", "oh", "o", "ooh", "oo", "uh", "um", "mm", "hm", "hmm", "mmm", "ha", "eh", "woah", "whoa", "wo",
        "yeah", "ye", "hey", "yo", "lalala", "nanana", "lala", "nana", "ohoh", "ahah",
    ];
    let mut any = false;
    for u in units.iter().filter(|u| !u.letters.is_empty()) {
        let w: String = u.letters.iter().map(|&t| VOCAB.as_bytes()[t as usize] as char).collect();
        if !SOUNDS.contains(&w.as_str()) {
            return false;
        }
        any = true;
    }
    any
}

/// The most likely first and last frame of every token. `band` (per token, inclusive) limits
/// where each may be. None if there's no way through (or the song is too long to search).
fn viterbi(em: &Emissions, tokens: &[u8], band: Option<&[(usize, usize)]>) -> Option<Vec<(usize, usize)>> {
    let t_max = em.frames;
    let l = tokens.len();
    let s_max = 2 * l + 1;
    if l == 0 || t_max < l || t_max.saturating_mul(s_max) > MAX_CELLS {
        return None;
    }
    let label = |s: usize| if s.is_multiple_of(2) { 0 } else { tokens[s / 2] };
    // Frames each state may be in: tokens their band; a blank between two tokens spans both.
    let window = |s: usize| -> (usize, usize) {
        let Some(b) = band else { return (0, t_max - 1) };
        if s % 2 == 1 {
            b[s / 2]
        } else {
            let k = s / 2;
            let lo = if k == 0 { 0 } else { b[k - 1].0 };
            let hi = if k == l { t_max - 1 } else { b[k].1 };
            (lo, hi)
        }
    };
    let windows: Vec<(usize, usize)> = (0..s_max).map(window).collect();
    const NEG: f32 = f32::NEG_INFINITY;
    let mut prev = vec![NEG; s_max];
    let mut cur = vec![NEG; s_max];
    let row = s_max.div_ceil(4);
    let mut back = vec![0u8; t_max * row];
    for s in 0..2.min(s_max) {
        if windows[s].0 == 0 {
            prev[s] = em.at(0, label(s));
        }
    }
    for t in 1..t_max {
        // States that can be reached by now and can still reach the end.
        let lo = (s_max).saturating_sub(2 * (t_max - t));
        let hi = (2 * t + 1).min(s_max - 1);
        cur[..lo].fill(NEG);
        if hi + 1 < s_max {
            cur[hi + 1..].fill(NEG);
        }
        let bp = &mut back[t * row..(t + 1) * row];
        for s in lo..=hi {
            let (a, b) = windows[s];
            if t < a || t > b {
                cur[s] = NEG;
                continue;
            }
            let mut best = prev[s];
            let mut from = 0u8;
            if s >= 1 && prev[s - 1] > best {
                best = prev[s - 1];
                from = 1;
            }
            if s >= 2 && s % 2 == 1 && tokens[s / 2] != tokens[s / 2 - 1] && prev[s - 2] > best {
                best = prev[s - 2];
                from = 2;
            }
            cur[s] = best + em.at(t, label(s));
            bp[s / 4] |= from << ((s % 4) * 2);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let mut s = if s_max >= 2 && prev[s_max - 2] > prev[s_max - 1] { s_max - 2 } else { s_max - 1 };
    if !prev[s].is_finite() {
        return None;
    }
    let mut spans = vec![(usize::MAX, 0usize); l];
    for t in (0..t_max).rev() {
        if s % 2 == 1 {
            let k = s / 2;
            spans[k].0 = t;
            spans[k].1 = spans[k].1.max(t);
        }
        if t > 0 {
            let from = (back[t * row + s / 4] >> ((s % 4) * 2)) & 3;
            s -= from as usize;
        }
    }
    spans.iter().all(|&(a, _)| a != usize::MAX).then_some(spans)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

/// What came out of aligning a song's lyrics.
pub struct Aligned {
    pub lines: Vec<Line>,
    /// How much later than the LRC times the singing is, overall (0 without LRC).
    pub offset: f64,
    /// Lines placed on the singing, out of the lines with words.
    pub placed: usize,
    pub lines_with_words: usize,
}

/// Places the lyrics on the singing. `times` are the LRC line times, if the lyrics have them
/// (same length as `texts`; empty texts are instrumental gaps).
pub fn align(em: &Emissions, texts: &[String], times: Option<&[f64]>) -> Option<Aligned> {
    let script = crate::romanize::script(texts);
    let per_line: Vec<Vec<Unit>> = texts.iter().map(|t| units(t, script)).collect();
    // Lines the model listens for: ones with letters outside parentheses that aren't just sounds.
    let listened: Vec<bool> = per_line
        .iter()
        .map(|u| u.iter().any(|u| !u.aside && !u.letters.is_empty()) && !vocalising(u))
        .collect();
    let mut tokens = Vec::new();
    let mut owner = Vec::new(); // (line, unit) of every token
    for (li, us) in per_line.iter().enumerate() {
        if !listened[li] {
            continue;
        }
        for (ui, u) in us.iter().enumerate() {
            if u.aside {
                continue;
            }
            for &t in &u.letters {
                tokens.push(t);
                owner.push((li, ui));
            }
        }
    }
    let first = viterbi(em, &tokens, None)?;
    let line_start = |spans: &[(usize, usize)]| {
        let mut starts = vec![None; texts.len()];
        for (k, &(li, _)) in owner.iter().enumerate() {
            starts[li].get_or_insert(spans[k].0 as f64 * FRAME);
        }
        starts
    };
    let end_time = em.frames as f64 * FRAME;
    let mut offset = 0.0;
    let mut spans = first;
    if let Some(times) = times {
        let starts = line_start(&spans);
        offset = median(
            starts
                .iter()
                .zip(times)
                .filter_map(|(s, t)| s.map(|s| s - t))
                .collect(),
        )
        .unwrap_or(0.0);
        // Each line near its own time; the next line's time (or the end) closes its window.
        let next_time = |li: usize| {
            times[li + 1..]
                .first()
                .map(|t| t + offset)
                .unwrap_or(end_time)
                .max(times[li] + offset)
        };
        let frame = |s: f64| ((s / FRAME).max(0.0) as usize).min(em.frames - 1);
        let mut band: Vec<(usize, usize)> = owner
            .iter()
            .map(|&(li, _)| (frame(times[li] + offset - BAND), frame(next_time(li) + BAND)))
            .collect();
        for k in 1..band.len() {
            band[k].0 = band[k].0.max(band[k - 1].0);
        }
        for k in (0..band.len().saturating_sub(1)).rev() {
            band[k].1 = band[k].1.min(band[k + 1].1);
        }
        if let Some(guided) = viterbi(em, &tokens, Some(&band)) {
            spans = guided;
        }
    }

    // Unit spans (first frame of its first letter, last frame of its last letter).
    let mut unit_spans: Vec<Vec<Option<(f64, f64)>>> = per_line.iter().map(|u| vec![None; u.len()]).collect();
    for (k, &(li, ui)) in owner.iter().enumerate() {
        let (a, b) = spans[k];
        let (a, b) = (a as f64 * FRAME, (b + 1) as f64 * FRAME);
        let slot = &mut unit_spans[li][ui];
        *slot = Some(match *slot {
            Some((s, e)) => (s.min(a), e.max(b)),
            None => (a, b),
        });
    }

    let mut lines = Vec::with_capacity(texts.len());
    let mut placed = 0;
    let mut prev_time = 0.0f64;
    // Where the line before ended: lyrics without times put an unplaced line there.
    let mut prev_end = 0.0f64;
    for (li, text) in texts.iter().enumerate() {
        let start = unit_spans[li].iter().flatten().map(|s| s.0).next();
        let expected = times.map(|t| (t[li] + offset).max(0.0));
        let trusted = match (start, expected) {
            (Some(s), Some(e)) => (s - e).abs() <= TRUST,
            (Some(_), None) => true,
            _ => false,
        };
        let next_line_time = || {
            (li + 1..texts.len())
                .find_map(|n| unit_spans[n].iter().flatten().map(|s| s.0).next().or(times.map(|t| t[n] + offset)))
                .unwrap_or(end_time)
        };
        if !trusted {
            // Keep the LRC time (moved by the song's offset); without one, follow the line before.
            let time = expected.unwrap_or(prev_end.min(next_line_time())).max(prev_time);
            prev_time = time;
            prev_end = time;
            lines.push(Line {
                time,
                text: text.clone(),
                words: Vec::new(),
                singers: Vec::new(),
            });
            continue;
        }
        placed += 1;
        let time = start.unwrap_or(prev_time).max(prev_time);
        let line_end = next_line_time();
        let mut words: Vec<Word> = Vec::new();
        let us = &per_line[li];
        for (ui, u) in us.iter().enumerate() {
            match unit_spans[li][ui] {
                Some((a, b)) => words.push(Word {
                    time: a.max(time),
                    text: u.text.clone(),
                    end: Some(b),
                }),
                // No letters (a number, an aside in parentheses): shown with the word before it.
                None => match words.last_mut() {
                    Some(w) => w.text.push_str(&u.text),
                    None => words.push(Word {
                        time,
                        text: u.text.clone(),
                        end: None,
                    }),
                },
            }
        }
        // A word lasts until the next one starts, unless a real pause follows it.
        let n = words.len();
        for i in 0..n {
            let own = words[i].end.unwrap_or(words[i].time);
            let next = if i + 1 < n { words[i + 1].time } else { line_end };
            let end = if next - own < 0.6 { next } else { own + 0.25 };
            words[i].end = Some(end.max(words[i].time + 0.05).min(next.max(words[i].time + 0.05)));
        }
        prev_time = time;
        prev_end = words.last().and_then(|w| w.end).unwrap_or(time);
        lines.push(Line {
            time,
            text: text.clone(),
            words,
            singers: Vec::new(),
        });
    }
    let lines_with_words = listened.iter().filter(|&&l| l).count();
    Some(Aligned {
        lines,
        offset,
        placed,
        lines_with_words,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spelled(u: &Unit) -> String {
        u.letters.iter().map(|&t| VOCAB.as_bytes()[t as usize] as char).collect()
    }

    #[test]
    fn latin_words() {
        let u = units("I'm not the only one, (yeah)", None);
        let t: Vec<_> = u.iter().map(|u| (u.text.as_str(), spelled(u), u.aside)).collect();
        assert_eq!(t[0], ("I'm ", "i'm".into(), false));
        assert_eq!(t[4].0, "one, ");
        assert_eq!(t[5], ("(yeah)", "yeah".into(), true));
        let u = units("Berikan senyuman 'tuk sebuah perpisahan", None);
        assert_eq!(u.len(), 5);
        assert_eq!(u.iter().map(|u| u.text.as_str()).collect::<String>(), "Berikan senyuman 'tuk sebuah perpisahan");
        assert_eq!(spelled(&u[2]), "tuk");
    }

    #[test]
    fn japanese_morae() {
        let u = units("もう会いたくないよ", Some(Script::Japanese));
        let t: Vec<_> = u.iter().map(|u| (u.text.clone(), spelled(u))).collect();
        assert_eq!(t[0], ("も".into(), "mo".into()));
        assert_eq!(t[2], ("会".into(), "a".into()), "{t:?}");
        assert_eq!(t[3], ("い".into(), "i".into()));
        let u = units("裏切られるんだ", Some(Script::Japanese));
        assert_eq!(spelled(&u[0]), "uragi", "{u:?}");
        let u = units("きっと", Some(Script::Japanese));
        let t: Vec<_> = u.iter().map(|u| (u.text.clone(), spelled(u))).collect();
        assert_eq!(t, vec![("き".into(), "ki".into()), ("っと".into(), "tto".into())]);
        let u = units("ジョーカー「ね」", Some(Script::Japanese));
        let t: Vec<_> = u.iter().map(|u| (u.text.clone(), spelled(u))).collect();
        assert_eq!(t[0], ("ジョー".into(), "joo".into()), "{t:?}");
        assert_eq!(u.iter().map(|u| u.text.as_str()).collect::<String>(), "ジョーカー「ね」");
    }

    #[test]
    fn chinese_and_korean() {
        let u = units("且看月", Some(Script::Chinese));
        assert_eq!(u.iter().map(spelled).collect::<Vec<_>>(), vec!["qie", "kan", "yue"]);
        let u = units("사랑해 요", Some(Script::Korean));
        assert_eq!(u.iter().map(spelled).collect::<Vec<_>>(), vec!["sa", "rang", "hae", "yo"]);
    }

    #[test]
    fn sounds_are_left_out() {
        assert!(vocalising(&units("La-la-la-la, ah-ah-ah", None)));
        assert!(!vocalising(&units("Oh, bersama alunan lagu", None)));
    }

    /// Emissions where token `tok` is clearly sung over the given frames, blank elsewhere.
    fn fake(frames: usize, sung: &[(usize, usize, u8)]) -> Emissions {
        let v = VOCAB.len();
        let mut logp = vec![-8.0f32; frames * v];
        for t in 0..frames {
            logp[t * v] = -0.01;
        }
        for &(a, b, tok) in sung {
            for t in a..b {
                logp[t * v] = -8.0;
                logp[t * v + tok as usize] = -0.01;
            }
        }
        Emissions { frames, logp }
    }

    #[test]
    fn finds_words_where_they_are_sung() {
        // "ab" sung at 1.0 s, "ba" at 3.0 s (letters a=1, b=18).
        let (a, b) = (token('a').unwrap(), token('b').unwrap());
        let em = fake(250, &[(50, 55, a), (55, 60, b), (150, 155, b), (155, 160, a)]);
        let texts = vec!["ab".to_string(), "".to_string(), "ba".to_string()];
        let out = align(&em, &texts, None).unwrap();
        assert!((out.lines[0].time - 1.0).abs() < 0.03, "{:?}", out.lines[0]);
        assert!((out.lines[2].time - 3.0).abs() < 0.03, "{:?}", out.lines[2]);
        // With LRC times 0.5 s early everywhere: the offset is found, the lines still land right.
        let out = align(&em, &texts, Some(&[0.5, 2.0, 2.5])).unwrap();
        assert!((out.offset - 0.5).abs() < 0.03);
        assert!((out.lines[2].time - 3.0).abs() < 0.03);
        assert_eq!(out.placed, 2);
    }
}
