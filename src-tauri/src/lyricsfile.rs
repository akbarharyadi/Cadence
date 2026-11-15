//! LRCLIB's word-by-word lyrics: its records carry a "Lyricsfile" (YAML) whose lines may hold
//! timed words (`lines[].words`, `hasWordSync: true`). Read into the same lines and words the
//! enhanced LRC and following the singer give, so the highlight follows each word without the
//! twenty seconds of listening (and the model download) alignment needs.
//!
//! Only what a Lyricsfile uses of YAML is read (block mappings and lists, plain and quoted
//! scalars, `|` blocks, `[]`); anything else is an error and the record's line-timed LRC is used,
//! as before.

use serde_json::{Map, Value};

use crate::lyrics::{Line, Word};

/// One line of the document: its indent, its text after the indent (None: blank or a comment).
struct Row<'a> {
    indent: usize,
    text: Option<&'a str>,
}

/// The YAML subset a Lyricsfile is written in, as JSON values.
pub fn parse_yaml(src: &str) -> Result<Value, String> {
    let rows: Vec<Row> = src
        .lines()
        .map(|l| {
            let l = l.trim_end_matches('\r');
            let body = l.trim_start_matches(' ');
            let indent = l.len() - body.len();
            let blank = body.trim().is_empty() || body.starts_with('#') || body == "---";
            Row { indent, text: (!blank).then_some(body.trim_end()) }
        })
        .collect();
    let mut p = Parser { rows, i: 0, depth: 0 };
    p.skip_blank();
    if p.i >= p.rows.len() {
        return Ok(Value::Null);
    }
    let indent = p.rows[p.i].indent;
    p.node(indent)
}

struct Parser<'a> {
    rows: Vec<Row<'a>>,
    i: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn skip_blank(&mut self) {
        while self.i < self.rows.len() && self.rows[self.i].text.is_none() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<(usize, &'a str)> {
        self.skip_blank();
        self.rows.get(self.i).and_then(|r| r.text.map(|t| (r.indent, t)))
    }

    fn node(&mut self, indent: usize) -> Result<Value, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("nested too deep".into());
        }
        let out = match self.peek() {
            Some((_, t)) if is_item(t) => self.list(indent),
            Some(_) => self.map(indent),
            None => Ok(Value::Null),
        };
        self.depth -= 1;
        out
    }

    fn list(&mut self, indent: usize) -> Result<Value, String> {
        let mut out = Vec::new();
        while let Some((ind, t)) = self.peek() {
            if ind != indent || !is_item(t) {
                if ind > indent {
                    return Err(format!("unexpected indent at line {}", self.i + 1));
                }
                break;
            }
            let rest = t[1..].trim_start();
            if rest.is_empty() {
                self.i += 1;
                match self.peek() {
                    Some((next, _)) if next > indent => out.push(self.node(next)?),
                    _ => out.push(Value::Null),
                }
            } else {
                // "- key: value": the rest of the line starts the item, as if on a line of its own.
                let at = indent + (t.len() - rest.len());
                self.rows[self.i] = Row { indent: at, text: Some(rest) };
                out.push(self.node(at)?);
            }
        }
        Ok(Value::Array(out))
    }

    fn map(&mut self, indent: usize) -> Result<Value, String> {
        let mut out = Map::new();
        while let Some((ind, t)) = self.peek() {
            if ind != indent || is_item(t) {
                if ind > indent {
                    return Err(format!("unexpected indent at line {}", self.i + 1));
                }
                break;
            }
            let (key, rest) = split_key(t).ok_or_else(|| format!("not a key at line {}", self.i + 1))?;
            self.i += 1;
            let value = if rest.is_empty() {
                match self.peek() {
                    Some((next, _)) if next > indent => self.node(next)?,
                    // A list at the key's own indent ("lines:\n- text: …").
                    Some((next, t)) if next == indent && is_item(t) => self.list(indent)?,
                    _ => Value::Null,
                }
            } else if rest.starts_with('|') || rest.starts_with('>') {
                self.block(indent, rest.starts_with('>'))
            } else {
                scalar(rest)?
            };
            out.insert(key, value);
        }
        Ok(Value::Object(out))
    }

    /// A `|` or `>` block: the lines indented under the key (blank lines kept).
    fn block(&mut self, indent: usize, folded: bool) -> Value {
        let mut lines: Vec<String> = Vec::new();
        let mut base: Option<usize> = None;
        while self.i < self.rows.len() {
            let r = &self.rows[self.i];
            match r.text {
                Some(t) if r.indent > indent => {
                    let b = *base.get_or_insert(r.indent);
                    lines.push(format!("{}{t}", " ".repeat(r.indent.saturating_sub(b))));
                }
                Some(_) => break,
                None => lines.push(String::new()),
            }
            self.i += 1;
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        Value::String(lines.join(if folded { " " } else { "\n" }))
    }
}

fn is_item(t: &str) -> bool {
    t == "-" || t.starts_with("- ")
}

/// `key: rest` (the key may be quoted); None when the line isn't one.
fn split_key(t: &str) -> Option<(String, &str)> {
    if let Some(q) = t.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let end = t[1..].find(q)? + 1;
        let key = t[1..end].to_string();
        let rest = t[end + 1..].trim_start().strip_prefix(':')?;
        return Some((key, rest.trim()));
    }
    let at = t.find(": ").or_else(|| t.ends_with(':').then(|| t.len() - 1))?;
    Some((t[..at].trim().to_string(), t[at + 1..].trim()))
}

fn scalar(s: &str) -> Result<Value, String> {
    let s = s.trim();
    if let Some(body) = s.strip_prefix('\'') {
        let end = closing_single(body).ok_or("unclosed quote")?;
        return Ok(Value::String(body[..end].replace("''", "'")));
    }
    if let Some(body) = s.strip_prefix('"') {
        return double_quoted(body).map(Value::String);
    }
    // A comment after a plain value.
    let s = match s.find(" #") {
        Some(i) => s[..i].trim_end(),
        None => s,
    };
    Ok(match s {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        "[]" => Value::Array(Vec::new()),
        "{}" => Value::Object(Map::new()),
        _ if s.starts_with('[') || s.starts_with('{') || s.starts_with('&') || s.starts_with('*') || s.starts_with('!') => {
            return Err(format!("unsupported value: {s}"));
        }
        _ => match s.parse::<i64>() {
            Ok(n) => Value::from(n),
            Err(_) => match s.parse::<f64>() {
                Ok(f) if f.is_finite() => Value::from(f),
                _ => Value::String(s.to_string()),
            },
        },
    })
}

fn closing_single(body: &str) -> Option<usize> {
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\'' {
            if b.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

fn double_quoted(body: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Ok(out),
            '\\' => match chars.next().ok_or("unclosed quote")? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                ' ' => out.push(' '),
                '/' => out.push('/'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'N' => out.push('\u{85}'),
                '_' => out.push('\u{a0}'),
                e @ ('x' | 'u' | 'U') => {
                    let n = match e {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let hex: String = chars.by_ref().take(n).collect();
                    let c = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).ok_or("bad escape")?;
                    out.push(c);
                }
                other => return Err(format!("unknown escape \\{other}")),
            },
            c => out.push(c),
        }
    }
    Err("unclosed quote".into())
}

/// Milliseconds as seconds (None for anything that isn't a sensible time).
fn secs(v: &Value) -> Option<f64> {
    let ms = v.as_f64()?;
    (ms.is_finite() && ms >= 0.0 && ms < 36_000_000.0).then_some(ms / 1000.0)
}

/// The word-timed lines of a Lyricsfile; None when it has no word timings or can't be read (the
/// caller then uses the record's line-timed LRC).
pub fn word_lines(src: &str) -> Option<Vec<Line>> {
    let doc = parse_yaml(src).map_err(|e| log::info!("Lyricsfile not read: {e}")).ok()?;
    // A positive offset shows the lyrics earlier (as LRC's [offset:]).
    let offset = doc["metadata"]["offset_ms"].as_f64().filter(|o| o.is_finite()).unwrap_or(0.0) / 1000.0;
    let mut lines = Vec::new();
    let mut timed_words = 0;
    for l in doc["lines"].as_array()? {
        let mut words: Vec<Word> = l["words"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| {
                Some(Word {
                    time: (secs(&w["start_ms"])? - offset).max(0.0),
                    text: w["text"].as_str()?.to_string(),
                    end: secs(&w["end_ms"]).map(|e| (e - offset).max(0.0)),
                })
            })
            .collect();
        let all_words = l["words"].as_array().map_or(0, Vec::len);
        // A word without its time: the line is kept, without word timings.
        if words.len() != all_words {
            words.clear();
        }
        let text = if words.is_empty() {
            l["text"].as_str().unwrap_or("").trim().to_string()
        } else {
            words.iter().map(|w| w.text.as_str()).collect::<String>().trim().to_string()
        };
        let Some(time) = secs(&l["start_ms"]).or_else(|| words.first().map(|w| w.time + offset)) else { continue };
        words.retain(|w| !w.text.trim().is_empty());
        // Words out of order can't be swept: the line keeps its own time only.
        if words.windows(2).any(|p| p[1].time + 0.001 < p[0].time) {
            words.clear();
        }
        timed_words += words.len();
        lines.push(Line { time: (time - offset).max(0.0), text, words, singers: Vec::new() });
    }
    if timed_words == 0 || lines.iter().all(|l| l.text.is_empty()) {
        return None;
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As LRCLIB's server writes one (serde_yaml: lists at their key's indent).
    // Invented text, title and artist (no real lyrics in fixtures).
    const SERVER: &str = "version: '1.0'\nmetadata:\n  title: Test Song\n  artist: Example Artist\n  album: Example's Album\n  duration_ms: 233000\n  instrumental: false\nlines:\n- text: the rain is falling\n  start_ms: 1000\n  end_ms: 2400\n  words:\n  - text: 'the rain '\n    start_ms: 1000\n    end_ms: 1500\n  - text: is falling\n    start_ms: 1500\n    end_ms: 2400\n- text: ''\n  start_ms: 3000\n- text: \"今日は雨が \\\"Test\\\"\"\n  start_ms: 4000\n  words:\n  - text: 今日は\n    start_ms: 4000\n  - text: \"雨が \"\n    start_ms: 4600\n  - text: '\"Test\"'\n    start_ms: 5200\n    end_ms: 6000\nplain: |\n  the rain is falling\n\n  今日は雨が\n";

    /// As a person writes one (lists indented under their key, comments).
    const HANDWRITTEN: &str = "# made by hand\nmetadata:\n  instrumental: false\n  offset_ms: 500\nlines:\n  - start_ms: 10500\n    text: Line one\n    words:\n      - text: \"Line \"\n        start_ms: 10500\n      - text: one   # sung long\n        start_ms: 11000\n        end_ms: 12500\n  - start_ms: 20000\n    text: No words here\n    words: []\n";

    #[test]
    fn reads_the_servers_lyricsfile() {
        let lines = word_lines(SERVER).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "the rain is falling");
        assert_eq!(lines[0].time, 1.0);
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[0], Word { time: 1.0, text: "the rain ".into(), end: Some(1.5) });
        assert_eq!(lines[0].words[1].end, Some(2.4));
        // An empty line (a pause) keeps its time.
        assert_eq!(lines[1].text, "");
        assert!(lines[1].words.is_empty());
        assert_eq!(lines[2].text, "今日は雨が \"Test\"");
        assert_eq!(lines[2].words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["今日は", "雨が ", "\"Test\""]);
        assert_eq!(lines[2].words[1].end, None);
    }

    #[test]
    fn reads_a_handwritten_one_with_its_offset() {
        let lines = word_lines(HANDWRITTEN).unwrap();
        assert_eq!(lines.len(), 2);
        // offset 500 ms: everything half a second earlier.
        assert_eq!(lines[0].time, 10.0);
        assert_eq!(lines[0].words[1].text, "one");
        assert_eq!(lines[0].words[1].end, Some(12.0));
        assert!(lines[1].words.is_empty());
        assert_eq!(lines[1].text, "No words here");
    }

    #[test]
    fn line_timed_or_broken_files_give_nothing() {
        // Line timings only: nothing to add to the LRC.
        assert!(word_lines("lines:\n- text: a line\n  start_ms: 10\n  end_ms: 20\n").is_none());
        // Not YAML the reader knows: silently the LRC.
        assert!(word_lines("lines: [ {text: a} ]").is_none());
        assert!(word_lines("lines:\n- text: \"unclosed\n").is_none());
        assert!(word_lines("").is_none());
        // A word without a time drops that line's words, not the line.
        let l = word_lines("lines:\n- text: a b\n  start_ms: 1000\n  words:\n  - text: 'a '\n    start_ms: 1000\n  - text: b\n- text: c\n  start_ms: 2000\n  words:\n  - text: c\n    start_ms: 2000\n").unwrap();
        assert!(l[0].words.is_empty() && l[0].text == "a b");
        assert_eq!(l[1].words.len(), 1);
    }

    #[test]
    fn yaml_scalars() {
        let v = parse_yaml("a: 'it''s'\nb: \"tab\\there \\u00e9\"\nc: 12\nd: -1.5\ne: ~\nf: true\ng: plain text # note\nh: []\n").unwrap();
        assert_eq!(v["a"], "it's");
        assert_eq!(v["b"], "tab\there é");
        assert_eq!(v["c"], 12);
        assert_eq!(v["d"], -1.5);
        assert!(v["e"].is_null());
        assert_eq!(v["f"], true);
        assert_eq!(v["g"], "plain text");
        assert_eq!(v["h"], Value::Array(vec![]));
        let v = parse_yaml(SERVER).unwrap();
        assert_eq!(v["plain"], "the rain is falling\n\n今日は雨が");
        assert_eq!(v["metadata"]["album"], "Example's Album");
    }
}
