//! Sync with phone: the music the listener keeps on their phone, and (on the computer) the
//! local network service the phone's Cadence pulls it from. See docs/research-phone-sync.md.
//!
//! What to keep on the phone is the listener's decision, saved in `phone.json` (and in backups):
//! playlists, albums and artists marked "Keep on phone", and the N most played songs. The songs
//! are worked out from that whenever they're needed (`select`), so a kept album or artist includes
//! songs added later.

pub mod commands;
pub mod keys;
pub mod manifest;
pub mod select;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod server;

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::library::Track;

/// The file of the listener's choices (in the data folder).
pub const FILE: &str = "phone.json";

/// How many of the most played songs can be kept on the phone.
pub const TOP_CHOICES: [u32; 4] = [0, 100, 200, 500];

/// What to keep on the phone, and the phones paired with this computer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Choice {
    /// Hand-made playlists, by id.
    pub playlists: BTreeSet<String>,
    /// Albums, by album id (made from the tags, so the same on every computer).
    pub albums: BTreeSet<String>,
    /// Artists, by their identity key (every spelling of an artist is one key).
    pub artists: BTreeSet<String>,
    /// The most played songs to keep too (0: none).
    pub top: u32,
    /// Phones paired with this computer (their tokens are in the keyring, see `server`).
    pub phones: Vec<Phone>,
}

/// A phone paired with this computer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Phone {
    /// The phone's own random id.
    pub id: String,
    /// Its name, as it gave it ("Pixel 8").
    pub name: String,
    /// Unix seconds.
    pub paired: u64,
    /// Unix seconds of the last time it asked for the music list (0: never).
    pub last_seen: u64,
}

/// One change of `phone.json` at a time (the window's choices and the server's notes).
static LOCK: Mutex<()> = Mutex::new(());

pub fn load(data_dir: &Path) -> Choice {
    let _one = LOCK.lock();
    crate::state::read_json(&data_dir.join(FILE))
}

/// Reads the choices, changes them with `f` and saves them, with no other change in between.
pub fn change<R>(data_dir: &Path, f: impl FnOnce(&mut Choice) -> R) -> R {
    let _one = LOCK.lock();
    let path = data_dir.join(FILE);
    let mut c: Choice = crate::state::read_json(&path);
    let before = c.clone();
    let out = f(&mut c);
    if c != before {
        crate::state::write_json(&path, &c);
    }
    out
}

/// What kind of thing is kept on the phone.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Playlist,
    Album,
    Artist,
}

/// Marks a playlist, album or artist to keep on the phone, or not. `id` is a playlist or album id,
/// or an artist's name as shown (any spelling of the artist).
pub fn keep(c: &mut Choice, kind: Kind, id: &str, on: bool) {
    let (set, id) = match kind {
        Kind::Playlist => (&mut c.playlists, id.to_string()),
        Kind::Album => (&mut c.albums, id.to_string()),
        Kind::Artist => (&mut c.artists, crate::identity::current().primary_key(id)),
    };
    if id.is_empty() {
        return;
    }
    if on {
        set.insert(id);
    } else {
        set.remove(&id);
    }
}

/// The nearest allowed "most played" count (anything else from the window becomes 0).
pub fn top_choice(n: u32) -> u32 {
    if TOP_CHOICES.contains(&n) { n } else { 0 }
}

/// What the settings page shows: the choices and what they come to.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub choice: Choice,
    pub songs: usize,
    pub bytes: u64,
    /// Songs whose key is still being worked out (they're sent once it is).
    pub pending: usize,
    pub left_out: usize,
    pub not_sent_playlists: usize,
}

/// The chosen songs, as library tracks (cloned: the library's lock isn't held after).
fn chosen(ctx: &Ctx) -> (Choice, Vec<Track>, select::Picked) {
    let choice = load(&ctx.state.data_dir);
    let playlists = ctx.state.playlists.lock().items.clone();
    let events = ctx.state.history.lock().events.clone();
    let skipped = ctx.state.settings.lock().skipped.clone();
    let ids = crate::identity::current();
    let lib = ctx.state.library.lock();
    let tracks = &lib.snapshot.tracks;
    let input = select::Input { tracks, playlists: &playlists, events: &events, skipped: &skipped, ids: &ids, now: now() };
    let picked = select::pick(&choice, &input);
    let by_id: HashMap<&str, &Track> = tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    let songs = picked.songs.iter().filter_map(|id| by_id.get(id.as_str()).map(|t| (*t).clone())).collect();
    (choice, songs, picked)
}

pub fn summary(ctx: &Ctx) -> Summary {
    let (choice, songs, picked) = chosen(ctx);
    let refs: Vec<&Track> = songs.iter().collect();
    let keys = keys::known(&ctx.state.data_dir, &refs);
    Summary {
        choice,
        songs: songs.len(),
        bytes: songs.iter().map(|t| t.size).sum(),
        pending: songs.iter().filter(|t| !keys.contains_key(&t.id)).count(),
        left_out: picked.left_out,
        not_sent_playlists: picked.not_sent_playlists,
    }
}

/// The music list for a phone, and the files it names (key → path). Starts working out the keys
/// the list still lacks.
pub fn manifest_now(ctx: &Ctx) -> (manifest::Manifest, HashMap<String, String>) {
    let (_, songs, picked) = chosen(ctx);
    let refs: Vec<&Track> = songs.iter().collect();
    let keys = keys::known(&ctx.state.data_dir, &refs);
    let m = manifest::build(&computer_name(), now(), &refs, &keys, picked.left_out);
    let files = songs.iter().filter_map(|t| Some((keys.get(&t.id)?.clone(), t.path.clone()))).collect();
    let missing: Vec<String> = songs.iter().filter(|t| !keys.contains_key(&t.id)).map(|t| t.id.clone()).collect();
    if !missing.is_empty() {
        keys::prepare(ctx, &missing);
    }
    (m, files)
}

/// Starts working out the keys of the chosen songs now (after the choices change), so the phone
/// doesn't wait for them.
pub fn prepare_keys(ctx: &Ctx) {
    let (_, songs, _) = chosen(ctx);
    let ids: Vec<String> = songs.into_iter().map(|t| t.id).collect();
    keys::prepare(ctx, &ids);
}

pub fn computer_name() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Unix seconds now.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeping_and_letting_go_of_an_album() {
        let mut c = Choice::default();
        keep(&mut c, Kind::Album, "a1", true);
        keep(&mut c, Kind::Playlist, "p1", true);
        assert!(c.albums.contains("a1") && c.playlists.contains("p1"));
        keep(&mut c, Kind::Album, "a1", false);
        assert!(c.albums.is_empty());
        keep(&mut c, Kind::Album, "", true);
        assert!(c.albums.is_empty(), "an empty id is not kept");
    }

    #[test]
    fn an_artist_is_kept_under_one_key_for_every_spelling() {
        crate::identity::set_for_test(Some(crate::identity::Identity::from_pairs(&[("藍井エイル", "Eir Aoi")])));
        let mut c = Choice::default();
        keep(&mut c, Kind::Artist, "Eir Aoi", true);
        keep(&mut c, Kind::Artist, "藍井エイル", true);
        assert_eq!(c.artists.len(), 1);
        keep(&mut c, Kind::Artist, "藍井エイル", false);
        assert!(c.artists.is_empty());
        crate::identity::set_for_test(None);
    }

    #[test]
    fn only_the_offered_top_counts_are_taken() {
        assert_eq!(top_choice(200), 200);
        assert_eq!(top_choice(7), 0);
    }

    #[test]
    fn choices_are_saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        change(dir.path(), |c| {
            c.top = 100;
            keep(c, Kind::Playlist, "p1", true);
        });
        let c = load(dir.path());
        assert_eq!(c.top, 100);
        assert!(c.playlists.contains("p1"));
    }

    #[test]
    fn an_old_or_partial_file_still_loads() {
        let c: Choice = serde_json::from_str(r#"{"albums":["a"]}"#).unwrap();
        assert_eq!(c.top, 0);
        assert!(c.albums.contains("a") && c.phones.is_empty());
    }
}
