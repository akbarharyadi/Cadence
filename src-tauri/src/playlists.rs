//! Playlists: hand-made lists of songs, and smart playlists that fill themselves from rules.
//!
//! The backend only stores them; smart rules are evaluated in the UI, which already has the whole
//! library and listening history in memory (so the result updates instantly as you edit rules).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    /// Hand-made playlists: the songs, in order.
    pub track_ids: Vec<String>,
    /// Smart playlists: the rules (shape owned by the UI, stored as-is).
    pub smart: Option<Value>,
    pub created: u64,
    pub updated: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Playlists {
    pub items: Vec<Playlist>,
}

impl Playlists {
    /// Inserts or replaces by id. New playlists get an id and timestamps.
    pub fn upsert(&mut self, mut p: Playlist, now: u64) -> Playlist {
        if p.id.is_empty() {
            p.id = crate::library::short_hash(&format!("{}{now}{}", p.name, self.items.len()));
            p.created = now;
        }
        p.updated = now;
        match self.items.iter_mut().find(|x| x.id == p.id) {
            Some(existing) => *existing = p.clone(),
            None => self.items.push(p.clone()),
        }
        p
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|p| p.id != id);
    }
}
