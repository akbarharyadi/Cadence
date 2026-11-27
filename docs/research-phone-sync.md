# Sync with phone (2026-11-28)

## The question

How does Cadence put the music you want onto the phone, keep it current, and keep listening data (plays,
favorites, ratings, skip marks, playlists) the same on both devices?

## What the owner chose

- **Transport:** the phone's Cadence pulls from the PC's Cadence over the network. Over Wi-Fi, or over the cable
  with USB tethering turned on (the same code path: tethering makes the cable a network).
- **What goes on the phone:** playlists, albums and artists marked "Keep on phone", plus an optional "Top 100 /
  200 / 500 most played". It stays in sync: changes follow, and anything no longer chosen is removed from the
  phone.
- **Files:** sent unchanged, bit-perfect (no conversion).
- **Listening data:** both ways.

## What the code has today (facts the design rests on)

- A song's id is a hash of its full path (`library.rs`, `short_hash(path)`), so the same file has different ids
  on the PC and the phone. Every piece of per-song data is keyed by that id: history events, marks
  (favorites/ratings, which also keep title/artist/album/duration), skip marks (`Settings.skipped`), and
  playlists (`track_ids`).
- A strict matcher already exists for ids that changed (`state.rs` `Finder`, `moved_songs`; used by backup
  restore in `backup::relink` and `follow_everywhere`): same title, same artist (identity-aware), length within
  2.5 s, ambiguous means no match.
- Android: the library scans `Settings.folders` (default `/storage/emulated/0/Music`) and watches them, so new
  files are picked up. The app may create its own files under `Music/` (scoped storage) and delete them, but no
  others. `INTERNET` is declared; Wi-Fi state, multicast and data-sync foreground service permissions are not.
  The only foreground service is `PlaybackService` (media playback), so a long transfer while paused could be
  killed.
- No HTTP server crate is in the build (reqwest is the only network crate). No device or install id exists.

## Design

### Pairing and transport

- Settings → "Phone sync" on the PC: off by default (it opens a port). When on, the PC listens on its local
  network address (port 47300 or the next free one) and shows the address, a 6-digit code, and a QR code.
- The phone enters (or scans) the address and code once. The PC answers with a random token, kept in each side's
  keyring (`secrets.rs`). Every request carries the token; requests from outside private address ranges
  (192.168/16, 10/8, 172.16/12, the tethering ranges) are refused.
- Plain HTTP on the local network, with the token and a hash check on every file. TLS can come later.
- New dependency: `tiny_http` (small, blocking, no async runtime; fits the code's threads). The PC side only.
- Each install gets a random device id (saved in settings) so the PC can list paired phones and their last sync.

### What to send

- `phone.json` on the PC (user decisions, so it goes in backups): kept playlist ids, album ids, artist keys
  (identity keys, so 藍井エイル and Eir Aoi are one), and `top: 0 | 100 | 200 | 500`.
- Top N: by play count from history over the last 12 months, songs played past halfway only.
- A smart playlist keeps following its rules; a kept album or artist includes songs added later.
- Skip-marked songs are left out of artists and Top N, but kept when they are in a kept playlist or album (being
  in a playlist is an explicit choice).
- UI: a "Keep on phone" toggle in the playlist, album and artist menus; a Phone sync page in Settings with the
  chosen items, the Top N choice, the total size against the phone's free space, and the last sync.

### Files

- The PC serves a manifest: for each song to send, a **song key**, its size, a path under `Music/Cadence/`
  (`Artist/Album/NN Title.flac`, made safe for Android file names), and the tags needed to match it.
- Song key: blake3 of the file's audio bytes (the whole file, computed once per file on the PC in a background
  job and cached by path, size and modified time). It's the same for a byte-identical copy anywhere, so it ties
  the phone's copy to the PC's song without trusting names.
- The phone keeps a ledger (`phone-sync.json`, app data): song key → local path, size, the PC it came from. It
  downloads missing files (resumable with HTTP Range, written to a `.part` file, checked by size and hash, then
  renamed), and deletes only files in its ledger that are no longer in the manifest. Files the user put on the
  phone some other way are never touched.
- Before a sync, the phone checks free space and stops with a clear message if it won't fit.
- On Android the transfer runs in a new data-sync foreground service (notification "Syncing music: 12 of 340"),
  so it survives the screen going off. It's a background job in the sidebar, and it can be paused.

### Listening data, both ways

- Both sides exchange changes since the last sync, translated through song keys (PC id ↔ key ↔ phone id). Songs
  only on one side are matched by the existing strict matcher, or skipped.
- History: events are added, never edited. Merge by (song, start time); a pair already present is skipped.
- Favorites, ratings and skip marks: last change wins. Needs a `changed` time on each mark and skip entry (new
  `serde(default)` fields; old entries count as older than any new change).
- Playlists: last change wins per playlist (`updated` exists). Deleting on one side deletes on the other
  (tombstones kept 90 days).
- Each side saves only through the existing atomic routines (`write_json`, `save_*`).

## Phases (each a separate change with its own tests)

1. PC: song keys job, `phone.json` and "Keep on phone" / Top N UI, the manifest, the server with pairing.
2. Phone: pairing screen, pulling files (ledger, resume, hash check, removal), foreground service, free space.
3. Listening data both ways (mark/skip timestamps, history merge, playlist merge).
4. Polish: QR code, sync on connect when the phone is charging and on Wi-Fi (setting, off by default).

## As built

- Phase 1 (computer): `src-tauri/src/phone/` `mod.rs` (choices), `select.rs`, `keys.rs`, `manifest.rs`,
  `server.rs`, `commands.rs`; Settings › Sync with phone; "Keep on Phone" in the playlist, album and artist
  menus. Smart playlists and single-file (CUE) albums aren't sent yet; the page counts them.
- Phase 2 (phone): `plan.rs` (what a sync does; only ledger files are ever removed; network paths checked),
  `client.rs` (pairing, the list, a download resumed from its `.part` file and checked against its key),
  `sync_job.rs` (the job, free space, a file in the way is never written over), Settings › Sync with computer,
  and `SyncService.kt` (data-sync foreground service, CPU and Wi-Fi awake, stops itself after 3 hours). The
  window drives the service from the progress events, so with the screen off its notification may lag; the copying
  itself doesn't depend on the window. The ledger is `phone-sync.json` (not backed up: it describes this phone).

## Open points for the owner

- New dependency `tiny_http` on the PC side (and a QR code crate in phase 4).
- The README privacy table needs a row: the PC opens a local network port while Phone sync is on.
