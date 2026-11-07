//! CUE sheets: an album kept as one long audio file, with a `.cue` text file listing where each
//! song starts. Cadence turns such a file into one library track per song; the engine plays each
//! from its start to the next one's (gaplessly, since they're the same audio).
//!
//! Only a FILE that holds two or more songs is split. A cue that lists one file per song (what
//! most rippers write next to already-split files) is ignored: its files are in the library on
//! their own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::audio::loudness::{GainInfo, SongGain, parse_tag_number};
use crate::library::{AUDIO_EXTENSIONS, Track, short_hash};

/// Where a library track sits inside its audio file.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    /// Seconds into the file where the song starts.
    pub start: f64,
    /// Where it ends (the next song's start); None for the last song of the file.
    pub end: Option<f64>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Sheet {
    pub performer: Option<String>,
    pub title: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub album_gain: Option<GainInfo>,
    pub files: Vec<File>,
}

#[derive(Debug, Default, PartialEq)]
pub struct File {
    pub name: String,
    pub tracks: Vec<CueTrack>,
}

#[derive(Debug, Default, PartialEq)]
pub struct CueTrack {
    pub no: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    /// Seconds from the start of its file (INDEX 01: after any pregap).
    pub start: f64,
    pub gain: Option<GainInfo>,
}

/// Cue sheets are usually UTF-8, but rippers on Windows write the system's code page: Shift-JIS
/// for Japanese albums, Windows-1252 for the rest. Shift-JIS is only believed when the text has
/// kana in it (Latin accents in a Windows-1252 file can also form valid Shift-JIS, as garbage).
pub fn decode(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.trim_start_matches('\u{feff}').to_string();
    }
    if let Some(s) = encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(bytes) {
        if s.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)) {
            return s.into_owned();
        }
    }
    encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned()
}

/// The arguments of a cue line: words, or "quoted text".
fn args(rest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = rest.trim().chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut s = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                s.push(c);
            }
            out.push(s);
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                s.push(c);
                chars.next();
            }
            out.push(s);
        }
    }
    out
}

/// "mm:ss:ff" (75 frames to a second) as seconds.
fn time(s: &str) -> Option<f64> {
    let mut p = s.split(':').map(|n| n.trim().parse::<u32>().ok());
    let (m, s, f) = (p.next()??, p.next()??, p.next()??);
    Some(f64::from(m) * 60.0 + f64::from(s) + f64::from(f) / 75.0)
}

fn gain(value: &str) -> Option<f32> {
    parse_tag_number(value)
}

pub fn parse(text: &str) -> Sheet {
    let mut sheet = Sheet::default();
    // The song being read: its number, the start found so far (INDEX 01, else INDEX 00), and
    // whether it's an audio track at all.
    // Each index remembers which FILE it was in: a song's INDEX 00 (pregap) can be at the end of
    // the previous file while its INDEX 01 is in the next one (EAC's "gaps appended" layout), and
    // the song belongs to the file its INDEX 01 is in.
    struct Open {
        track: CueTrack,
        index0: Option<(usize, f64)>,
        index1: Option<(usize, f64)>,
        audio: bool,
        gain: Option<f32>,
        peak: Option<f32>,
    }
    let mut album_gain: (Option<f32>, Option<f32>) = (None, None);
    let mut open: Option<Open> = None;
    let close = |sheet: &mut Sheet, open: &mut Option<Open>| {
        let Some(mut o) = open.take() else { return };
        let Some((file, start)) = o.index1.or(o.index0) else { return };
        let Some(file) = sheet.files.get_mut(file) else { return };
        if !o.audio {
            return;
        }
        o.track.start = start;
        o.track.gain = o.gain.map(|g| GainInfo { gain: g, peak: o.peak.filter(|p| *p > 0.0).unwrap_or(1.0) });
        file.tracks.push(o.track);
    };
    for line in text.lines() {
        let line = line.trim();
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let a = args(rest);
        match cmd.to_ascii_uppercase().as_str() {
            "FILE" => {
                // (A song stays open across a FILE line: its INDEX 01 may follow it.)
                if let Some(name) = a.first() {
                    sheet.files.push(File { name: name.clone(), tracks: Vec::new() });
                }
            }
            "TRACK" => {
                close(&mut sheet, &mut open);
                open = Some(Open {
                    track: CueTrack { no: a.first().and_then(|n| n.parse().ok()).unwrap_or(0), ..Default::default() },
                    index0: None,
                    index1: None,
                    audio: a.get(1).is_none_or(|t| t.eq_ignore_ascii_case("AUDIO")),
                    gain: None,
                    peak: None,
                });
            }
            "TITLE" => match (&mut open, a.first()) {
                (Some(o), Some(t)) => o.track.title = Some(t.clone()),
                (None, Some(t)) => sheet.title = Some(t.clone()),
                _ => {}
            },
            "PERFORMER" => match (&mut open, a.first()) {
                (Some(o), Some(t)) => o.track.performer = Some(t.clone()),
                (None, Some(t)) => sheet.performer = Some(t.clone()),
                _ => {}
            },
            "INDEX" => {
                if let (Some(o), Some(n), Some(t), Some(file)) =
                    (&mut open, a.first(), a.get(1).and_then(|t| time(t)), sheet.files.len().checked_sub(1))
                {
                    match n.as_str() {
                        "00" => o.index0 = Some((file, t)),
                        "01" => o.index1 = Some((file, t)),
                        _ => {}
                    }
                }
            }
            "REM" => {
                let key = a.first().map(|k| k.to_ascii_uppercase()).unwrap_or_default();
                let value = a.get(1).cloned().unwrap_or_default();
                match (key.as_str(), &mut open) {
                    ("GENRE", None) => sheet.genre = Some(value).filter(|v| !v.is_empty()),
                    ("DATE", None) => sheet.year = value.get(..4).and_then(|y| y.parse().ok()),
                    ("REPLAYGAIN_ALBUM_GAIN", None) => album_gain.0 = gain(&value),
                    ("REPLAYGAIN_ALBUM_PEAK", None) => album_gain.1 = value.parse().ok(),
                    ("REPLAYGAIN_TRACK_GAIN", Some(o)) => o.gain = gain(&value),
                    ("REPLAYGAIN_TRACK_PEAK", Some(o)) => o.peak = value.parse().ok(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    close(&mut sheet, &mut open);
    sheet.album_gain = album_gain.0.map(|g| GainInfo { gain: g, peak: album_gain.1.filter(|p| *p > 0.0).unwrap_or(1.0) });
    sheet
}

pub fn read(path: &Path) -> Option<Sheet> {
    let bytes = std::fs::read(path).ok()?;
    Some(parse(&decode(&bytes)))
}

/// Case- and separator-insensitive form of a path, to match what a cue says with what's on disk.
fn norm(s: &str) -> String {
    s.replace('\\', "/").to_lowercase()
}

/// The audio file a FILE line means: the file as named, else one with the same name and another
/// audio extension (cues say "WAVE" and name `.wav` after the ripper's flac was converted), else a
/// file named like the cue itself.
fn resolve(cue: &Path, name: &str, by_path: &HashMap<String, usize>) -> Option<usize> {
    let dir = cue.parent()?;
    let named = dir.join(name.replace('\\', "/"));
    let mut tries = vec![named.clone()];
    for base in [named, cue.to_path_buf()] {
        for ext in AUDIO_EXTENSIONS {
            tries.push(base.with_extension(ext));
        }
    }
    tries.iter().find_map(|p| by_path.get(&norm(&p.to_string_lossy())).copied())
}

/// The songs of `file` (a FILE of the cue with two or more songs), made from the library's one
/// track for the audio file.
fn expand(base: &Track, sheet: &Sheet, file: &File) -> Vec<Track> {
    let tracks: Vec<&CueTrack> = file.tracks.iter().filter(|t| t.start < base.duration).collect();
    let album = sheet.title.clone().unwrap_or_else(|| base.album.clone());
    let album_artist = sheet.performer.clone().unwrap_or_else(|| base.album_artist.clone());
    let album_key = format!("{}|{}", album_artist.to_lowercase(), album.to_lowercase());
    tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let end = tracks.get(i + 1).map(|n| n.start);
            let mut track = base.clone();
            track.id = short_hash(&format!("{}#cue{}", base.path, t.no));
            track.title = t.title.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Track {:02}", t.no));
            track.artist = t.performer.clone().or_else(|| sheet.performer.clone()).unwrap_or_else(|| base.artist.clone());
            track.album = album.clone();
            track.album_artist = album_artist.clone();
            track.album_id = short_hash(&album_key);
            track.track_no = Some(t.no);
            if let Some(g) = &sheet.genre {
                track.genre = g.clone();
            }
            track.year = sheet.year.or(base.year);
            track.duration = end.unwrap_or(base.duration) - t.start;
            // The cue's ReplayGain values are this song's; the whole file's would be wrong.
            track.gain = SongGain { track: t.gain, album: sheet.album_gain.or(base.gain.album) };
            // Measuring loudness or sound for each song would read the whole file each time.
            track.gain_measured = true;
            track.cue = Some(Span { start: t.start, end });
            track
        })
        .collect()
}

/// Replaces each audio file that a cue splits into several songs with those songs.
pub fn apply(tracks: Vec<Track>, cues: &[PathBuf]) -> Vec<Track> {
    if cues.is_empty() {
        return tracks;
    }
    let by_path: HashMap<String, usize> = tracks.iter().enumerate().map(|(i, t)| (norm(&t.path), i)).collect();
    let mut split: HashMap<usize, Vec<Track>> = HashMap::new();
    for cue in cues {
        let Some(sheet) = read(cue) else { continue };
        for file in sheet.files.iter().filter(|f| f.tracks.len() >= 2) {
            let Some(i) = resolve(cue, &file.name, &by_path) else {
                log::info!("cue {}: can't find {}", cue.display(), file.name);
                continue;
            };
            if split.contains_key(&i) {
                continue; // The first cue for a file wins.
            }
            let songs = expand(&tracks[i], &sheet, file);
            if songs.len() >= 2 {
                split.insert(i, songs);
            }
        }
    }
    if !split.is_empty() {
        log::info!("{} audio files are split into songs by cue sheets", split.len());
    }
    tracks
        .into_iter()
        .enumerate()
        .flat_map(|(i, t)| split.remove(&i).unwrap_or_else(|| vec![t]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = "REM GENRE Euro-House\nREM DATE 1991\nREM REPLAYGAIN_ALBUM_GAIN -2.76 dB\nREM REPLAYGAIN_ALBUM_PEAK 0.972656\nPERFORMER \"2 Unlimited\"\nTITLE \"Get Ready for This\"\nFILE \"2 Unlimited - Get Ready for This.flac\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Orchestral Mix Edit\"\n    REM REPLAYGAIN_TRACK_GAIN -3.53 dB\n    REM REPLAYGAIN_TRACK_PEAK 0.972656\n    INDEX 00 00:00:00\n    INDEX 01 00:00:33\n  TRACK 02 AUDIO\n    TITLE \"800 Mix Edit\"\n    PERFORMER \"Someone Else\"\n    INDEX 00 02:53:50\n    INDEX 01 02:55:03\n  TRACK 03 AUDIO\n    TITLE \"Orchestral Mix\"\n    INDEX 01 08:22:63\n";

    fn base() -> Track {
        Track {
            id: "base".into(),
            path: "D:\\Music\\Album\\2 Unlimited - Get Ready for This.flac".into(),
            title: "whole".into(),
            artist: "2 Unlimited".into(),
            album: "Get Ready for This".into(),
            album_artist: "2 Unlimited".into(),
            duration: 600.0,
            codec: "FLAC".into(),
            size: 1,
            ..Default::default()
        }
    }

    #[test]
    fn reads_a_sheet() {
        let s = parse(SHEET);
        assert_eq!((s.performer.as_deref(), s.title.as_deref(), s.genre.as_deref(), s.year), (Some("2 Unlimited"), Some("Get Ready for This"), Some("Euro-House"), Some(1991)));
        assert_eq!(s.album_gain, Some(GainInfo { gain: -2.76, peak: 0.972656 }));
        assert_eq!(s.files.len(), 1);
        let t = &s.files[0].tracks;
        assert_eq!(t.len(), 3);
        // INDEX 01 is where a song starts (00 is the pregap); 75 frames to a second.
        assert!((t[0].start - 33.0 / 75.0).abs() < 1e-9);
        assert!((t[1].start - (2.0 * 60.0 + 55.0 + 3.0 / 75.0)).abs() < 1e-9);
        assert_eq!(t[0].gain, Some(GainInfo { gain: -3.53, peak: 0.972656 }));
        assert_eq!((t[1].title.as_deref(), t[1].performer.as_deref()), (Some("800 Mix Edit"), Some("Someone Else")));
        assert_eq!(t[2].gain, None);
    }

    #[test]
    fn handles_odd_sheets() {
        // Unquoted names, lower-case commands, a bad line, only INDEX 00, a data track.
        let s = parse("file song.wav WAVE\ntrack 01 audio\n index 01 00:00:00\nTRACK 02 AUDIO\n INDEX 00 01:00:00\nTRACK 03 MODE1/2352\n INDEX 01 02:00:00\nTRACK 04 AUDIO\n INDEX 01 xx\nnonsense\n");
        let t = &s.files[0].tracks;
        assert_eq!(t.iter().map(|t| t.no).collect::<Vec<_>>(), [1, 2], "the data track and the track without a time are left out");
        assert_eq!(t[1].start, 60.0);
        assert_eq!(parse("").files.len(), 0);
    }

    #[test]
    fn a_song_belongs_to_the_file_of_its_index_01() {
        // EAC's "gaps appended": song 2's pregap (INDEX 00) is at the end of file 1, its INDEX 01
        // is at the start of file 2.
        let s = parse("FILE \"1.wav\" WAVE\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  INDEX 00 04:08:30\nFILE \"2.wav\" WAVE\n  INDEX 01 00:00:00\n TRACK 03 AUDIO\n  INDEX 00 01:29:64\nFILE \"3.wav\" WAVE\n  INDEX 01 00:00:00\n");
        let counts: Vec<usize> = s.files.iter().map(|f| f.tracks.len()).collect();
        assert_eq!(counts, [1, 1, 1], "one song per file: nothing to split");
        assert_eq!(s.files[1].tracks[0].no, 2);
        assert_eq!(s.files[1].tracks[0].start, 0.0);
        // A song with only an INDEX 00 still gets that as its start.
        let s = parse("FILE \"a.wav\" WAVE\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  INDEX 00 01:00:00\n");
        assert_eq!(s.files[0].tracks.len(), 2);
    }

    #[test]
    fn splits_a_file_into_songs() {
        let s = parse(SHEET);
        let songs = expand(&base(), &s, &s.files[0]);
        assert_eq!(songs.len(), 3);
        assert_eq!(songs[0].title, "Orchestral Mix Edit");
        assert_eq!(songs[0].track_no, Some(1));
        assert_eq!(songs[0].artist, "2 Unlimited");
        assert_eq!(songs[1].artist, "Someone Else");
        // Each song runs to the next one's start; the last to the end of the file.
        let first = songs[0].cue.unwrap();
        assert_eq!(first.end, Some(songs[1].cue.unwrap().start));
        assert!((songs[0].duration - (first.end.unwrap() - first.start)).abs() < 1e-9);
        assert_eq!(songs[2].cue.unwrap().end, None);
        assert!((songs[2].duration - (600.0 - songs[2].cue.unwrap().start)).abs() < 1e-9);
        // Distinct ids, one album, the cue's ReplayGain (not the whole file's).
        assert_eq!(songs.iter().map(|t| &t.id).collect::<std::collections::HashSet<_>>().len(), 3);
        assert!(songs.iter().all(|t| t.album_id == songs[0].album_id && t.path == base().path && t.gain_measured));
        assert_eq!(songs[0].gain.track, Some(GainInfo { gain: -3.53, peak: 0.972656 }));
        assert_eq!(songs[0].gain.album, Some(GainInfo { gain: -2.76, peak: 0.972656 }));
        assert_eq!(songs[0].genre, "Euro-House");
        assert_eq!(songs[0].year, Some(1991));
    }

    #[test]
    fn songs_past_the_end_of_the_file_are_dropped() {
        let s = parse(SHEET);
        let mut short = base();
        short.duration = 300.0; // The cue's third song starts at 8:22.
        assert_eq!(expand(&short, &s, &s.files[0]).len(), 2);
    }

    #[test]
    fn decodes_text_encodings() {
        assert_eq!(decode("\u{feff}TITLE \"é\"".as_bytes()), "TITLE \"é\"");
        // Windows-1252: an en dash and an apostrophe; also bytes that would be (garbage) Shift-JIS.
        assert_eq!(decode(b"Maximum \x96 Overdrive"), "Maximum \u{2013} Overdrive");
        assert_eq!(decode(b"Don\x92t \xa1Hola\xbf"), "Don\u{2019}t \u{a1}Hola\u{bf}");
        // Shift-JIS with kana.
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("TITLE \"ダーリン\"");
        assert_eq!(decode(&sjis), "TITLE \"ダーリン\"");
    }

    #[test]
    fn apply_splits_only_files_with_several_songs() {
        let dir = std::env::temp_dir().join(format!("cadence-cue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cue = dir.join("album.cue");
        // The cue names a .wav that was converted to .flac; a second FILE has one song only.
        std::fs::write(&cue, SHEET.replace("2 Unlimited - Get Ready for This.flac", "album.wav") + "FILE \"bonus.flac\" WAVE\n  TRACK 04 AUDIO\n    INDEX 01 00:00:00\n").unwrap();
        let mk = |name: &str| Track { id: name.into(), path: dir.join(name).to_string_lossy().into_owned(), duration: 600.0, ..Default::default() };
        let out = apply(vec![mk("album.flac"), mk("bonus.flac"), mk("other.flac")], &[cue]);
        assert_eq!(out.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Orchestral Mix Edit", "800 Mix Edit", "Orchestral Mix", "", ""]);
        assert!(out[3].cue.is_none() && out[4].cue.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Run by hand: scans a real folder (CADENCE_SCAN_DIR) and lists the songs cue sheets made.
    #[test]
    #[ignore]
    fn real_scan() {
        let Ok(dir) = std::env::var("CADENCE_SCAN_DIR") else { return };
        let tracks = crate::library::scan(&[dir], Vec::new(), |_, _| {});
        let cue: Vec<&Track> = tracks.iter().filter(|t| t.cue.is_some()).collect();
        let mut files: Vec<&str> = cue.iter().map(|t| t.path.as_str()).collect();
        files.dedup();
        files.sort();
        files.dedup();
        println!("{} tracks, {} from cue sheets, out of {} files", tracks.len(), cue.len(), files.len());
        let mut worst = 0.0f64;
        for t in &cue {
            let c = t.cue.unwrap();
            worst = worst.max(c.end.map_or(0.0, |e| e - t.duration - c.start).abs());
        }
        println!("largest span/duration mismatch: {worst}");
        for t in cue.iter().take(14) {
            println!("  {:02} {} | {} | {} | {:.1}s from {:.1}", t.track_no.unwrap_or(0), t.title, t.artist, t.album, t.duration, t.cue.unwrap().start);
        }
        for f in files.iter().take(3) {
            println!("file: {f}");
        }
    }

    #[test]
    fn one_file_per_song_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("cadence-cue2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cue = dir.join("split.cue");
        std::fs::write(&cue, "FILE \"01.flac\" WAVE\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\nFILE \"02.flac\" WAVE\n TRACK 02 AUDIO\n  INDEX 01 00:00:00\n").unwrap();
        let mk = |name: &str| Track { id: name.into(), path: dir.join(name).to_string_lossy().into_owned(), duration: 200.0, ..Default::default() };
        let out = apply(vec![mk("01.flac"), mk("02.flac")], &[cue]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|t| t.cue.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
