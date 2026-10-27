//! The music library: imported folders, scanned into tracks and albums.
//!
//! Tags and audio properties come from `lofty`. Results are cached (keyed by file size + modified time)
//! so rescans only read new or changed files. Album art is taken from the embedded cover or a cover
//! image in the folder, resized once and stored on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::tag::Tag;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::audio::loudness::{GainInfo, SongGain, parse_tag_number};

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "alac", "aac", "wav", "aif", "aiff", "ogg", "oga", "mka",
];
const LOSSLESS: &[&str] = &["FLAC", "ALAC", "WAV", "AIFF", "PCM"];
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "artwork"];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Track {
    pub id: String,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub album_id: String,
    pub genre: String,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration: f64,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub bitrate: Option<u32>,
    pub codec: String,
    pub size: u64,
    pub modified: u64,
    /// Unix seconds when the file first appeared in the library.
    pub added: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_embedded_art: bool,
    /// Loudness (ReplayGain) from tags or Cadence's own measurement.
    pub gain: SongGain,
    /// The gain came from Cadence's measurement (not tags), so it's never re-read from the file.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gain_measured: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub genre: String,
    pub art: Option<String>,
    pub art_small: Option<String>,
    pub track_ids: Vec<String>,
    pub duration: f64,
    /// "Hi-Res Lossless", "Lossless" or the codec name, like Apple Music's badge.
    pub quality: String,
    /// Colours taken from the cover: a readable accent and a deep tint for backgrounds (hex).
    pub accent: Option<String>,
    pub tint: Option<String>,
    /// e.g. "24-bit/48 kHz FLAC".
    pub format: String,
    pub added: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct LibraryCache {
    pub tracks: Vec<Track>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
}

pub fn short_hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex()[..16].to_string()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A folder that lists no music although it had some: is it just unreadable right now (no
/// permission to read music on Android, a drive that isn't plugged in) rather than emptied?
/// Then its songs are kept, instead of the library losing them (and their measurements).
fn unreadable(folder: &Path, songs: &[Track]) -> bool {
    let Some(first) = songs.first() else { return false };
    if !folder.exists() {
        // A song added on its own whose folder is still there was deleted or moved.
        return !(is_audio(folder) && folder.parent().is_some_and(Path::exists));
    }
    match std::fs::metadata(&first.path) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Scans the folders, reusing cached entries for files that haven't changed.
pub fn scan(
    folders: &[String],
    previous: Vec<Track>,
    progress: impl Fn(usize, usize) + Sync,
) -> Vec<Track> {
    let started = std::time::Instant::now();
    let mut files: Vec<PathBuf> = Vec::new();
    // Songs of folders that can't be read right now: kept as they were.
    let mut kept: Vec<Track> = Vec::new();
    let mut previous = previous;
    for folder in folders {
        let found: Vec<PathBuf> = walkdir::WalkDir::new(folder)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_audio(e.path()))
            .map(|e| e.into_path())
            .collect();
        if found.is_empty() {
            let (mine, rest): (Vec<Track>, Vec<Track>) =
                previous.into_iter().partition(|t| Path::new(&t.path).starts_with(folder));
            previous = rest;
            if unreadable(Path::new(folder), &mine) {
                log::warn!("can't read {folder} right now; keeping its {} songs", mine.len());
                kept.extend(mine);
            }
        }
        files.extend(found);
    }
    let mut cached: HashMap<String, Track> =
        previous.into_iter().map(|t| (t.path.clone(), t)).collect();
    let total = files.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let walked = started.elapsed();
    let reread = std::sync::atomic::AtomicUsize::new(0);

    let jobs: Vec<(PathBuf, Option<Track>)> = files
        .into_iter()
        .map(|p| {
            let key = p.to_string_lossy().into_owned();
            let old = cached.remove(&key);
            (p, old)
        })
        .collect();

    let mut tracks: Vec<Track> = jobs
        .into_par_iter()
        .filter_map(|(path, old)| {
            let meta = std::fs::metadata(&path).ok()?;
            let size = meta.len();
            let modified = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n % 25 == 0 || n == total {
                progress(n, total);
            }
            match old {
                Some(t) if t.size == size && t.modified == modified => Some(t),
                old => {
                    reread.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let mut t = read_track(&path)?;
                    t.size = size;
                    t.modified = modified;
                    t.added = old.map(|o| o.added).unwrap_or_else(now);
                    Some(t)
                }
            }
        })
        .collect();
    log::info!(
        "found {total} songs in {walked:?}, checked them in {:?} ({} read again)",
        started.elapsed() - walked,
        reread.into_inner()
    );
    tracks.extend(kept);
    tracks.sort_by(|a, b| {
        (
            a.album_artist.to_lowercase(),
            a.album.to_lowercase(),
            a.disc_no,
            a.track_no,
            a.title.to_lowercase(),
        )
            .cmp(&(
                b.album_artist.to_lowercase(),
                b.album.to_lowercase(),
                b.disc_no,
                b.track_no,
                b.title.to_lowercase(),
            ))
    });
    tracks
}

/// Reads tags and audio properties of one file.
pub fn read_track(path: &Path) -> Option<Track> {
    let tagged = match lofty::read_from_path(path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("can't read tags of {}: {e}", path.display());
            return None;
        }
    };
    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let get = |f: &dyn Fn(&lofty::tag::Tag) -> Option<String>| {
        tag.and_then(f)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let path_str = path.to_string_lossy().into_owned();
    let folder = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title =
        get(&|t| t.title().map(|s| s.into_owned())).unwrap_or_else(|| title_from_file(path));
    let artist =
        get(&|t| t.artist().map(|s| s.into_owned())).unwrap_or_else(|| "Unknown Artist".into());
    let album = get(&|t| t.album().map(|s| s.into_owned())).unwrap_or_else(|| folder.clone());
    let album_artist = get(&|t| t.get_string(ItemKey::AlbumArtist).map(str::to_string))
        .unwrap_or_else(|| artist.clone());
    let genre = get(&|t| t.genre().map(|s| s.into_owned())).unwrap_or_default();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let codec = codec_name(&ext, tagged.file_type());

    // Albums are grouped by album artist + title (so two artists' "Greatest Hits" stay apart).
    let album_key = format!("{}|{}", album_artist.to_lowercase(), album.to_lowercase());
    Some(Track {
        id: short_hash(&path_str),
        path: path_str,
        album_id: short_hash(&album_key),
        title,
        artist,
        album,
        album_artist,
        genre,
        year: tag.and_then(|t| t.date()).map(|d| d.year as u32),
        track_no: tag.and_then(|t| t.track()),
        disc_no: tag.and_then(|t| t.disk()),
        duration: props.duration().as_secs_f64(),
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth(),
        channels: props.channels(),
        bitrate: props.audio_bitrate(),
        codec,
        has_embedded_art: tag.is_some_and(|t| !t.pictures().is_empty()),
        gain: tag.map(replay_gain_tags).unwrap_or_default(),
        ..Default::default()
    })
}

/// ReplayGain values written by other tools (foobar2000, beets, loudgain, Mp3tag...).
fn replay_gain_tags(tag: &Tag) -> SongGain {
    let num = |key: ItemKey| tag.get_string(key).and_then(parse_tag_number);
    let pair = |gain: ItemKey, peak: ItemKey| {
        num(gain).map(|g| GainInfo {
            gain: g,
            peak: num(peak).filter(|p| *p > 0.0).unwrap_or(1.0),
        })
    };
    SongGain {
        track: pair(ItemKey::ReplayGainTrackGain, ItemKey::ReplayGainTrackPeak),
        album: pair(ItemKey::ReplayGainAlbumGain, ItemKey::ReplayGainAlbumPeak),
    }
}

fn codec_name(ext: &str, file_type: lofty::file::FileType) -> String {
    use lofty::file::FileType;
    match file_type {
        FileType::Flac => "FLAC".into(),
        FileType::Mpeg => "MP3".into(),
        FileType::Wav => "WAV".into(),
        FileType::Aiff => "AIFF".into(),
        FileType::Vorbis => "Vorbis".into(),
        FileType::Opus => "Opus".into(),
        FileType::Mp4 => {
            if ext == "alac" {
                "ALAC".into()
            } else {
                "AAC".into()
            }
        }
        _ => ext.to_uppercase(),
    }
}

/// "01. A Small Miracle 小小奇迹" → "A Small Miracle 小小奇迹".
fn title_from_file(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let trimmed = stem
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches(['.', '-', ' ', '_']);
    if trimmed.is_empty() {
        stem
    } else {
        trimmed.to_string()
    }
}

/// Groups tracks into albums and makes sure each album has cached artwork.
pub fn build_albums(tracks: &mut [Track], art_dir: &Path) -> Vec<Album> {
    // MP4 files store ALAC and AAC in the same container; lofty reports bit depth only for lossless.
    for t in tracks.iter_mut() {
        if t.codec == "AAC" && t.bit_depth.is_some() {
            t.codec = "ALAC".into();
        }
    }
    let mut groups: Vec<(String, Vec<&Track>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for t in tracks.iter() {
        match index.get(t.album_id.as_str()) {
            Some(&i) => groups[i].1.push(t),
            None => {
                index.insert(&t.album_id, groups.len());
                groups.push((t.album_id.clone(), vec![t]));
            }
        }
    }
    let _ = std::fs::create_dir_all(art_dir);
    let mut albums: Vec<Album> = groups
        .par_iter()
        .map(|(id, list)| {
            let mut list = list.clone();
            list.sort_by_key(|t| (t.disc_no.unwrap_or(1), t.track_no.unwrap_or(u32::MAX)));
            let first = list[0];
            let (art, art_small) = ensure_art(id, &list, art_dir);
            let (accent, tint) = art_small
                .as_deref()
                .and_then(|p| saved_colors(id, Path::new(p), art_dir))
                .unzip();
            let max_rate = list.iter().filter_map(|t| t.sample_rate).max();
            let max_bits = list.iter().filter_map(|t| t.bit_depth).max();
            let codec = first.codec.clone();
            let lossless = list.iter().all(|t| LOSSLESS.contains(&t.codec.as_str()));
            // Apple Music's rule: lossless above 48 kHz is "Hi-Res Lossless".
            let quality = if lossless {
                if max_rate.unwrap_or(0) > 48_000 {
                    "Hi-Res Lossless"
                } else {
                    "Lossless"
                }
                .to_string()
            } else {
                codec.clone()
            };
            let format = match (max_bits, max_rate) {
                (Some(b), Some(r)) => format!("{b}-bit/{} {codec}", khz(r)),
                (None, Some(r)) => format!("{} {codec}", khz(r)),
                _ => codec.clone(),
            };
            Album {
                id: id.clone(),
                title: first.album.clone(),
                artist: first.album_artist.clone(),
                year: list.iter().filter_map(|t| t.year).max(),
                genre: first.genre.clone(),
                art,
                art_small,
                track_ids: list.iter().map(|t| t.id.clone()).collect(),
                duration: list.iter().map(|t| t.duration).sum(),
                quality,
                format,
                added: list.iter().map(|t| t.added).max().unwrap_or(0),
                accent,
                tint,
            }
        })
        .collect();
    albums.sort_by_key(|a| (a.artist.to_lowercase(), a.year, a.title.to_lowercase()));
    albums
}

/// Picks an accent and a deep background tint from cover art.
///
/// The accent is the cover's dominant *vivid* hue (a 24-bin hue histogram weighted by saturation and
/// mid-lightness), made readable on dark backgrounds. A small vivid detail (eyes, a logo) doesn't win
/// over the colour that fills the cover. Gold-ish hues give way to the runner-up when it's close, so the
/// accent stays distinct from the gold "Lossless" badge.
/// An album's cover colours, worked out once and kept next to its artwork (`write_art` drops them):
/// decoding every cover again would slow every start.
fn saved_colors(album_id: &str, small: &Path, art_dir: &Path) -> Option<(String, String)> {
    let saved = art_dir.join(format!("{album_id}.colors"));
    if let Ok(text) = std::fs::read_to_string(&saved)
        && let Some((accent, tint)) = text.trim().split_once(' ')
    {
        return Some((accent.to_string(), tint.to_string()));
    }
    let colors = cover_colors(small)?;
    let _ = std::fs::write(&saved, format!("{} {}", colors.0, colors.1));
    Some(colors)
}

fn cover_colors(path: &Path) -> Option<(String, String)> {
    const BINS: usize = 24;
    let img = image::open(path).ok()?.thumbnail(64, 64).to_rgb8();
    let mut weight = [0f32; BINS];
    let mut hue_x = [0f32; BINS];
    let mut hue_y = [0f32; BINS];
    let mut sat = [0f32; BINS];
    let (mut all_x, mut all_y, mut all_w) = (0f32, 0f32, 0f32);
    for p in img.pixels() {
        let (h, s, l) = rgb_to_hsl(p[0], p[1], p[2]);
        let w = s * s * (1.0 - (l - 0.55).abs() * 1.8).max(0.0);
        if w <= 0.0 {
            continue;
        }
        let bin = ((h * BINS as f32) as usize).min(BINS - 1);
        let (x, y) = (
            (h * std::f32::consts::TAU).cos(),
            (h * std::f32::consts::TAU).sin(),
        );
        weight[bin] += w;
        hue_x[bin] += w * x;
        hue_y[bin] += w * y;
        sat[bin] += w * s;
        all_x += w * x;
        all_y += w * y;
        all_w += w;
    }
    if all_w < 1e-3 {
        // Black-and-white cover: neutral accent, neutral tint.
        return Some(("#b9bcd6".into(), "#191a24".into()));
    }
    // Neighbouring bins belong together; score each bin with its neighbours.
    let score =
        |i: usize| weight[i] + 0.5 * (weight[(i + BINS - 1) % BINS] + weight[(i + 1) % BINS]);
    let mut order: Vec<usize> = (0..BINS).collect();
    order.sort_by(|a, b| score(*b).total_cmp(&score(*a)));
    // Hues between 30° and 75° read as gold.
    let is_gold = |i: usize| (30..75).contains(&(i * 360 / BINS));
    let mut pick = order[0];
    if is_gold(pick) && score(order[1]) > score(pick) * 0.45 {
        pick = order[1];
    }
    let hue = (hue_y[pick].atan2(hue_x[pick]) / std::f32::consts::TAU).rem_euclid(1.0);
    let s = (sat[pick] / weight[pick]).clamp(0.45, 0.85);
    let accent = hsl_to_hex(hue, s, 0.72);
    let avg_h = (all_y.atan2(all_x) / std::f32::consts::TAU).rem_euclid(1.0);
    let tint = hsl_to_hex(avg_h, 0.45, 0.15);
    Some((accent, tint))
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn hsl_to_hex(h: f32, s: f32, l: f32) -> String {
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(h + 1.0 / 3.0),
        channel(h),
        channel(h - 1.0 / 3.0)
    )
}

pub fn khz(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", rate as f64 / 1000.0)
    }
}

/// Returns (large, small) artwork paths, creating them from the embedded cover or a folder image.
fn ensure_art(
    album_id: &str,
    tracks: &[&Track],
    art_dir: &Path,
) -> (Option<String>, Option<String>) {
    let large = art_dir.join(format!("{album_id}.jpg"));
    let small = art_dir.join(format!("{album_id}_s.jpg"));
    let as_str = |p: &Path| Some(p.to_string_lossy().into_owned());
    if large.exists() && small.exists() {
        return (as_str(&large), as_str(&small));
    }
    // Looking for a cover reads the songs and lists their folder, slow on a phone's shared storage:
    // an album known to have none isn't searched again until its songs or folder change.
    let none = art_dir.join(format!("{album_id}.none"));
    let key = cover_search_key(tracks);
    if std::fs::read_to_string(&none).is_ok_and(|k| k == key) {
        return (None, None);
    }
    let Some(bytes) = embedded_cover(tracks).or_else(|| folder_cover(tracks)) else {
        let _ = std::fs::write(&none, key);
        return (None, None);
    };
    write_art(album_id, &bytes, art_dir)
}

/// What a cover search depends on: the album's songs and its folder (where a cover image may be added).
fn cover_search_key(tracks: &[&Track]) -> String {
    let mut parts: Vec<String> = tracks
        .iter()
        .map(|t| format!("{}|{}|{}|{}", t.path, t.size, t.modified, t.has_embedded_art))
        .collect();
    parts.sort();
    let folder = tracks
        .first()
        .and_then(|t| Path::new(&t.path).parent().map(Path::to_path_buf))
        .and_then(|d| std::fs::metadata(d).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    parts.push(folder.to_string());
    short_hash(&parts.join("\n"))
}

/// The album id for an album artist and title (see `album_key` in `read_track`).
pub fn album_id_for(album_artist: &str, album: &str) -> String {
    short_hash(&format!("{}|{}", album_artist.to_lowercase(), album.to_lowercase()))
}

/// Saves a cover image as an album's large and small artwork.
pub fn write_art(album_id: &str, bytes: &[u8], art_dir: &Path) -> (Option<String>, Option<String>) {
    let large = art_dir.join(format!("{album_id}.jpg"));
    let small = art_dir.join(format!("{album_id}_s.jpg"));
    let as_str = |p: &Path| Some(p.to_string_lossy().into_owned());
    let _ = std::fs::create_dir_all(art_dir);
    // A new cover: its colours are worked out again, and the album has a cover now.
    let _ = std::fs::remove_file(art_dir.join(format!("{album_id}.colors")));
    let _ = std::fs::remove_file(art_dir.join(format!("{album_id}.none")));
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let save = |size: u32, path: &Path| -> bool {
                let resized = if img.width() > size || img.height() > size {
                    img.resize(size, size, image::imageops::FilterType::Lanczos3)
                } else {
                    img.clone()
                };
                let rgb = resized.to_rgb8();
                let Ok(file) = std::fs::File::create(path) else {
                    return false;
                };
                let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
                    std::io::BufWriter::new(file),
                    90,
                );
                enc.encode_image(&rgb).is_ok()
            };
            let ok_large = save(1200, &large);
            let ok_small = save(360, &small);
            (
                if ok_large { as_str(&large) } else { None },
                if ok_small { as_str(&small) } else { None },
            )
        }
        Err(e) => {
            log::warn!("can't decode artwork for album {album_id}: {e}");
            (None, None)
        }
    }
}

fn embedded_cover(tracks: &[&Track]) -> Option<Vec<u8>> {
    for t in tracks.iter().filter(|t| t.has_embedded_art).take(3) {
        let Ok(tagged) = lofty::read_from_path(&t.path) else {
            continue;
        };
        for tag in tagged.tags() {
            let pics = tag.pictures();
            if let Some(p) = pics
                .iter()
                .find(|p| p.pic_type() == PictureType::CoverFront)
                .or(pics.first())
                .filter(|p| complete_image(p.data()))
            {
                return Some(p.data().to_vec());
            }
        }
    }
    None
}

/// Whether picture data is whole. Some taggers store cut-off JPEGs; decoders then fill the missing
/// part with grey, so a cover in the folder (or another song's) is the better choice.
fn complete_image(data: &[u8]) -> bool {
    if data.starts_with(&[0xFF, 0xD8]) {
        // The end-of-image marker, allowing for a little padding after it.
        let tail = &data[data.len().saturating_sub(64)..];
        return tail.windows(2).any(|w| w == [0xFF, 0xD9]);
    }
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        return data.windows(4).rev().take(64).any(|w| w == b"IEND");
    }
    !data.is_empty()
}

fn folder_cover(tracks: &[&Track]) -> Option<Vec<u8>> {
    let dir = Path::new(&tracks.first()?.path).parent()?;
    let images: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png"))
        })
        .collect();
    let named = images.iter().find(|p| {
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        COVER_NAMES
            .iter()
            .any(|n| stem == *n || stem.starts_with(n))
    });
    // Otherwise the biggest image in the folder is most likely the cover.
    let pick = named.or_else(|| {
        images
            .iter()
            .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
    })?;
    std::fs::read(pick).ok()
}

#[cfg(test)]
mod art_tests {
    #[test]
    fn cut_off_jpegs_are_rejected() {
        assert!(super::complete_image(&[0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9]));
        assert!(super::complete_image(&[0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9, 0, 0]));
        assert!(!super::complete_image(&[0xFF, 0xD8, 1, 2, 3, 4, 5]));
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    fn song(path: &Path) -> Track {
        Track { id: "x".into(), path: path.to_string_lossy().into_owned(), gain_measured: true, ..Default::default() }
    }

    #[test]
    fn keeps_songs_of_folders_it_cannot_read() {
        let root = std::env::temp_dir().join("cadence-scan-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A drive that isn't plugged in: its songs stay.
        let gone = root.join("usb-drive");
        let kept = scan(&[gone.to_string_lossy().into_owned()], vec![song(&gone.join("a.flac"))], |_, _| {});
        assert_eq!(kept.len(), 1);
        assert!(kept[0].gain_measured);
        // The folder lists nothing, but the file is still there (no permission to list it): stays.
        let hidden = root.join("hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        let file = hidden.join("b.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(scan(&[hidden.to_string_lossy().into_owned()], vec![song(&file)], |_, _| {}).len(), 1);
        // The songs were really deleted: they go.
        let emptied = root.join("emptied");
        std::fs::create_dir_all(&emptied).unwrap();
        assert!(scan(&[emptied.to_string_lossy().into_owned()], vec![song(&emptied.join("c.flac"))], |_, _| {}).is_empty());
        // A song added on its own (opened from another app) and then deleted: it goes...
        let opened = emptied.join("d.flac");
        assert!(scan(&[opened.to_string_lossy().into_owned()], vec![song(&opened)], |_, _| {}).is_empty());
        // ...but not when the drive it was on is missing.
        let away = gone.join("e.flac");
        assert_eq!(scan(&[away.to_string_lossy().into_owned()], vec![song(&away)], |_, _| {}).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remembers_covers_found_and_not_found() {
        let root = std::env::temp_dir().join("cadence-cover-test");
        let _ = std::fs::remove_dir_all(&root);
        let (music, art) = (root.join("album"), root.join("art"));
        std::fs::create_dir_all(&music).unwrap();
        let mut tracks = vec![Track { album_id: "a1".into(), ..song(&music.join("01.flac")) }];
        // No cover anywhere: noted, so the next start doesn't search again.
        assert!(build_albums(&mut tracks, &art)[0].art.is_none());
        assert!(art.join("a1.none").exists());
        assert!(build_albums(&mut tracks, &art)[0].art.is_none());
        // A cover image added to the folder changes it: found, and its colours kept with it.
        let red = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 40]));
        red.save(music.join("cover.jpg")).unwrap();
        let album = build_albums(&mut tracks, &art).remove(0);
        assert!(album.art.is_some() && album.accent.is_some());
        assert!(!art.join("a1.none").exists());
        let saved = std::fs::read_to_string(art.join("a1.colors")).unwrap();
        assert_eq!(saved, format!("{} {}", album.accent.unwrap(), album.tint.unwrap()));
        // A new cover (set by hand or fetched) drops the saved colours.
        write_art("a1", &std::fs::read(music.join("cover.jpg")).unwrap(), &art);
        assert!(!art.join("a1.colors").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
