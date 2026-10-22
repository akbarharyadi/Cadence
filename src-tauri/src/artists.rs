//! Artist pictures chosen by the user (an online photo, an album cover or an image file).
//! Without one, an artist is shown with their latest album's cover.
//!
//! Pictures are saved in the art folder (`art/artists/`) as a large and a small JPEG; which
//! artist has which is kept in `artist-images.json`, keyed by the artist's lower-case name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistImage {
    pub large: String,
    pub small: String,
}

/// A photo offered from the online catalogue.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Photo {
    pub name: String,
    pub picture: String,
    pub thumb: String,
    pub fans: u64,
}

/// Where a new picture comes from.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Source {
    Url { url: String },
    File { path: String },
}

fn index_file(data_dir: &Path) -> PathBuf {
    data_dir.join("artist-images.json")
}

pub fn all(data_dir: &Path) -> HashMap<String, ArtistImage> {
    crate::state::read_json(&index_file(data_dir))
}

/// Saves `source` as the picture for `artist`, replacing any earlier one.
pub fn set(data_dir: &Path, art_dir: &Path, artist: &str, source: Source) -> Result<ArtistImage, String> {
    let bytes = match source {
        Source::Url { url } => {
            if !url.starts_with("https://") {
                return Err("Only https addresses can be used".into());
            }
            download(&url)?
        }
        Source::File { path } => std::fs::read(&path).map_err(|e| format!("Couldn't read the image: {e}"))?,
    };
    let dir = art_dir.join("artists");
    let key = artist.trim().to_lowercase();
    // A new file name each time, so the interface never shows a cached old picture.
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let name = format!("{}-{stamp}", crate::library::short_hash(&key));
    let (large, small) = crate::library::write_art(&name, &bytes, &dir);
    let (Some(large), Some(small)) = (large, small) else {
        return Err("That file isn't an image Cadence can read (use JPEG, PNG or WebP).".into());
    };
    let image = ArtistImage { large, small };
    let mut index = all(data_dir);
    if let Some(old) = index.insert(key, image.clone()) {
        remove_files(&old);
    }
    crate::state::write_json(&index_file(data_dir), &index);
    Ok(image)
}

/// Goes back to the album cover.
pub fn reset(data_dir: &Path, artist: &str) {
    let mut index = all(data_dir);
    if let Some(old) = index.remove(&artist.trim().to_lowercase()) {
        remove_files(&old);
        crate::state::write_json(&index_file(data_dir), &index);
    }
}

fn remove_files(img: &ArtistImage) {
    let _ = std::fs::remove_file(&img.large);
    let _ = std::fs::remove_file(&img.small);
}

/// Photos of artists with this name in Deezer's catalogue (no account or key; only the name is sent).
pub fn search(artist: &str) -> Result<Vec<Photo>, String> {
    let url = format!("https://api.deezer.com/search/artist?q={}&limit=8", urlencoding::encode(artist));
    let v: Value = client()?
        .get(url)
        .send()
        .and_then(|r| r.json())
        .map_err(|_| "Couldn't reach the online catalogue. Check your internet connection.".to_string())?;
    let photos: Vec<Photo> = v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let picture = h["picture_xl"].as_str().filter(|s| !s.is_empty())?;
            // Deezer's stand-in for artists without a photo has an empty image id.
            if picture.contains("/artist//") {
                return None;
            }
            Some(Photo {
                name: h["name"].as_str().unwrap_or("").to_string(),
                picture: picture.to_string(),
                thumb: h["picture_medium"].as_str().unwrap_or(picture).to_string(),
                fans: h["nb_fan"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    // Only artists with this name (the search also returns artists who merely covered them).
    let want = letters(artist);
    let named: Vec<Photo> = photos
        .iter()
        .filter(|p| {
            let n = letters(&p.name);
            !n.is_empty() && (n.contains(&want) || want.contains(&n))
        })
        .cloned()
        .collect();
    Ok(if named.is_empty() { photos } else { named })
}

/// Letters and digits only, lower case: "Radio Head" and "Radiohead" compare equal.
fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn client() -> Result<reqwest::blocking::Client, String> {
    crate::net::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let r = client()?.get(url).send().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("The picture didn't download ({})", r.status()));
    }
    r.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// Live: `cargo test --lib artists -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn finds_and_saves_a_photo() {
        let photos = super::search("Radiohead").unwrap();
        for p in &photos {
            println!("{} ({} fans) {}", p.name, p.fans, p.picture);
        }
        assert!(photos.first().is_some_and(|p| p.name == "Radiohead"));
        let dir = std::env::temp_dir().join("cadence-artist-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = super::Source::Url { url: photos[0].picture.clone() };
        let img = super::set(&dir, &dir.join("art"), "Radiohead", src).unwrap();
        println!("saved {} and {}", img.large, img.small);
        assert!(std::path::Path::new(&img.small).exists());
        assert_eq!(super::all(&dir).len(), 1);
        super::reset(&dir, "radiohead");
        assert!(super::all(&dir).is_empty() && !std::path::Path::new(&img.large).exists());
    }
}
