//! Editing song details (tags) in the files themselves.

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{Tag, items::Timestamp};
use serde::Deserialize;

/// Fields to change. `None` leaves a field as it is; an empty string removes it.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TagChanges {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<String>,
    pub disc_no: Option<String>,
}

pub fn write(path: &Path, c: &TagChanges) -> Result<(), String> {
    let mut tagged = lofty::read_from_path(path)
        .map_err(|e| format!("Couldn't read {}: {e}", file_name(path)))?;
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tagged.primary_tag_type()));
    }
    let tag = tagged.primary_tag_mut().expect("tag was just inserted");

    let text = |tag: &mut Tag, key: ItemKey, v: &Option<String>| {
        if let Some(v) = v {
            let v = v.trim();
            if v.is_empty() {
                tag.remove_key(key);
            } else {
                tag.insert_text(key, v.to_string());
            }
        }
    };
    text(tag, ItemKey::TrackTitle, &c.title);
    text(tag, ItemKey::TrackArtist, &c.artist);
    text(tag, ItemKey::AlbumTitle, &c.album);
    text(tag, ItemKey::AlbumArtist, &c.album_artist);
    text(tag, ItemKey::Genre, &c.genre);

    let number = |v: &Option<String>| -> Result<Option<Option<u32>>, String> {
        match v.as_deref().map(str::trim) {
            None => Ok(None),
            Some("") => Ok(Some(None)),
            Some(s) => s
                .parse::<u32>()
                .map(|n| Some(Some(n)))
                .map_err(|_| format!("“{s}” isn’t a number")),
        }
    };
    match number(&c.year)? {
        Some(Some(y)) => tag.set_date(Timestamp {
            year: y as u16,
            ..Default::default()
        }),
        Some(None) => tag.remove_date(),
        None => {}
    }
    match number(&c.track_no)? {
        Some(Some(n)) => tag.set_track(n),
        Some(None) => tag.remove_track(),
        None => {}
    }
    match number(&c.disc_no)? {
        Some(Some(n)) => tag.set_disk(n),
        Some(None) => tag.remove_disk(),
        None => {}
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("Couldn't save {}: {e}", file_name(path)))
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
