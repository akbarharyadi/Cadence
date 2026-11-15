# Audit and improvement research (2026-11-15)

Twelve subagent passes: six read-only code audits of every menu and control, six research passes that
opened and read primary pages (about 250 in total). Code findings were traced in source, not clicked
through in a running app. Evidence strength is marked: **strong** (primary page or code read),
**medium** (several weak signals), **weak** (one source or inference).

Not reachable by the fetcher, so thinly covered: Reddit, Hydrogenaudio, Head-Fi, the MusicBee, JRiver
and Poweramp forums, Plex pages.

## 1. Defects found in code

### Fix first

| # | Problem | Where |
|---|---|---|
| 1 | Unmute jumps to 100 % volume (full DAC level in Direct output) | `PlayerBar.svelte:65,318` |
| 2 | Android deletes duplicates in one tap, no confirm, no recycle bin | `NeedsAttention.svelte:448`, `changes.rs:307` |
| 3 | Playlist remove/reorder edits the wrong entry when a track id has gone | `PlaylistView.svelte:11-44`; playlists never remapped (`jobs.rs:165` remaps the queue only) |
| 4 | Undo of duplicate removal loses rating/favorite; undo entry dropped before restore | `marks.rs:85`, `changes.rs:221` |
| 5 | Multi-file tag save stops at first failure, earlier files not rescanned | `write_tags` in `api.rs` |
| 6 | History capped at 20,000 events, older ones dropped | `history.rs:17,143` |
| 7 | Scrobble queue kept forever on a permanent 400/401 | `scrobble.rs:140-151` |
| 8 | Desktop update failure shows nothing | `updates.svelte.ts:~125` |
| 9 | JSON saves: fixed `.tmp` name, no `sync_all`, errors swallowed | `state.rs:292-298` |

### Audio engine

| Problem | Where | Fix |
|---|---|---|
| CUE tracks start up to one FLAC frame early: Symphonia's accurate seek lands before the target and the code never discards `required_ts − actual_ts` | `source.rs:150-169` | discard the difference; for consecutive tracks of one file keep the same decoder |
| Resampler (rubato `Fft::new`, chunk 1024) likely rolls off the top treble in shared mode. One external measurement of the same setup: −1.9 dB at 0.81·Nyquist. **Verify with a sweep first.** | `resample.rs:8,25` | `Fft::new_custom`, 1 sub-chunk, chunk 4096–8192 |
| Exclusive thread has no MMCSS "Pro Audio" priority | `exclusive.rs` | `AvSetMmThreadCharacteristicsW` |
| Leveling uses sample peak, not true peak | `loudness.rs:111` | ebur128 `Mode::TRUE_PEAK`, −1 dBTP |
| No dither at 24-bit when DSP or volume is active; none on Linux/macOS direct | `pcm.rs:43-66`, `output.rs:470` | TPDF at device bit depth when gain ≠ 1 |
| Volume not ramped; crossfeed level change clears filter memory | `output.rs:156`, `crossfeed.rs:99` | per-sample smoothing |
| Leveling/tone change restarts the song; sleep fade moves the user volume; filter-count change clicks | `mod.rs:918`, `sleep.rs:59-82`, `eq.rs:295` | move gain/EQ/crossfeed/volume after the ring buffer with a segment timeline (CamillaDSP model) |
| Crossfade sums with no limiter | `mod.rs` | look-ahead true-peak limiter, active only when needed |
| Sleep "end of song/album" ignored across a sample-rate switch | `mod.rs:1502` | check `stop_here` before switching |
| Exclusive mode does not follow a default-device change; no device picker | `output.rs`, `exclusive.rs` | wasapi default-device callback; cpal `Device::id()` |
| Opus files cannot play (no decoder in Symphonia 0.6.1) | `Cargo.toml` | `symphonia-adapter-libopus` |

### Controls and navigation

- Album page shows "no longer in your library" after editing its title (album id is a hash of the tags).
- Page error boundary sticks on desktop until "Try again" (`+page.svelte:248`).
- Escape inside a dropdown closes the whole dialog (`Modal.svelte:15`).
- Now Playing seek slider freezes after a click without drag (`NowPlaying.svelte:190`).
- Lyric line click and the mini player ignore the timing offset; auto-scroll fights touch/scrollbar.
- Back: no scroll restore, same-page pushes, no forward, unbounded (`store.svelte.ts:888-915`).
- Phone: Artists, playlists, Needs Attention, Health unreachable; landscape/tablet hides search.
- Shuffle order lost after restart; skipped unplayable files never reported; queue label stuck at 200.
- Stats artist names open "No music by…" for featured artists.
- Hotkey toggle shows on when nothing registered; platform wording says "Windows" everywhere.
- Dead: `quit`, `debug_tap` commands; `eq.preamp`, `exclusiveRate` settings.
- Suspected: duplicate AI pick keys crash Home/Discover; a panic in a rayon job aborts the app.

### CPU and memory

- Infinite drift animations under `blur(36–110px)` run while paused or hidden (`mini/+page.svelte:110`,
  `NowPlaying.svelte:328`, `+page.svelte:375`, `PlayerBar.svelte:511`).
- Tauri events with changing payloads grow the webview heap (tauri #12724, open): likely the cause of
  the earlier 1.3 GB. Send position only; push the queue on a revision counter.
- Sync commands run on the main thread (Tauri docs): volume drag writes `settings.json` per input event.
- Mini window boots the whole store; search re-normalizes every track per keystroke.

## 2. Improvements, ranked

### Tier 1: durability (small, do before anything else)

1. Atomic writes with `tempfile` + `sync_all` + `.bak` rotation.
2. History archive (append-only per-year file) before the 20k drain.
3. One-file backup and restore of user data, with rolling snapshots.
4. Remap playlists when files are replaced (reuse the `marks.rs` follow logic).
5. Stable `track_uid` that survives file replacement: match by MusicBrainz recording id, then
   fingerprint, then title+duration. Design this before SQLite so the schema keys on it.

### Tier 2: fits this library specifically

6. **One artist across scripts.** MusicBrainz aliases (verified live: 米津玄師 ↔ Kenshi Yonezu ↔
   よねづけんし; BTS ↔ 방탄소년단) as a canonical artist id, used by search, Stats, mix caps and radio.
   1 request/s, about 30 minutes once. Search keys from the romanizers already in `romanize.rs`.
7. **Grounded "Ask the DJ".** Model writes a structured query, Rust retrieves, model orders. Deezer
   measured 45 % of generated playlists replayed vs 27 % manual; Plexamp removed its LLM DJ in 2026 as
   unreliable. Needs a language tag per track (`lingua`).
8. **Shuffle that feels random**: balanced by artist, recently played pushed later (Roon 2.73 and
   Spotify, Nov 2025). **Shuffle by album**: Roon's 160-vote request, open since 2017.
9. **Skip-aware radio** with "less like this", hide, snooze 30 days.
10. **Word-level and translated lyrics**: AMLL TTML DB (community word-by-word CJK, CC0) before
    alignment; LLM line-by-line translation; furigana from existing lindera readings; search the
    library by a remembered lyric line.

### Tier 3: usability

11. Multi-select in track lists (WAI-ARIA listbox pattern); menu actions already take several tracks.
12. Phone "Library" tab; size-class layout (bar → rail → sidebar) instead of width media queries.
13. Keyboard: command palette, type-to-jump, forward/back with scroll restore, settings search.
14. Waveform seekbar (known): the most-shipped feature of 2026 (Tauon, Dopamine, Feishin, Roon 2.74).
    Precompute ~1–2k buckets, draw once, no per-frame work.
15. Queue: autoplay when it ends (known), "stop after this track" on any item, jump-to-current.
16. Artist page sort order; duplicate warning when adding to a playlist; playlist folders; notes on
    albums (Roon, 143 votes); UI zoom.
17. Floating lyrics window, reusing the mini-player window.

### Tier 4: audio features

18. Output device picker and per-device profiles (known).
19. Hardware DAC volume on the Android USB driver, so the path stays bit-perfect.
20. AutoEq browser with target choice; FIR convolution later (`fft-convolver`).
21. Analysis: fake-lossless spectral cutoff, DR meter, true bit depth. Useful for a downloader-fed library.

### Tier 5: larger pieces

22. SQLite (`rusqlite` + `rusqlite_migration`, WAL), history first. FTS5 trigram fails under 3
    characters, so keep the in-memory index for 2-character CJK names.
23. ListenBrainz two-way (import listens, love sync, fresh releases by owned artists); Last.fm (known).
24. Picard-compatible tags (`ARTISTS`, `RELEASETYPE`, `ORIGINALDATE`, MBIDs); opt-in rating export to
    Vorbis `RATING`.
25. M3U8/XSPF import and export with a not-in-library list (known).
26. Phone-as-remote over LAN; state sync by change log (files via Syncthing, not built in).
27. Android Auto browse tree: Kotlin `MediaLibraryService`, must answer without the webview.

### Project hygiene

- **Tests**: none exist. `vitest` + `@testing-library/svelte` + Tauri `mockIPC`, and one CI job running
  `pnpm check`, `vitest run`, `cargo test --lib`. Would have caught the unmute, Escape and playlist bugs.
- **Code signing**: Azure Artifact Signing is open to individuals in the US and Canada only. Otherwise
  SignPath Foundation (free for OSI-licensed projects), then Certum open-source (€69). `package.json`
  says MIT, `tauri.conf.json` says GPL-3.0: reconcile first.
- **Security**: `csp: null`; mini window holds the main window's capabilities; AI base URL accepts
  `http://`; diagnostics record clicked-button text (folder paths, song titles).
- **Android 16 target**: orientation locks ignored on tablets, edge-to-edge and predictive back
  mandatory, local-network permission, 16 KB page alignment for the Rust libraries.

## 3. Rejected

ASIO (legal since the 2025 GPLv3 dual licence, but WASAPI exclusive already covers it), DSD, MQA,
beat-matched AutoMix, LLM-as-recommender, per-track bandits, cr-sqlite/automerge, a Subsonic server,
Chromecast for FLAC (gapless only by transcoding), Tauri SQL plugin, `tauri-driver` e2e, EV certificate,
noise shaping at 24-bit, CLAP alternatives with non-commercial weights (MuQ, MERT, Essentia).

## 4. Key sources

- Roon feature suggestions by votes: https://community.roonlabs.com/c/feedback/feature-suggestions/12/l/votes
- Symfonium feature requests: https://support.symfonium.app
- Deezer Text2Playlist: https://arxiv.org/html/2501.05894
- Spotify agentic playlists: https://research.atspotify.com/2025/9/personalizing-agentic-ai-to-users-musical-tastes-with-scalable-preference-optimization
- Plexamp removes Sonic Sage: https://forums.plex.tv/t/plexamp-v4-50-3-ready-or-not/942338
- Spotify shuffle 2025: https://engineering.atspotify.com/2025/11/shuffle-making-random-feel-more-human
- Balanced shuffle: https://keyj.emphy.de/balanced-shuffle/
- MusicBrainz API and rate limits: https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting
- Picard tag mapping: https://picard-docs.musicbrainz.org/en/v3.0/appendices/tag_mapping.html
- AMLL TTML DB: https://github.com/amll-dev/amll-ttml-db
- LRCLIB: https://lrclib.net/docs
- CamillaDSP: https://github.com/HEnquist/camilladsp
- rubato cutoff measurement: https://github.com/gufo-org/gufo/pull/1118
- Tauri event memory growth: https://github.com/tauri-apps/tauri/issues/12724
- Tauri commands and threads: https://v2.tauri.app/develop/calling-rust/
- Android 16 behaviour changes: https://developer.android.com/about/versions/16/behavior-changes-16
- Android bit-perfect mixer: https://developer.android.com/reference/android/media/AudioMixerAttributes
- SignPath Foundation: https://signpath.org
- SQLite WAL and FTS5: https://sqlite.org/wal.html , https://sqlite.org/fts5.html
