//! Playlist import and export.
//!
//! Export: M3U8 (UTF-8, no BOM, `#EXTM3U`, `#PLAYLIST`, one `#EXTINF` per song and its path) and CSV
//! (UTF-8 with a BOM, RFC 4180 quoting). Import: .m3u/.m3u8, .csv (this app's own export and
//! Exportify's Spotify exports) and .xspf, each entry matched to a library song without guessing:
//!
//! 1. its file: the exact path, then relative to the playlist file's folder, then the last 2-3 path
//!    components when exactly one library file ends the same way (a playlist from another machine);
//! 2. its tags: the same title (also without "(feat. X)", "(Remastered 2011)" and the like when both
//!    are the same kind of recording), the same artist across spellings and scripts (`identity`), and
//!    the same length within 3 seconds when the entry has one;
//! 3. otherwise it's not found, and when two different songs fit equally well it's ambiguous. An
//!    entry is never attached to a song it only might be.
//!
//! Everything here is a pure function of the text and the library snapshot (the `Ctx` glue is at the
//! end), so it's tested without the app.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::identity::Identity;
use crate::library::Track;
use crate::state::match_key;

/// A playlist file larger than this is something else (or made to fill the memory).
pub const MAX_FILE: u64 = 64 << 20;
const MAX_ENTRIES: usize = 500_000;
/// An entry's length may differ from the library song's by this much (a rip's silence, a store's
/// rounding): a little more than the 2.5 s `state::Finder` allows, since the lengths come from
/// another service.
const SAME_LENGTH: f64 = 3.0;
/// Songs this close in length are one recording (the same song on a single and on its album).
const SAME_RECORDING: f64 = 1.0;
/// Unmatched entries the preview lists (the missing list file has all of them).
const SHOWN_MISSES: usize = 1000;
/// The "remaster" kind of `autotag::version_kinds`: a remaster is the same recording for matching.
const REMASTER: u16 = 1 << 7;

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportOptions {
    /// "m3u8" or "csv".
    pub format: String,
    /// Paths relative to the playlist file's folder (absolute for files on another drive).
    pub relative: bool,
    /// "/" in paths (else the library's own style).
    pub forward_slashes: bool,
    /// CRLF line ends in an M3U8 (some devices need them); LF otherwise.
    pub crlf: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { format: "m3u8".into(), relative: false, forward_slashes: false, crlf: false }
    }
}

impl ExportOptions {
    pub fn extension(&self) -> Result<&'static str, String> {
        match self.format.to_ascii_lowercase().as_str() {
            "m3u8" => Ok("m3u8"),
            "csv" => Ok("csv"),
            other => Err(format!("Can't export as “{other}”: choose M3U8 or CSV.")),
        }
    }
}

/// A file name for a playlist: no character Windows, macOS, Linux or Android refuse, no reserved
/// device name, not empty, not endless.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let mut s = cleaned.trim().trim_end_matches(['.', ' ']).trim_start_matches('.').trim().to_string();
    if s.chars().count() > 100 {
        s = s.chars().take(100).collect::<String>().trim_end().to_string();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0');
    if s.is_empty() {
        return "Playlist".into();
    }
    if reserved {
        s.insert(0, '_');
    }
    s
}

/// One line of text: line breaks become spaces.
fn one_line(s: &str) -> String {
    s.split(['\r', '\n']).map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

fn drive(c: &str) -> bool {
    let b = c.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A path as its parts, "." and ".." resolved; and whether it is absolute (a root, a drive, UNC).
/// `fold`: lower-case and composed (NFC), for comparing; Windows' extended-length prefix is dropped.
fn clean(path: &str, fold: bool) -> (bool, Vec<String>) {
    use unicode_normalization::UnicodeNormalization;
    let mut p = path.trim().replace('\\', "/");
    if let Some(rest) = p.strip_prefix("//?/UNC/") {
        p = format!("//{rest}");
    } else if let Some(rest) = p.strip_prefix("//?/") {
        p = rest.to_string();
    }
    if fold {
        p = p.nfc().collect::<String>().to_lowercase();
    }
    let rooted = p.starts_with('/');
    let mut parts: Vec<String> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| l != ".." && !(parts.len() == 1 && drive(l))) {
                    parts.pop();
                } else if !rooted && !parts.first().is_some_and(|f| drive(f)) {
                    parts.push("..".into());
                }
            }
            c => parts.push(c.to_string()),
        }
    }
    let absolute = rooted || parts.first().is_some_and(|f| drive(f));
    (absolute, parts)
}

/// The comparison key of an absolute path (a drive path has no leading slash).
fn key_of(absolute: bool, parts: &[String]) -> String {
    let joined = parts.join("/");
    if absolute && !parts.first().is_some_and(|f| drive(f)) { format!("/{joined}") } else { joined }
}

/// `track` relative to the folder `base` (with "/" and ".."), None when they share no root (another
/// drive) so an absolute path is used.
fn relative_to(track: &str, base: &str) -> Option<String> {
    let (ta, t) = clean(track, false);
    let (ba, b) = clean(base, false);
    if !ta || !ba {
        return None;
    }
    let same = |x: &str, y: &str| if drive(x) || drive(y) { x.eq_ignore_ascii_case(y) } else { x == y };
    let common = t.iter().zip(&b).take_while(|(x, y)| same(x, y)).count();
    // A drive (or the first folder of a UNC share) in common is needed, else there's no way across.
    if common == 0 && (t.first().is_some_and(|f| drive(f)) || b.first().is_some_and(|f| drive(f))) {
        return None;
    }
    if t.len() <= common {
        return None;
    }
    let mut out: Vec<&str> = vec![".."; b.len() - common];
    out.extend(t[common..].iter().map(String::as_str));
    Some(out.join("/"))
}

/// How a song's path is written in an export.
fn export_path(track_path: &str, base: Option<&str>, o: &ExportOptions) -> String {
    let own = track_path.strip_prefix(r"\\?\").unwrap_or(track_path);
    let backslashes = own.contains('\\');
    let path = o.relative.then(|| base.and_then(|b| relative_to(own, b))).flatten().unwrap_or_else(|| own.to_string());
    if o.forward_slashes {
        path.replace('\\', "/")
    } else if backslashes {
        path.replace('/', "\\")
    } else {
        path
    }
}

/// A file name that says nothing of its song: a track number ("01.flac", "1-03.flac",
/// "Track 07.mp3") or a few characters.
fn generic_name(name: &str) -> bool {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let lower = stem.to_lowercase();
    let words: String = lower.replace("track", "").chars().filter(|c| c.is_alphanumeric() && !c.is_ascii_digit()).collect();
    words.is_empty() || stem.chars().filter(|c| c.is_alphanumeric()).count() < 4
}

fn file_stem(path: &str) -> String {
    let p = path.replace('\\', "/");
    let name = p.rsplit('/').next().unwrap_or("");
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_string()
}

fn extinf_text(t: &Track) -> String {
    let artist = one_line(&t.artist);
    let title = one_line(&t.title);
    let title = if title.is_empty() { file_stem(&t.path) } else { title };
    if artist.is_empty() { title } else { format!("{artist} - {title}") }
}

pub struct Rendered {
    pub bytes: Vec<u8>,
    /// Songs in the file whose audio shares its file with other songs (a CUE-sheet album).
    pub cue: usize,
}

/// The file's content. `base`: the folder the file is saved in (for relative paths).
pub fn render(name: &str, tracks: &[Track], base: Option<&str>, o: &ExportOptions) -> Result<Rendered, String> {
    let cue = tracks.iter().filter(|t| t.cue.is_some()).count();
    let bytes = match o.extension()? {
        "csv" => render_csv(tracks, base, o),
        _ => render_m3u8(name, tracks, base, o),
    };
    Ok(Rendered { bytes, cue })
}

fn render_m3u8(name: &str, tracks: &[Track], base: Option<&str>, o: &ExportOptions) -> Vec<u8> {
    let eol = if o.crlf { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(64 + tracks.len() * 160);
    out.push_str("#EXTM3U");
    out.push_str(eol);
    let name = one_line(name);
    if !name.is_empty() {
        out.push_str("#PLAYLIST:");
        out.push_str(&name);
        out.push_str(eol);
    }
    for t in tracks {
        let secs = if t.duration > 0.0 { t.duration.round() as i64 } else { -1 };
        out.push_str(&format!("#EXTINF:{secs},{}", extinf_text(t)));
        out.push_str(eol);
        out.push_str(&one_line(&export_path(&t.path, base, o)));
        out.push_str(eol);
    }
    out.into_bytes()
}

/// RFC 4180: a field with a comma, a quote, a line break or spaces at its ends is quoted.
pub(crate) fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) || s.starts_with(' ') || s.ends_with(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

const CSV_HEADER: [&str; 8] = ["Title", "Artist", "Album", "Album artist", "Duration (seconds)", "Track number", "Year", "Path"];

fn render_csv(tracks: &[Track], base: Option<&str>, o: &ExportOptions) -> Vec<u8> {
    let mut out = String::with_capacity(64 + tracks.len() * 200);
    out.push_str(&CSV_HEADER.join(","));
    out.push_str("\r\n");
    for t in tracks {
        let row = [
            t.title.clone(),
            t.artist.clone(),
            t.album.clone(),
            t.album_artist.clone(),
            if t.duration > 0.0 { format!("{}", t.duration.round() as i64) } else { String::new() },
            t.track_no.map(|n| n.to_string()).unwrap_or_default(),
            t.year.map(|n| n.to_string()).unwrap_or_default(),
            export_path(&t.path, base, o),
        ];
        out.push_str(&row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(out.as_bytes());
    bytes
}

// ---------------------------------------------------------------------------------------------
// Reading files
// ---------------------------------------------------------------------------------------------

/// The text of a playlist file's bytes: UTF-8 (a BOM dropped), UTF-16 with a BOM (Notepad's
/// "Unicode"), else the legacy single-byte code page (Windows-1252, which is Latin-1 plus the
/// punctuation Windows put in 0x80-0x9F) that an old .m3u was saved in.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(bytes).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    M3u,
    Csv,
    Xspf,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::M3u => "M3U",
            Kind::Csv => "CSV",
            Kind::Xspf => "XSPF",
        }
    }
}

/// What a file is, from its extension (and, for a name that says nothing, its first line).
pub fn kind_of(path: &str, text: &str) -> Result<Kind, String> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "m3u" | "m3u8" => Ok(Kind::M3u),
        "csv" => Ok(Kind::Csv),
        "xspf" => Ok(Kind::Xspf),
        _ => {
            let head = text.trim_start().get(..40).unwrap_or(text.trim_start()).to_ascii_lowercase();
            if head.starts_with("#extm3u") {
                Ok(Kind::M3u)
            } else if head.starts_with("<?xml") || head.starts_with("<playlist") {
                Ok(Kind::Xspf)
            } else {
                Err("Cadence can import .m3u, .m3u8, .csv and .xspf playlists.".into())
            }
        }
    }
}

/// One line of a playlist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    /// As written (a path, a `file://` address, a web address); may be empty (a CSV without paths).
    pub path: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: Option<f64>,
    pub isrc: String,
    /// An M3U's "Artist - Title" line, not split yet (see `splits`).
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub name: Option<String>,
    pub entries: Vec<Entry>,
}

fn strip_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let head = line.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &line[prefix.len()..])
}

pub fn parse_m3u(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut pending = Entry::default();
    for raw in text.split(['\r', '\n']) {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            let c = c.trim_start();
            if let Some(rest) = strip_ci(c, "EXTINF:") {
                let (left, right) = rest.split_once(',').unwrap_or((rest, ""));
                pending.duration = left.split_whitespace().next().and_then(|d| d.parse::<f64>().ok()).filter(|d| *d > 0.0);
                pending.text = right.trim().to_string();
            } else if let Some(rest) = strip_ci(c, "PLAYLIST:") {
                if out.name.is_none() && !rest.trim().is_empty() {
                    out.name = Some(rest.trim().to_string());
                }
            } else if let Some(rest) = strip_ci(c, "EXTALB:") {
                pending.album = rest.trim().to_string();
            } else if let Some(rest) = strip_ci(c, "EXTART:") {
                pending.artist = rest.trim().to_string();
            }
            continue;
        }
        if out.entries.len() >= MAX_ENTRIES {
            break;
        }
        let mut e = std::mem::take(&mut pending);
        e.path = line.to_string();
        // A known artist (#EXTART) makes the whole text the title.
        if !e.artist.is_empty() && !e.text.is_empty() {
            e.title = std::mem::take(&mut e.text);
        }
        out.entries.push(e);
    }
    out
}

/// The rows of a CSV: quotes, doubled quotes and line breaks inside quotes; the separator is the
/// header line's most frequent of comma, semicolon and tab.
pub fn parse_csv_rows(text: &str) -> Vec<Vec<String>> {
    let text = text.trim_start_matches('\u{feff}');
    let first = text.split(['\r', '\n']).next().unwrap_or("");
    let delimiter = {
        let (mut quoted, mut counts) = (false, [0usize; 3]);
        for c in first.chars() {
            match c {
                '"' => quoted = !quoted,
                ',' if !quoted => counts[0] += 1,
                ';' if !quoted => counts[1] += 1,
                '\t' if !quoted => counts[2] += 1,
                _ => {}
            }
        }
        let best = counts.iter().enumerate().max_by_key(|(i, n)| (**n, usize::MAX - i)).map(|(i, _)| i).unwrap_or(0);
        [',', ';', '\t'][best]
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut any = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                quoted = true;
                any = true;
            }
            c if c == delimiter => {
                row.push(std::mem::take(&mut field));
                any = true;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                if any || !field.is_empty() {
                    row.push(std::mem::take(&mut field));
                    if row.iter().any(|f| !f.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                any = false;
                if rows.len() > MAX_ENTRIES {
                    return rows;
                }
            }
            c => {
                field.push(c);
                any = true;
            }
        }
    }
    if any || !field.is_empty() {
        row.push(field);
        if row.iter().any(|f| !f.trim().is_empty()) {
            rows.push(row);
        }
    }
    rows
}

fn header_key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Ms,
    Seconds,
    /// "Duration": milliseconds when it's a big number, "3:45" or seconds otherwise.
    Guess,
}

/// "3:45", "1:02:03", "215", "215.4" (seconds) or a count of milliseconds.
fn parse_duration(value: &str, unit: Unit) -> Option<f64> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let secs = if v.contains(':') {
        v.split(':').try_fold(0.0, |acc, p| p.trim().parse::<f64>().ok().map(|n| acc * 60.0 + n))?
    } else {
        let n: f64 = v.replace(',', ".").parse().ok()?;
        match unit {
            Unit::Ms => n / 1000.0,
            Unit::Seconds => n,
            Unit::Guess if n.fract() == 0.0 && n > 20_000.0 => n / 1000.0,
            Unit::Guess => n,
        }
    };
    (secs.is_finite() && secs > 0.0).then_some(secs)
}

pub fn parse_csv(text: &str) -> Result<Parsed, String> {
    let rows = parse_csv_rows(text);
    let Some(head) = rows.first() else { return Err("This CSV file is empty.".into()) };
    let keys: Vec<String> = head.iter().map(|h| header_key(h)).collect();
    let find = |names: &[&str]| keys.iter().position(|k| names.contains(&k.as_str()));
    let title = find(&["title", "trackname", "track", "name", "song", "songname", "tracktitle"]);
    let Some(title) = title else {
        return Err("This CSV has no title column. Expected a header such as “Title” or “Track Name”.".into());
    };
    let artist = find(&["artist", "artists", "artistname", "artistnames", "trackartist", "trackartists", "performer"]);
    let album = find(&["album", "albumname"]);
    let isrc = find(&["isrc"]);
    let path = find(&["path", "location", "filepath", "file", "filename"]);
    let (duration, unit) = if let Some(i) = find(&["durationms", "lengthms", "trackdurationms"]) {
        (Some(i), Unit::Ms)
    } else if let Some(i) = find(&["durationseconds", "durationsec", "durations", "seconds", "lengthseconds"]) {
        (Some(i), Unit::Seconds)
    } else {
        (find(&["duration", "length", "time"]), Unit::Guess)
    };
    let get = |row: &[String], i: Option<usize>| i.and_then(|i| row.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut out = Parsed::default();
    for row in rows.iter().skip(1).take(MAX_ENTRIES) {
        let e = Entry {
            path: get(row, path),
            artist: get(row, artist),
            title: get(row, Some(title)),
            album: get(row, album),
            duration: duration.and_then(|i| row.get(i)).and_then(|v| parse_duration(v, unit)),
            isrc: get(row, isrc),
            text: String::new(),
        };
        if e.title.is_empty() && e.path.is_empty() {
            continue;
        }
        out.entries.push(e);
    }
    Ok(out)
}

fn xml_text(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match tail.find(';').filter(|e| *e <= 10) {
            Some(end) => {
                let ent = &tail[1..end];
                let ch = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => ent
                        .strip_prefix("#x")
                        .or_else(|| ent.strip_prefix("#X"))
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                match ch {
                    Some(c) => {
                        out.push(c);
                        rest = &tail[end + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &tail[1..];
                    }
                }
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text of the first `<tag>…</tag>` in `s`.
fn xml_tag(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(i) = s[from..].find(&open) {
        let at = from + i + open.len();
        match s.as_bytes().get(at) {
            Some(b'>') | Some(b' ') | Some(b'/') | Some(b'\t') | Some(b'\r') | Some(b'\n') => {
                let gt = s[at..].find('>')? + at;
                if s.as_bytes()[gt - 1] == b'/' {
                    return Some(String::new());
                }
                let end = s[gt + 1..].find(&format!("</{tag}>"))? + gt + 1;
                return Some(xml_text(&s[gt + 1..end]));
            }
            _ => from = at,
        }
    }
    None
}

pub fn parse_xspf(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let list_at = text.find("<trackList").unwrap_or(text.len());
    out.name = xml_tag(&text[..list_at], "title").filter(|t| !t.is_empty());
    let mut rest = &text[list_at..];
    while let Some(i) = rest.find("<track>").or_else(|| rest.find("<track ")) {
        let body_start = i + 6;
        let end = rest[body_start..].find("</track>").map_or(rest.len(), |e| e + body_start);
        let body = &rest[body_start..end];
        let mut e = Entry {
            path: xml_tag(body, "location").unwrap_or_default(),
            title: xml_tag(body, "title").unwrap_or_default(),
            artist: xml_tag(body, "creator").unwrap_or_default(),
            album: xml_tag(body, "album").unwrap_or_default(),
            duration: xml_tag(body, "duration").and_then(|d| parse_duration(&d, Unit::Ms)),
            ..Default::default()
        };
        e.path = e.path.trim().to_string();
        if !(e.title.is_empty() && e.path.is_empty()) {
            out.entries.push(e);
        }
        if out.entries.len() >= MAX_ENTRIES {
            break;
        }
        rest = &rest[(end + 8).min(rest.len())..];
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------------------------

/// A `file://` address as a path ("file:///D:/Music/a%20b.flac" → "D:/Music/a b.flac").
fn from_url(s: &str) -> String {
    let Some(head) = s.get(..7).filter(|h| h.eq_ignore_ascii_case("file://")) else { return s.to_string() };
    let mut rest = &s[head.len()..];
    if let Some(r) = rest.get(..10).filter(|h| h.eq_ignore_ascii_case("localhost/")).map(|_| &rest[9..]) {
        rest = r;
    }
    let decoded = urlencoding::decode(rest).map(|c| c.into_owned()).unwrap_or_else(|_| rest.to_string());
    let b = decoded.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return decoded[1..].to_string();
    }
    decoded
}

/// The version words other than "remaster" in a title: "(Live)", "- Instrumental", "(Remix)"…
fn version_kinds(s: &str) -> u16 {
    crate::autotag::version_kinds(s) & !REMASTER
}

/// A title cut at " feat. X" (when not in brackets, which `core_title` cuts).
fn without_feat(title: &str) -> &str {
    let lower = title.to_ascii_lowercase();
    let cut = [" feat. ", " feat ", " ft. ", " featuring ", " feat.", " ft."].iter().filter_map(|m| lower.find(m)).min();
    &title[..cut.unwrap_or(title.len())]
}

/// A title without "(feat. …)", "[Remastered]", " - Live" and the like, letters and digits only:
/// `catalog::core_title`, after a "feat." that is not in brackets.
fn core(title: &str) -> String {
    crate::catalog::core_title(without_feat(title))
}

fn lossless(codec: &str) -> bool {
    ["FLAC", "ALAC", "WAV", "AIFF", "PCM", "APE", "WV"].iter().any(|c| codec.eq_ignore_ascii_case(c))
}

/// The copy to use among songs that are one recording: lossless, higher bit depth and sample rate,
/// then the earliest year, then the path (so the choice never depends on the library's order).
fn best<'a>(cands: &[&'a Track]) -> &'a Track {
    cands
        .iter()
        .copied()
        .min_by(|a, b| {
            let key = |t: &Track| (!lossless(&t.codec), std::cmp::Reverse(t.bit_depth.unwrap_or(0)), std::cmp::Reverse(t.sample_rate.unwrap_or(0)), t.year.unwrap_or(u32::MAX));
            key(a).cmp(&key(b)).then_with(|| a.path.cmp(&b.path)).then_with(|| a.id.cmp(&b.id))
        })
        .expect("at least one song")
}

/// The library, indexed once per import.
pub struct Index<'a> {
    by_path: HashMap<String, Vec<&'a Track>>,
    by_suffix2: HashMap<String, Vec<&'a Track>>,
    by_suffix3: HashMap<String, Vec<&'a Track>>,
    by_title: HashMap<String, Vec<&'a Track>>,
    by_core: HashMap<String, Vec<&'a Track>>,
}

impl<'a> Index<'a> {
    pub fn new(tracks: &'a [Track]) -> Self {
        let mut ix = Index { by_path: HashMap::new(), by_suffix2: HashMap::new(), by_suffix3: HashMap::new(), by_title: HashMap::new(), by_core: HashMap::new() };
        for t in tracks {
            if !t.path.is_empty() {
                let (abs, parts) = clean(&t.path, true);
                let n = parts.len();
                ix.by_path.entry(key_of(abs, &parts)).or_default().push(t);
                if n >= 2 {
                    ix.by_suffix2.entry(parts[n - 2..].join("/")).or_default().push(t);
                }
                if n >= 3 {
                    ix.by_suffix3.entry(parts[n - 3..].join("/")).or_default().push(t);
                }
            }
            let k = match_key(&t.title);
            if !k.is_empty() {
                ix.by_title.entry(k.clone()).or_default().push(t);
            }
            // The catalogue's title of a song kept under its own (the subtitle tag): either finds it.
            let sub = match_key(&t.subtitle);
            if !sub.is_empty() && sub != k {
                ix.by_title.entry(sub).or_default().push(t);
            }
            let c = core(&t.title);
            if !c.is_empty() {
                ix.by_core.entry(c).or_default().push(t);
            }
        }
        ix
    }
}

/// What the matching of one entry came to.
#[derive(Debug)]
pub enum Outcome<'a> {
    Matched(&'a Track),
    NotFound(String),
    Ambiguous(String),
}

/// The ways an "Artist - Title" text can be read, the first " - " first, and the whole text as a
/// title (no artist) last: "A - B - C" is A / "B - C", then "A - B" / C, then "A - B - C".
fn splits(text: &str) -> Vec<(String, String)> {
    let t = text.trim();
    let mut at: Vec<(usize, usize)> = [" - ", " – ", " — "].iter().flat_map(|s| t.match_indices(s).map(|(i, m)| (i, m.len()))).collect();
    at.sort();
    let mut out: Vec<(String, String)> = at
        .into_iter()
        .filter_map(|(i, n)| {
            let (l, r) = (t[..i].trim(), t[i + n..].trim());
            (!l.is_empty() && !r.is_empty()).then(|| (l.to_string(), r.to_string()))
        })
        .collect();
    if !t.is_empty() {
        out.push((String::new(), t.to_string()));
    }
    out
}

/// The (artist, title) readings of an entry; for a path with nothing else, its file name's.
fn readings(e: &Entry) -> Vec<(String, String)> {
    if !e.title.is_empty() {
        return vec![(e.artist.clone(), e.title.clone())];
    }
    if !e.text.is_empty() {
        return splits(&e.text);
    }
    if e.path.is_empty() { Vec::new() } else { splits(&file_stem(&from_url(&e.path))) }
}

fn clock(secs: f64) -> String {
    let s = secs.round().max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The result of reading an entry's tags against the library.
enum Tag<'a> {
    One(&'a Track),
    Ambiguous(usize),
    /// Found, but only with another length: the closest one's.
    Length(f64),
    /// The title is in the library, by other artists.
    OtherArtist,
    /// Only a title, and nothing to check it against.
    NoArtist,
    None,
}

impl<'a> Index<'a> {
    fn artist_ok(who: &Identity, artist: &str, t: &Track) -> bool {
        let credit = if t.artist.trim().is_empty() { &t.album_artist } else { &t.artist };
        who.same_artist(artist, credit)
    }

    /// Several songs fit: the one on the entry's album, else, when they're all one recording, the
    /// best copy; else it's ambiguous.
    fn choose(cands: &[&'a Track], e: &Entry) -> Tag<'a> {
        if let [one] = cands {
            return Tag::One(*one);
        }
        let mut pool: Vec<&Track> = cands.to_vec();
        if !e.album.is_empty() {
            let (k, c) = (match_key(&e.album), core(&e.album));
            let on: Vec<&Track> = pool.iter().copied().filter(|t| match_key(&t.album) == k || (!c.is_empty() && core(&t.album) == c)).collect();
            match on.as_slice() {
                [] => {}
                [one] => return Tag::One(*one),
                _ => pool = on,
            }
        }
        let (lo, hi) = pool.iter().fold((f64::MAX, f64::MIN), |(lo, hi), t| (lo.min(t.duration), hi.max(t.duration)));
        if hi - lo <= SAME_RECORDING { Tag::One(best(&pool)) } else { Tag::Ambiguous(pool.len()) }
    }

    fn by_tags(&self, e: &Entry, who: &Identity, only: Option<&HashSet<&str>>) -> Tag<'a> {
        let mut closest: Option<f64> = None;
        let mut title_seen = false;
        let mut needs_artist = false;
        for (artist, title) in readings(e) {
            let key = match_key(&title);
            if key.is_empty() {
                continue;
            }
            let entry_core = core(&title);
            let title_only = artist.trim().is_empty();
            for exact in [true, false] {
                let pool: &[&Track] = (if exact { self.by_title.get(&key) } else { self.by_core.get(&entry_core) }).map_or(&[][..], |v| v.as_slice());
                let mut found: Vec<&Track> = pool
                    .iter()
                    .copied()
                    .filter(|t| only.is_none_or(|o| o.contains(t.id.as_str())))
                    // "Song" is "Song (Remastered 2011)" and "Song (feat. X)", not "Song (Live)".
                    .filter(|t| exact || (!entry_core.is_empty() && version_kinds(&title) == version_kinds(&t.title)))
                    .collect();
                if found.is_empty() {
                    continue;
                }
                title_seen = true;
                if title_only {
                    // A title alone proves little: only its length, and only one song of that length.
                    let Some(d) = e.duration else {
                        needs_artist = true;
                        continue;
                    };
                    found.retain(|t| (t.duration - d).abs() <= SAME_LENGTH);
                    match found.as_slice() {
                        [] => {}
                        [one] => return Tag::One(*one),
                        more => return Tag::Ambiguous(more.len()),
                    }
                    continue;
                }
                found.retain(|t| Self::artist_ok(who, &artist, t));
                if found.is_empty() {
                    continue;
                }
                if let Some(d) = e.duration {
                    let gap = |t: &&Track| (t.duration - d).abs();
                    let have = found.iter().min_by(|a, b| gap(a).total_cmp(&gap(b))).map(|t| t.duration);
                    found.retain(|t| (t.duration - d).abs() <= SAME_LENGTH);
                    if found.is_empty() {
                        closest = closest.or(have);
                        continue;
                    }
                }
                return Self::choose(&found, e);
            }
        }
        match (closest, title_seen, needs_artist) {
            (Some(have), ..) => Tag::Length(have),
            (None, _, true) => Tag::NoArtist,
            (None, true, _) => Tag::OtherArtist,
            _ => Tag::None,
        }
    }

    /// The one song at a file: the candidates all share one file when they are a CUE album's songs
    /// (the entry's title, then its length, say which); `Err(n)` when `n` songs fit and none is the one.
    fn one_file(cands: &[&'a Track], e: &Entry, raw: &str) -> Result<&'a Track, usize> {
        let exact = |p: &str| {
            let (a, parts) = clean(p, false);
            key_of(a, &parts)
        };
        let mut pool: Vec<&'a Track> = cands.to_vec();
        let mut files: Vec<String> = pool.iter().map(|t| exact(&t.path)).collect();
        files.sort();
        files.dedup();
        if files.len() > 1 {
            // Several files: only one spelled exactly like the entry's path (they differ in letter
            // case) is the one; an entry from another machine can't choose.
            let want = exact(raw);
            pool.retain(|t| exact(&t.path) == want);
            if pool.is_empty() {
                return Err(cands.len());
            }
        }
        if let [one] = pool.as_slice() {
            return Ok(*one);
        }
        let titles: Vec<String> = readings(e).into_iter().map(|(_, t)| match_key(&t)).filter(|k| !k.is_empty()).collect();
        let by_title: Vec<&'a Track> = pool.iter().copied().filter(|t| titles.contains(&match_key(&t.title))).collect();
        let mut narrowed = if by_title.is_empty() { pool.clone() } else { by_title };
        if let Some(d) = e.duration {
            let near: Vec<&'a Track> = narrowed.iter().copied().filter(|t| (t.duration - d).abs() <= SAME_LENGTH).collect();
            if !near.is_empty() || narrowed.len() > 1 {
                narrowed = near;
            }
        }
        match narrowed.as_slice() {
            [one] => Ok(*one),
            [] => Err(pool.len()),
            more => Err(more.len()),
        }
    }

    /// The library song at the entry's path: exact, then relative to the playlist's folder, then by
    /// its last 2-3 components. `Ok(None)` when none; `Err(candidates)` when several files fit.
    fn by_file(&self, e: &Entry, base: Option<&str>) -> Result<Option<&'a Track>, Vec<&'a Track>> {
        let raw = from_url(e.path.trim());
        if raw.trim().is_empty() || raw.contains("://") {
            return Ok(None);
        }
        let (abs, parts) = clean(&raw, true);
        if parts.is_empty() {
            return Ok(None);
        }
        let resolve = |cands: &Vec<&'a Track>| -> Result<Option<&'a Track>, Vec<&'a Track>> {
            match Self::one_file(cands, e, &raw) {
                Ok(t) => Ok(Some(t)),
                Err(_) => Err(cands.clone()),
            }
        };
        if abs {
            if let Some(c) = self.by_path.get(&key_of(true, &parts)) {
                return resolve(c);
            }
        } else if let Some(b) = base {
            let (_, joined) = clean(&format!("{b}/{raw}"), true);
            let (babs, _) = clean(b, true);
            if let Some(c) = self.by_path.get(&key_of(babs, &joined)) {
                return resolve(c);
            }
        }
        // Only the end of the path in common ("…/CD1/01.flac"): the songs whose title and length
        // agree with what the entry says; with nothing said and a file name that says nothing
        // either (a track number), no match at all.
        let n = parts.len();
        let says_nothing = e.title.trim().is_empty() && e.text.trim().is_empty() && e.duration.is_none();
        let generic = says_nothing && generic_name(&parts[n - 1]);
        let agreeing = |c: &Vec<&'a Track>| -> Vec<&'a Track> { c.iter().copied().filter(|t| Self::agrees(e, t)).collect() };
        for k in [3, 2] {
            let Some(c) = (n >= k).then(|| parts[n - k..].join("/")).and_then(|s| if k == 3 { self.by_suffix3.get(&s) } else { self.by_suffix2.get(&s) }) else { continue };
            if generic {
                // Several songs end so: said to be ambiguous; one: still not enough to take it.
                return if c.len() > 1 { Err(c.clone()) } else { Ok(None) };
            }
            let fit = agreeing(c);
            return match (fit.is_empty(), c.len()) {
                // Several songs end so and none is the one the entry describes: ambiguous.
                (true, 2..) => Err(c.clone()),
                // The same end of a path, another song: the tags (if any) look further.
                (true, _) => Ok(None),
                _ => resolve(&fit),
            };
        }
        Ok(None)
    }

    /// Whether what an entry says of its song (its title, its length) fits a song found by the end
    /// of its path. An entry that says neither fits.
    fn agrees(e: &Entry, t: &Track) -> bool {
        let titles: Vec<String> = if !e.title.trim().is_empty() {
            vec![e.title.clone()]
        } else if !e.text.trim().is_empty() {
            splits(&e.text).into_iter().map(|(_, t)| t).collect()
        } else {
            Vec::new()
        };
        let title_ok = titles.is_empty()
            || titles.iter().any(|x| {
                let (k, c) = (match_key(x), core(x));
                (!k.is_empty() && k == match_key(&t.title)) || (!c.is_empty() && c == core(&t.title)) || (!k.is_empty() && k == match_key(&t.subtitle))
            });
        let length_ok = e.duration.is_none_or(|d| (t.duration - d).abs() <= SAME_LENGTH);
        title_ok && length_ok
    }

    pub fn find(&self, e: &Entry, base: Option<&str>, who: &Identity) -> Outcome<'a> {
        let mut only: Option<HashSet<&str>> = None;
        let mut shared = 0;
        match self.by_file(e, base) {
            Ok(Some(t)) => return Outcome::Matched(t),
            Ok(None) => {}
            Err(c) => {
                shared = c.len();
                only = Some(c.iter().map(|t| t.id.as_str()).collect());
            }
        }
        let has_tags = !e.title.is_empty() || !e.text.is_empty();
        match self.by_tags(e, who, only.as_ref()) {
            Tag::One(t) => Outcome::Matched(t),
            Tag::Ambiguous(n) => Outcome::Ambiguous(format!("{n} songs in your library fit equally well")),
            _ if shared > 1 => Outcome::Ambiguous(format!("{shared} songs in your library end with that path")),
            _ if !has_tags => Outcome::NotFound("No file at that path in your library".into()),
            Tag::Length(have) => Outcome::NotFound(match e.duration {
                Some(d) => format!("In your library, but {} long there and {} in the file", clock(have), clock(d)),
                None => "In your library, but with another length".into(),
            }),
            Tag::OtherArtist => Outcome::NotFound("The title is in your library, but by another artist".into()),
            Tag::NoArtist => Outcome::NotFound("Only a title, with no artist or length to check it against".into()),
            Tag::None => Outcome::NotFound("Not in your library".into()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Importing a file
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Miss {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: Option<f64>,
    pub isrc: String,
    pub reason: String,
    pub ambiguous: bool,
}

#[derive(Debug, Default)]
pub struct Imported {
    pub name: String,
    pub format: &'static str,
    pub total: usize,
    /// The ids of the songs found, in the file's order (a song listed twice is here twice).
    pub matched: Vec<String>,
    pub misses: Vec<Miss>,
}

impl Imported {
    pub fn ambiguous(&self) -> usize {
        self.misses.iter().filter(|m| m.ambiguous).count()
    }
}

fn miss(e: &Entry, reason: String, ambiguous: bool) -> Miss {
    let (mut artist, mut title) = (e.artist.clone(), e.title.clone());
    if title.is_empty() {
        let first = readings(e).into_iter().next();
        if let Some((a, t)) = first {
            artist = a;
            title = t;
        }
    }
    Miss { artist, title, album: e.album.clone(), duration: e.duration, isrc: e.isrc.clone(), reason, ambiguous }
}

/// The folder a path is in, whichever slash it uses (a path from another system isn't `Path`'s).
fn parent_of(path: &str) -> Option<String> {
    let i = path.rfind(['/', '\\'])?;
    match &path[..i] {
        "" if path.starts_with('/') => Some("/".into()),
        "" => None,
        p => Some(p.to_string()),
    }
}

/// Reads and matches one playlist file's text. `path`: where it is (its extension tells the kind,
/// its folder resolves relative paths, its name is the playlist's when it doesn't say).
pub fn import_text(path: &str, text: &str, index: &Index, who: &Identity) -> Result<Imported, String> {
    let kind = kind_of(path, text)?;
    let parsed = match kind {
        Kind::M3u => parse_m3u(text),
        Kind::Csv => parse_csv(text)?,
        Kind::Xspf => parse_xspf(text),
    };
    let base = parent_of(path);
    let mut out = Imported {
        name: parsed.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| file_stem(path)),
        format: kind.label(),
        total: parsed.entries.len(),
        ..Default::default()
    };
    for e in &parsed.entries {
        match index.find(e, base.as_deref(), who) {
            Outcome::Matched(t) => out.matched.push(t.id.clone()),
            Outcome::NotFound(why) => out.misses.push(miss(e, why, false)),
            Outcome::Ambiguous(why) => out.misses.push(miss(e, why, true)),
        }
    }
    Ok(out)
}

/// A file read from disk, size checked first.
pub fn read_playlist(path: &Path) -> Result<String, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let meta = std::fs::metadata(path).map_err(|e| format!("Can't open “{name}”: {e}"))?;
    if !meta.is_file() {
        return Err(format!("“{name}” isn't a file."));
    }
    if meta.len() > MAX_FILE {
        return Err(format!("“{name}” is {} MB: too big to be a playlist (the limit is {} MB).", meta.len() >> 20, MAX_FILE >> 20));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Can't read “{name}”: {e}"))?;
    Ok(decode(&bytes))
}

pub fn import_file(path: &Path, index: &Index, who: &Identity) -> Result<Imported, String> {
    let text = read_playlist(path)?;
    import_text(&path.to_string_lossy(), &text, index, who)
}

/// The unmatched entries as a CSV the downloader (or a person) can read: UTF-8 with a BOM.
pub fn missing_csv(miss: &[Miss]) -> Vec<u8> {
    let mut out = String::from("Artist,Title,Album,Duration (seconds),ISRC,Reason\r\n");
    for m in miss {
        let row = [
            m.artist.clone(),
            m.title.clone(),
            m.album.clone(),
            m.duration.map(|d| d.round().to_string()).unwrap_or_default(),
            m.isrc.clone(),
            m.reason.clone(),
        ];
        out.push_str(&row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(out.as_bytes());
    bytes
}

// ---------------------------------------------------------------------------------------------
// The commands' side (api.rs)
// ---------------------------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDone {
    pub path: String,
    /// Songs written.
    pub written: usize,
    /// Ids the library doesn't have, left out.
    pub skipped: usize,
    /// Songs of a CUE-sheet album (written with their file's path once each).
    pub cue: usize,
}

#[derive(Deserialize)]
pub struct ExportItem {
    pub name: String,
    pub ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportAllDone {
    pub folder: String,
    pub files: usize,
    pub written: usize,
    pub skipped: usize,
    pub cue: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub source: String,
    pub name: String,
    pub format: String,
    pub total: usize,
    pub matched: Vec<String>,
    pub not_found: usize,
    pub ambiguous: usize,
    /// The first unmatched entries (the missing list file has all).
    pub misses: Vec<Miss>,
    pub more_misses: usize,
    /// Why the file couldn't be read (the rest of the fields are empty).
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFiles {
    /// Android: where playlist files are looked for (the file picker can't hand over a readable path).
    pub dir: String,
    pub files: Vec<ImportFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    pub path: String,
    pub rows: usize,
}

fn songs(ctx: &crate::ctx::Ctx, ids: &[String]) -> (Vec<Track>, usize) {
    let lib = ctx.state.library.lock();
    let mut out = Vec::with_capacity(ids.len());
    let mut skipped = 0;
    for id in ids {
        match lib.by_id.get(id).and_then(|i| lib.snapshot.tracks.get(*i)) {
            Some(t) => out.push(t.clone()),
            None => skipped += 1,
        }
    }
    (out, skipped)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("Can't make the folder {}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("Can't save {}: {e}", path.display()))
}

/// Where a file with no chosen path goes: Android's Documents › Cadence (there's no save picker
/// that can be written to); none elsewhere.
pub(crate) fn default_dir() -> Result<PathBuf, String> {
    crate::backup::export_dir(Path::new("")).ok_or_else(|| "Choose where to save the file.".to_string())
}

pub fn export_one(ctx: &crate::ctx::Ctx, name: &str, ids: &[String], path: Option<String>, o: &ExportOptions) -> Result<ExportDone, String> {
    let ext = o.extension()?;
    let out = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            let mut p = PathBuf::from(p);
            if p.extension().is_none() {
                p.set_extension(ext);
            }
            p
        }
        None => default_dir()?.join(format!("{}.{ext}", safe_name(name))),
    };
    let (tracks, skipped) = songs(ctx, ids);
    let base = out.parent().map(|p| p.to_string_lossy().into_owned());
    let r = render(name, &tracks, base.as_deref(), o)?;
    write_file(&out, &r.bytes)?;
    Ok(ExportDone { path: out.to_string_lossy().into_owned(), written: tracks.len(), skipped, cue: r.cue })
}

pub fn export_all(ctx: &crate::ctx::Ctx, items: &[ExportItem], folder: Option<String>, o: &ExportOptions) -> Result<ExportAllDone, String> {
    let ext = o.extension()?;
    let dir = match folder.filter(|p| !p.trim().is_empty()) {
        Some(f) => PathBuf::from(f),
        None => default_dir()?,
    };
    if !items.is_empty() && dir.exists() && !dir.is_dir() {
        return Err(format!("{} isn't a folder.", dir.display()));
    }
    let base = dir.to_string_lossy().into_owned();
    let mut used: HashSet<String> = HashSet::new();
    let mut done = ExportAllDone { folder: base.clone(), files: 0, written: 0, skipped: 0, cue: 0 };
    for item in items {
        let stem = safe_name(&item.name);
        let mut file = stem.clone();
        let mut n = 1;
        while !used.insert(file.to_lowercase()) {
            n += 1;
            file = format!("{stem} ({n})");
        }
        let (tracks, skipped) = songs(ctx, &item.ids);
        let r = render(&item.name, &tracks, Some(&base), o)?;
        write_file(&dir.join(format!("{file}.{ext}")), &r.bytes)?;
        done.files += 1;
        done.written += tracks.len();
        done.skipped += skipped;
        done.cue += r.cue;
    }
    Ok(done)
}

/// Reads, parses and matches each file, one preview each; a file that can't be read has its error.
pub fn import_files(ctx: &crate::ctx::Ctx, paths: &[String]) -> Vec<ImportPreview> {
    let tracks = ctx.state.library.lock().snapshot.tracks.clone();
    let who = crate::identity::current();
    let index = Index::new(&tracks);
    paths
        .iter()
        .map(|p| match import_file(Path::new(p), &index, &who) {
            Ok(i) => {
                let ambiguous = i.ambiguous();
                let total_misses = i.misses.len();
                ImportPreview {
                    source: p.clone(),
                    name: i.name,
                    format: i.format.to_string(),
                    total: i.total,
                    matched: i.matched,
                    not_found: total_misses - ambiguous,
                    ambiguous,
                    more_misses: total_misses.saturating_sub(SHOWN_MISSES),
                    misses: i.misses.into_iter().take(SHOWN_MISSES).collect(),
                    error: None,
                }
            }
            Err(e) => ImportPreview {
                source: p.clone(),
                name: file_stem(p),
                format: String::new(),
                total: 0,
                matched: Vec::new(),
                not_found: 0,
                ambiguous: 0,
                misses: Vec::new(),
                more_misses: 0,
                error: Some(e),
            },
        })
        .collect()
}

/// Writes the unmatched entries of a playlist file to `path` (or, on Android, Documents › Cadence).
pub fn save_missing(ctx: &crate::ctx::Ctx, source: &str, path: Option<String>) -> Result<SavedFile, String> {
    let tracks = ctx.state.library.lock().snapshot.tracks.clone();
    let who = crate::identity::current();
    let index = Index::new(&tracks);
    let imported = import_file(Path::new(source), &index, &who)?;
    let out = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            let mut p = PathBuf::from(p);
            if p.extension().is_none() {
                p.set_extension("csv");
            }
            p
        }
        None => default_dir()?.join(format!("{} - not in library.csv", safe_name(&imported.name))),
    };
    write_file(&out, &missing_csv(&imported.misses))?;
    Ok(SavedFile { path: out.to_string_lossy().into_owned(), rows: imported.misses.len() })
}

/// Android: the playlist files in Documents › Cadence (elsewhere none: the open dialog is used).
pub fn import_candidates() -> ImportFiles {
    let Some(dir) = crate::backup::export_dir(Path::new("")) else { return ImportFiles { dir: String::new(), files: Vec::new() } };
    let mut files: Vec<ImportFile> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
            if !["m3u", "m3u8", "csv", "xspf"].contains(&ext.as_str()) {
                return None;
            }
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some(ImportFile { path: p.to_string_lossy().into_owned(), name: e.file_name().to_string_lossy().into_owned(), size: meta.len() })
        })
        .collect();
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    ImportFiles { dir: dir.to_string_lossy().into_owned(), files }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, path: &str, artist: &str, title: &str, album: &str, duration: f64) -> Track {
        Track { id: id.into(), path: path.into(), artist: artist.into(), title: title.into(), album: album.into(), duration, codec: "FLAC".into(), ..Default::default() }
    }

    fn who() -> Identity {
        Identity::new()
    }

    fn run(file: &str, text: &str, lib: &[Track]) -> Imported {
        run_as(file, text, lib, &who())
    }

    fn run_as(file: &str, text: &str, lib: &[Track], who: &Identity) -> Imported {
        import_text(file, text, &Index::new(lib), who).unwrap()
    }

    fn ids(i: &Imported) -> Vec<&str> {
        i.matched.iter().map(String::as_str).collect()
    }

    fn library() -> Vec<Track> {
        vec![
            song("a", r"D:\Music\Kenshi Yonezu\Lemon\01 Lemon.flac", "米津玄師", "Lemon", "Lemon", 255.0),
            song("b", "D:\\Music\\Reol, Giga\\Sol\\02 \"Quoted\", comma.flac", "Reol, Giga", "\"Quoted\", comma", "Sol", 200.4),
            song("c", r"D:\Music\幾田りら\Emoji 🎵\03 夜に駆ける 🎵.flac", "幾田りら", "夜に駆ける 🎵", "Emoji 🎵", 261.0),
            song("d", r"E:\Other\Artist\Album\04 Song.flac", "Artist", "Song", "Album", 180.0),
        ]
    }

    fn render_text(name: &str, tracks: &[Track], base: Option<&str>, o: &ExportOptions) -> String {
        String::from_utf8(render(name, tracks, base, o).unwrap().bytes).unwrap()
    }

    // ----- matching by the end of a path -----

    #[test]
    fn the_end_of_a_path_alone_is_not_enough() {
        let lib = vec![
            song("d1", r"D:\Music\Invented Band\Box Set\CD1\01.flac", "Invented Band", "Morning Made Up", "Box Set", 200.0),
            song("x", r"D:\Music\Other Act\Long Title Song.flac", "Other Act", "Long Title Song", "Other", 210.0),
        ];
        // Another machine's box set: only "CD1/01.flac" in common, and another title: not this song.
        let other = run("/p/x.m3u8", "#EXTM3U\n#EXTINF:180,Someone Else - Evening Invented\n/home/u/Music/Box/CD1/01.flac\n", &lib);
        assert!(other.matched.is_empty(), "{:?}", other.matched);
        // The same title and length: it is.
        let same = run("/p/x.m3u8", "#EXTM3U\n#EXTINF:200,Invented Band - Morning Made Up\n/home/u/Music/Box/CD1/01.flac\n", &lib);
        assert_eq!(ids(&same), ["d1"]);
        // The title agrees but not the length: not this song.
        let long = run("/p/x.m3u8", "#EXTM3U\n#EXTINF:320,Invented Band - Morning Made Up\n/home/u/Music/Box/CD1/01.flac\n", &lib);
        assert!(long.matched.is_empty());
        // Nothing said, and a file name that's only a track number: no match.
        let bare = run("/p/x.m3u8", "/home/u/Music/Box/CD1/01.flac\n", &lib);
        assert!(bare.matched.is_empty());
        // A file name that names the song, nothing else said: the end of the path is enough.
        let named = run("/p/x.m3u8", "/elsewhere/Other Act/Long Title Song.flac\n", &lib);
        assert_eq!(ids(&named), ["x"]);
        assert!(generic_name("01.flac") && generic_name("1-03.flac") && generic_name("Track 07.mp3") && generic_name("ab.flac"));
        assert!(!generic_name("Long Title Song.flac") && !generic_name("夜に駆ける.flac"));
    }

    // ----- export -----

    #[test]
    fn m3u8_has_the_header_name_extinf_and_path() {
        let lib = library();
        let text = render_text("Road trip", &lib[..1], None, &ExportOptions::default());
        assert_eq!(text, "#EXTM3U\n#PLAYLIST:Road trip\n#EXTINF:255,米津玄師 - Lemon\nD:\\Music\\Kenshi Yonezu\\Lemon\\01 Lemon.flac\n");
        assert!(!render(&"x".to_string(), &lib, None, &ExportOptions::default()).unwrap().bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "no BOM");
        let crlf = render_text("x", &lib[..1], None, &ExportOptions { crlf: true, ..Default::default() });
        assert!(crlf.contains("\r\n") && !crlf.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn relative_paths_use_dots_and_fall_back_to_absolute_on_another_drive() {
        let lib = library();
        let o = ExportOptions { relative: true, forward_slashes: true, ..Default::default() };
        let text = render_text("x", &lib, Some(r"D:\Music\Playlists"), &o);
        assert!(text.contains("\n../Kenshi Yonezu/Lemon/01 Lemon.flac\n"), "{text}");
        assert!(text.contains("\nE:/Other/Artist/Album/04 Song.flac\n"), "another drive stays absolute: {text}");
        let back = render_text("x", &lib[..1], Some(r"D:\Music\Playlists"), &ExportOptions { relative: true, ..Default::default() });
        assert!(back.contains("\n..\\Kenshi Yonezu\\Lemon\\01 Lemon.flac\n"), "{back}");
        assert_eq!(relative_to("/storage/emulated/0/Music/a/b.flac", "/storage/emulated/0/Documents/Cadence").as_deref(), Some("../../Music/a/b.flac"));
        assert_eq!(relative_to("/music/a.flac", "/music").as_deref(), Some("a.flac"));
    }

    #[test]
    fn csv_export_has_a_bom_a_header_and_quotes() {
        let lib = library();
        let r = render("x", &lib[..3], None, &ExportOptions { format: "csv".into(), ..Default::default() }).unwrap();
        assert!(r.bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        let text = String::from_utf8(r.bytes[3..].to_vec()).unwrap();
        let mut lines = text.split("\r\n");
        assert_eq!(lines.next().unwrap(), "Title,Artist,Album,Album artist,Duration (seconds),Track number,Year,Path");
        assert!(text.contains("\"Reol, Giga\""));
        assert!(text.contains("\"\"\"Quoted\"\", comma\""), "{text}");
        assert!(text.contains("夜に駆ける 🎵"));
        // Read back by the CSV reader.
        let rows = parse_csv_rows(&text);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2][0], "\"Quoted\", comma");
        assert_eq!(rows[2][1], "Reol, Giga");
    }

    #[test]
    fn names_are_made_safe_for_files() {
        assert_eq!(safe_name("Road trip: 2026/11?"), "Road trip_ 2026_11_");
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("  ..  "), "Playlist");
        assert_eq!(safe_name("夜に駆ける 🎵"), "夜に駆ける 🎵");
        assert!(safe_name(&"a".repeat(300)).len() <= 100);
    }

    // ----- round trips -----

    #[test]
    fn export_then_import_gives_the_same_songs() {
        let lib = library();
        let all: Vec<Track> = vec![lib[2].clone(), lib[0].clone(), lib[1].clone(), lib[3].clone(), lib[0].clone()];
        let want = ["c", "a", "b", "d", "a"];
        for relative in [false, true] {
            for forward_slashes in [false, true] {
                for crlf in [false, true] {
                    let o = ExportOptions { relative, forward_slashes, crlf, ..Default::default() };
                    let text = render_text("Mix", &all, Some(r"D:\Music\Playlists"), &o);
                    let got = run(r"D:\Music\Playlists\Mix.m3u8", &text, &lib);
                    assert_eq!(ids(&got), want, "relative={relative} forward={forward_slashes} crlf={crlf}\n{text}");
                    assert_eq!(got.name, "Mix");
                    assert!(got.misses.is_empty());
                }
            }
        }
    }

    #[test]
    fn csv_round_trips_by_path_and_by_tags() {
        let lib = library();
        let o = ExportOptions { format: "csv".into(), ..Default::default() };
        let text = decode(&render("Mix", &lib, None, &o).unwrap().bytes);
        let got = run("Mix.csv", &text, &lib);
        assert_eq!(ids(&got), ["a", "b", "c", "d"]);
        // Without its paths (another machine), the tags find the songs.
        let rows = parse_csv_rows(&text);
        let stripped: String = rows.iter().map(|r| r[..7].iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\r\n");
        let got = run("Mix.csv", &stripped, &lib);
        assert_eq!(ids(&got), ["a", "b", "c", "d"], "{stripped}");
    }

    #[test]
    fn cue_songs_are_written_with_the_files_path_and_read_back() {
        let span = |s| Some(crate::cue::Span { start: s, end: None });
        let mut a = song("c1", "/m/Album/album.flac", "Band", "First", "Album", 100.0);
        let mut b = song("c2", "/m/Album/album.flac", "Band", "Second - Part 2", "Album", 120.0);
        a.cue = span(0.0);
        b.cue = span(100.0);
        let lib = vec![a.clone(), b.clone()];
        let r = render("x", &[b.clone(), a.clone()], None, &ExportOptions::default()).unwrap();
        assert_eq!(r.cue, 2);
        let text = decode(&r.bytes);
        assert_eq!(text.matches("/m/Album/album.flac").count(), 2);
        assert!(text.contains("#EXTINF:120,Band - Second - Part 2"));
        let got = run("/p/x.m3u8", &text, &lib);
        assert_eq!(ids(&got), ["c2", "c1"]);
        // The path alone can't say which song of the file: ambiguous, not a guess.
        let bare = run("/p/x.m3u8", "/m/Album/album.flac\n", &lib);
        assert!(bare.matched.is_empty());
        assert_eq!(bare.ambiguous(), 1);
    }

    // ----- reading -----

    #[test]
    fn a_legacy_non_utf8_m3u_is_read_as_windows_1252() {
        let bytes = b"#EXTM3U\r\n#EXTINF:200,Beyonc\xE9 - Caf\xE9\r\nD:\\Music\\Beyonc\xE9\\Caf\xE9.flac\r\n".to_vec();
        let lib = vec![song("x", "D:\\Music\\Beyoncé\\Café.flac", "Beyoncé", "Café", "A", 200.0)];
        assert!(decode(&bytes).contains("Beyoncé"));
        assert_eq!(ids(&run("old.m3u", &decode(&bytes), &lib)), ["x"]);
        // UTF-8 with a BOM, and UTF-16 with a BOM.
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice("#EXTM3U\n#PLAYLIST:夜\nD:\\Music\\Beyoncé\\Café.flac\n".as_bytes());
        let got = run("a.m3u8", &decode(&with_bom), &lib);
        assert_eq!((ids(&got), got.name.as_str()), (vec!["x"], "夜"));
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("D:\\Music\\Beyoncé\\Café.flac\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(ids(&run("u.m3u", &decode(&utf16), &lib)), ["x"]);
    }

    #[test]
    fn file_urls_are_decoded() {
        let lib = vec![song("x", r"D:\Music\Artist X\Song (1) 夜.flac", "A", "S", "B", 100.0), song("y", "/home/me/My Music/a b.flac", "A", "T", "B", 100.0)];
        let text = "#EXTM3U\nfile:///D:/Music/Artist%20X/Song%20%281%29%20%E5%A4%9C.flac\nFILE:///home/me/My%20Music/a%20b.flac\nfile://localhost/home/me/My%20Music/a%20b.flac\n";
        assert_eq!(ids(&run("p.m3u", text, &lib)), ["x", "y", "y"]);
        assert_eq!(from_url("C:\\50%25 off\\a.flac"), "C:\\50%25 off\\a.flac", "only file:// addresses are decoded");
    }

    #[test]
    fn comments_blank_lines_and_odd_line_ends_are_fine() {
        let lib = library();
        let text = "\u{feff}#EXTM3U\r\n\r\n# a comment\r\n#EXTINF:abc,Whatever\r\nD:\\Music\\Kenshi Yonezu\\Lemon\\01 Lemon.flac\r\r#EXTALB:x\rE:/Other/Artist/Album/04 Song.flac\n\n";
        let got = run("x.m3u", text, &lib);
        assert_eq!(ids(&got), ["a", "d"]);
        assert_eq!(got.total, 2);
    }

    #[test]
    fn paths_from_the_other_kind_of_machine() {
        // A Windows playlist on a Linux library, and the other way round.
        let linux = vec![song("l", "/mnt/music/Artist/Album/01 Song.flac", "A", "S", "B", 1.0)];
        assert_eq!(ids(&run("/p/x.m3u8", "D:\\Music\\Artist\\Album\\01 Song.flac\n", &linux)), ["l"]);
        let win = vec![song("w", r"D:\Music\Artist\Album\01 Song.flac", "A", "S", "B", 1.0)];
        assert_eq!(ids(&run("/p/x.m3u8", "/mnt/music/Artist/Album/01 Song.flac\n", &win)), ["w"]);
        // Letter case doesn't matter, nor do composed and decomposed letters (macOS).
        let mac = vec![song("m", "/Users/me/Music/Beyonce\u{301}/Cafe\u{301}.flac", "A", "S", "B", 1.0)];
        assert_eq!(ids(&run("/p/x.m3u8", "/users/ME/music/Beyonc\u{e9}/CAF\u{e9}.flac\n", &mac)), ["m"]);
    }

    #[test]
    fn a_playlist_from_another_machine_is_found_by_the_end_of_its_paths() {
        let lib = library();
        let text = "#EXTM3U\n/home/me/music/Kenshi Yonezu/Lemon/01 Lemon.flac\n../Kenshi Yonezu/Lemon/01 Lemon.flac\n";
        let got = run("/other/Mix.m3u8", text, &lib);
        assert_eq!(ids(&got), ["a", "a"]);
        // Only the file name: too little to go on.
        assert_eq!(run("/other/Mix.m3u8", "01 Lemon.flac\n", &lib).matched.len(), 0);
    }

    #[test]
    fn two_songs_with_the_same_path_end_are_not_guessed() {
        let lib = vec![
            song("x", r"D:\A\Artist\Album\01.flac", "Artist", "One", "Album", 100.0),
            song("y", r"E:\B\Artist\Album\01.flac", "Artist", "Two", "Album", 100.0),
        ];
        let got = run("/p/m.m3u8", "/other/Artist/Album/01.flac\n", &lib);
        assert!(got.matched.is_empty());
        assert_eq!(got.ambiguous(), 1);
        assert!(got.misses[0].reason.contains("2 songs"));
        // With tags that say which, they decide between those two.
        let got = run("/p/m.m3u8", "#EXTINF:100,Artist - Two\n/other/Artist/Album/01.flac\n", &lib);
        assert_eq!(ids(&got), ["y"]);
        // And tags that fit neither leave it ambiguous.
        let got = run("/p/m.m3u8", "#EXTINF:100,Artist - Three\n/other/Artist/Album/01.flac\n", &lib);
        assert_eq!(got.ambiguous(), 1);
    }

    // ----- tags -----

    #[test]
    fn extinf_only_entries_match_by_tags_across_scripts() {
        let lib = library();
        let pair = Identity::from_pairs(&[("Kenshi Yonezu", "米津玄師")]);
        let text = "#EXTINF:255,Kenshi Yonezu - Lemon\n/gone/a.flac\n#EXTINF:254,Kenshi Yonezu - Lemon\n/gone/b.flac\n";
        assert!(run_as("p.m3u", text, &lib, &who()).matched.is_empty(), "without the pair they're two artists");
        let got = run_as("p.m3u", text, &lib, &pair);
        assert_eq!(ids(&got), ["a", "a"]);
        // The same title with another length is another recording.
        let off = run_as("p.m3u", "#EXTINF:300,Kenshi Yonezu - Lemon\n/gone/a.flac\n", &lib, &pair);
        assert!(off.matched.is_empty());
        assert!(off.misses[0].reason.contains("4:15") && off.misses[0].reason.contains("5:00"), "{}", off.misses[0].reason);
    }

    #[test]
    fn titles_and_artists_with_dashes() {
        let lib = vec![
            song("t1", "/m/1.flac", "A", "Re - Birth", "X", 200.0),
            song("t2", "/m/2.flac", "Jay - Z", "99 Problems", "X", 230.0),
            song("t3", "/m/3.flac", "Solo", "Title", "X", 100.0),
        ];
        let text = "#EXTINF:200,A - Re - Birth\n/x/1\n#EXTINF:230,Jay - Z - 99 Problems\n/x/2\n#EXTINF:100,Solo - Title\n/x/3\n#EXTINF:100,Title\n/x/4\n";
        let got = run("p.m3u", text, &lib);
        assert_eq!(ids(&got), ["t1", "t2", "t3", "t3"], "a title alone with its length is one song");
    }

    #[test]
    fn feat_and_remastered_variants_match_but_not_other_recordings() {
        let lib = vec![
            song("s", "/m/s.flac", "A", "Song", "X", 200.0),
            song("r", "/m/r.flac", "B", "Tune (Remastered 2011)", "X", 180.0),
            song("f", "/m/f.flac", "C", "Track (feat. Guest)", "X", 150.0),
            song("l", "/m/l.flac", "D", "Piece - Live", "X", 300.0),
        ];
        let text = "#EXTINF:200,A - Song (feat. Someone)\n/x/1\n#EXTINF:180,B - Tune\n/x/2\n#EXTINF:150,C - Track\n/x/3\n#EXTINF:180,B - Tune - Remastered 2011\n/x/4\n#EXTINF:300,D - Piece\n/x/5\n#EXTINF:200,A - Song ft. Someone\n/x/6\n#EXTINF:150,C - Track (Live)\n/x/7\n";
        let got = run("p.m3u", text, &lib);
        assert_eq!(ids(&got), ["s", "r", "f", "r", "s"]);
        assert_eq!(got.misses.len(), 2, "the studio Piece isn't the live one, nor is Track (Live): {:?}", got.misses);
        assert_eq!(crate::autotag::version_kinds("Song (Remastered 2011)"), REMASTER);
    }

    #[test]
    fn entries_without_a_length() {
        let two = vec![song("x", "/m/1.flac", "A", "Song", "Single", 200.0), song("y", "/m/2.flac", "A", "Song", "Album", 231.0)];
        let one = vec![song("x", "/m/1.flac", "A", "Song", "Single", 200.0)];
        let text = "#EXTINF:-1,A - Song\n/x/1\n";
        assert_eq!(ids(&run("p.m3u", text, &one)), ["x"]);
        let got = run("p.m3u", text, &two);
        assert!(got.matched.is_empty());
        assert_eq!(got.ambiguous(), 1, "two different recordings: ambiguous");
        // The album the entry names settles it.
        let csv = "Title,Artist,Album\nSong,A,Album\n";
        assert_eq!(ids(&run("p.csv", csv, &two)), ["y"]);
        // One recording on two albums: the best copy (24-bit), whichever comes first.
        let mut hi = song("hi", "/m/hi.flac", "A", "Song", "Deluxe", 200.3);
        hi.bit_depth = Some(24);
        let mut lo = song("lo", "/m/lo.flac", "A", "Song", "Album", 200.0);
        lo.bit_depth = Some(16);
        for lib in [vec![hi.clone(), lo.clone()], vec![lo.clone(), hi.clone()]] {
            assert_eq!(ids(&run("p.m3u", text, &lib)), ["hi"]);
        }
        // Without quality to tell them apart: the earliest year, then the path.
        let (mut a, mut b) = (lo.clone(), hi.clone());
        b.bit_depth = Some(16);
        a.year = Some(2019);
        b.year = Some(2021);
        assert_eq!(ids(&run("p.m3u", text, &[b, a])), ["lo"]);
    }

    #[test]
    fn two_equal_candidates_with_a_length_are_ambiguous() {
        let lib = vec![song("x", "/m/1.flac", "A", "Song", "One", 199.0), song("y", "/m/2.flac", "A", "Song", "Two", 201.5)];
        let got = run("p.m3u", "#EXTINF:200,A - Song\n/x/1\n", &lib);
        assert_eq!((got.matched.len(), got.ambiguous(), got.misses.len()), (0, 1, 1));
        assert!(got.misses[0].ambiguous);
    }

    #[test]
    fn a_title_alone_without_a_length_is_not_matched() {
        let lib = vec![song("x", "/m/1.flac", "A", "Intro", "One", 60.0)];
        let got = run("p.m3u", "Intro\n", &lib);
        assert!(got.matched.is_empty());
        assert!(got.misses[0].reason.contains("No file at that path"), "{}", got.misses[0].reason);
    }

    #[test]
    fn duplicates_in_the_file_are_kept_in_order() {
        let lib = library();
        let text = "D:\\Music\\Kenshi Yonezu\\Lemon\\01 Lemon.flac\nE:\\Other\\Artist\\Album\\04 Song.flac\nD:\\Music\\Kenshi Yonezu\\Lemon\\01 Lemon.flac\n";
        let got = run("p.m3u", text, &lib);
        assert_eq!(ids(&got), ["a", "d", "a"]);
        assert_eq!(got.total, 3);
    }

    // ----- other formats -----

    #[test]
    fn exportify_csv_is_read_by_header_name() {
        let lib = vec![
            song("1", "/m/1.flac", "Kenshi Yonezu", "Lemon", "Lemon", 255.4),
            song("2", "/m/2.flac", "Reol", "Sol", "Sol", 200.0),
            song("3", "/m/3.flac", "Reol, Giga", "Sol", "Giga", 233.0),
        ];
        let csv = "\u{feff}\"Track URI\",\"track name\",\"Album Name\",\"Artist Name(s)\",\"Release Date\",\"DURATION (MS)\",\"ISRC\"\n\
                   \"spotify:track:1\",\"Lemon\",\"Lemon\",\"Kenshi Yonezu\",\"2018\",\"255000\",\"JPPO01\"\n\
                   \"spotify:track:2\",\"Sol\",\"Sol\",\"Reol;Giga\",\"2020\",\"233100\",\"JPPO02\"\n\
                   \"spotify:track:3\",\"Not Here\",\"X\",\"Nobody\",\"2020\",\"100000\",\"USXX03\"\n";
        let got = run("Liked Songs.csv", csv, &lib);
        assert_eq!(ids(&got), ["1", "3"]);
        assert_eq!(got.name, "Liked Songs");
        assert_eq!(got.misses.len(), 1);
        assert_eq!((got.misses[0].artist.as_str(), got.misses[0].title.as_str(), got.misses[0].isrc.as_str()), ("Nobody", "Not Here", "USXX03"));
        assert_eq!(got.misses[0].duration, Some(100.0));
        let missing = String::from_utf8(missing_csv(&got.misses)[3..].to_vec()).unwrap();
        assert_eq!(missing, "Artist,Title,Album,Duration (seconds),ISRC,Reason\r\nNobody,Not Here,X,100,USXX03,Not in your library\r\n");
        assert!(parse_csv("Foo,Bar\r\n1,2\r\n").is_err());
        assert_eq!(parse_csv_rows("a;b\r\n\"x;1\";\"line\nbreak\"\r\n"), vec![vec!["a", "b"], vec!["x;1", "line\nbreak"]]);
    }

    #[test]
    fn xspf_is_read() {
        let lib = library();
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\"><title>My &amp; Mix</title><trackList>\
            <track><location>file:///D:/Music/Kenshi%20Yonezu/Lemon/01%20Lemon.flac</location><title>Lemon</title></track>\
            <track><title>Sol</title><creator>Reol, Giga</creator><album>Sol</album><duration>200400</duration></track>\
            <track><title><![CDATA[Nope & more]]></title><creator>Z</creator></track></trackList></playlist>";
        let got = run("x.xspf", xml, &lib);
        assert_eq!(got.name, "My & Mix");
        assert_eq!(got.total, 3);
        assert_eq!(ids(&got), ["a"]);
        // The second has no path and its title is "Sol", not the library's quoted title: not found.
        assert_eq!(got.misses[1].title, "Nope & more");
    }

    #[test]
    fn the_file_name_names_the_playlist_unless_it_says() {
        assert_eq!(run("/p/Road Trip.m3u8", "#EXTM3U\n", &[]).name, "Road Trip");
        assert_eq!(run("/p/a.m3u8", "#PLAYLIST:Named\n", &[]).name, "Named");
        assert!(import_text("a.txt", "hello", &Index::new(&[]), &who()).is_err());
    }

    // ----- limits -----

    #[test]
    fn a_huge_file_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.m3u8");
        std::fs::File::create(&p).unwrap().set_len(MAX_FILE + 1).unwrap();
        let err = import_file(&p, &Index::new(&[]), &who()).unwrap_err();
        assert!(err.contains("too big") && err.contains("big.m3u8"), "{err}");
        assert!(import_file(&dir.path().join("none.m3u"), &Index::new(&[]), &who()).is_err());
    }

    #[test]
    fn an_import_from_a_real_file_resolves_paths_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let lists = dir.path().join("Lists");
        std::fs::create_dir_all(&lists).unwrap();
        let track = music.join("Art").join("a b.flac");
        let lib = vec![song("k", &track.to_string_lossy(), "Art", "Ab", "Al", 10.0)];
        let file = lists.join("Mine.m3u");
        std::fs::write(&file, "#EXTM3U\n../Music/Art/a b.flac\n").unwrap();
        let got = import_file(&file, &Index::new(&lib), &who()).unwrap();
        assert_eq!((ids(&got), got.name.as_str()), (vec!["k"], "Mine"));
    }

    #[test]
    fn fifty_thousand_lines_are_matched_quickly() {
        let lib: Vec<Track> = (0..20_000).map(|i| song(&format!("id{i}"), &format!("D:\\Music\\Artist {}\\Album {}\\{:02} Song {i}.flac", i % 500, i % 2000, i % 30), &format!("Artist {}", i % 500), &format!("Song {i}"), &format!("Album {}", i % 2000), 180.0 + (i % 50) as f64)).collect();
        let mut text = String::from("#EXTM3U\n");
        for n in 0..25_000usize {
            let i = (n * 7) % 24_000; // some songs the library doesn't have
            text.push_str(&format!("#EXTINF:{},Artist {} - Song {i}\nD:\\Elsewhere\\Artist {}\\Album {}\\{:02} Song {i}.flac\n", 180 + i % 50, i % 500, i % 500, i % 2000, i % 30));
        }
        assert_eq!(text.lines().count(), 50_001);
        let started = std::time::Instant::now();
        let got = run("/p/big.m3u8", &text, &lib);
        let took = started.elapsed();
        assert_eq!(got.total, 25_000);
        assert_eq!(got.matched.len() + got.misses.len(), 25_000);
        assert!(got.matched.len() > 20_000, "{}", got.matched.len());
        assert!(took < std::time::Duration::from_secs(20), "took {took:?}");
    }
}
