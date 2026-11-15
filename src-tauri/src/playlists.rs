//! Playlists: hand-made lists of songs, and smart playlists that fill themselves from rules.
//!
//! The backend only stores them; smart rules are evaluated in the UI, which already has the whole
//! library and listening history in memory (so the result updates instantly as you edit rules).

use std::collections::HashMap;

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
    /// Inserts or replaces by id. New playlists get an id and timestamps; a save that changes
    /// nothing (adding songs that were all there already, say) keeps the time it last changed.
    pub fn upsert(&mut self, mut p: Playlist, now: u64) -> Playlist {
        if p.id.is_empty() {
            p.id = crate::library::short_hash(&format!("{}{now}{}", p.name, self.items.len()));
            p.created = now;
        }
        match self.items.iter_mut().find(|x| x.id == p.id) {
            Some(existing) => {
                let same = existing.name == p.name && existing.track_ids == p.track_ids && existing.smart == p.smart;
                p.updated = if same { existing.updated } else { now };
                if p.created == 0 {
                    p.created = existing.created;
                }
                *existing = p.clone();
            }
            None => {
                p.updated = now;
                self.items.push(p.clone());
            }
        }
        p
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|p| p.id != id);
    }

    /// Points songs that moved (old id → new id) at their new ids, in place. Returns how many
    /// entries changed. Songs not in `moved` are left as they are.
    pub fn follow(&mut self, moved: &HashMap<String, String>) -> usize {
        let mut changed = 0;
        for p in &mut self.items {
            for id in &mut p.track_ids {
                if let Some(new) = moved.get(id.as_str())
                    && new != id {
                        *id = new.clone();
                        changed += 1;
                    }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(ids: &[&str]) -> Playlist {
        Playlist { name: "Mine".into(), track_ids: ids.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn a_save_that_changes_nothing_keeps_its_time() {
        let mut all = Playlists::default();
        let p = all.upsert(list(&["a", "b"]), 100);
        assert_eq!((p.created, p.updated), (100, 100));
        let same = all.upsert(Playlist { id: p.id.clone(), ..list(&["a", "b"]) }, 200);
        assert_eq!(same.updated, 100);
        let more = all.upsert(Playlist { id: p.id.clone(), ..list(&["a", "b", "c"]) }, 300);
        assert_eq!((more.created, more.updated), (100, 300));
        assert_eq!(all.items.len(), 1);
    }

    #[test]
    fn songs_follow_their_new_ids_and_missing_ones_stay() {
        let mut all = Playlists::default();
        all.upsert(list(&["old", "kept", "missing", "old"]), 1);
        let moved: HashMap<String, String> = [("old".to_string(), "new".to_string())].into();
        assert_eq!(all.follow(&moved), 2);
        assert_eq!(all.items[0].track_ids, ["new", "kept", "missing", "new"]);
    }
}
