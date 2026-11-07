# Features to add, from the music players people love (2026-11-07)

Method: three research passes over official pages and GitHub issue trackers (foobar2000, MusicBee, Strawberry, Plexamp,
Roon, AIMP, Audirvana, Harmonoid, Tauon, Quod Libet, Navidrome/Feishin, Poweramp, Symfonium, Namida, Musicolet, Auxio,
Neutron and others), checked against what Cadence has today (README, CHANGELOG, code).
Evidence caveat: Reddit, Hydrogenaudio and the Plex forums could not be read. "Demand" below means issue-tracker
reactions and vendor feature lists, not a survey. Streaming-only features (Spotify, Apple) are from memory.

## What Cadence already has (don't re-research)

Gapless, crossfade, ReplayGain/R128, WASAPI exclusive, parametric EQ + AutoEq, crossfeed, signal path, synced/karaoke
lyrics and Follow singer, bliss "sounds like", AI DJ + Discover, Stats, playlists + smart playlists, tag editor, Fix
song info, cover/artist pictures, ListenBrainz, SMTC/media keys, tray + mini player, themes, shuffle/repeat + shortcuts,
Android app with widget, self-updater.

## Gaps confirmed in the code

No favorites/hearts, no star ratings (the `Track` type has none and smart-playlist fields are title, artist, album,
genre, year, plays, skips, duration, last played, added, quality), no sleep timer, no global hotkeys, no Discord
presence, no Last.fm (ListenBrainz only), no M3U/CSV playlist import or export, no folder view, no CUE sheets, no
visualizer or waveform seekbar, no playback speed or A–B loop, no MusicBrainz/AcoustID lookup, no artist bios/credits,
no casting (UPnP/Chromecast/AirPlay), no Subsonic/Jellyfin, no backup/restore of play data.

## Ranked recommendations

Score = how many loved players have it, how loudly users ask for it, and fit with a local-lossless player.

### Tier 1: do next (high demand, small to medium work)

| # | Feature | Why / who has it |
|---|---|---|
| 1 | **Favorites (♥) and 1–5 star ratings**, shown in lists, Now Playing and the ⋯ menu, sortable, bulk-editable | Most-requested item after smart playlists (Strawberry #259, Navidrome #1473/#1479, Feishin #2037). Every serious player has it. Unlocks #2 and #4. |
| 2 | **Smart playlist fields: rating, favorite, BPM, bitrate, sample rate, codec, album artist, composer**, plus "limit by time", sort by rating then plays | Smart playlists are demand #1 (Navidrome #1417, 232 comments). Symfonium's pitch is literally "more than 4 stars, not played in a month". Cadence's engine already exists. |
| 3 | **Keep ratings, plays and favorites when files move** (key by content hash / tag, not path) | Silent data loss is the top complaint (Navidrome #2068, Strawberry #1819). Cadence already relinks the queue; extend to history, ratings, skips. |
| 4 | **Sleep timer** (minutes, end of song, end of album, N songs) with a fade-out | Common on mobile (Musicolet, Pulsar, Namida, Samsung, AIMP, Spotify). Tiny to build; evidence on desktop is thin but the Android app needs it. |
| 5 | **Queue upgrades**: Play next vs Add to queue, save queue as playlist, undo, clearing keeps the playing song, queue survives restart | Feishin #628/#1643/#640/#809, Navidrome #627. Verify what survives a restart today. |
| 6 | **Playlist import and export (M3U8, CSV)** with a "not in your library" list | Navidrome #1800, Harmonoid #383/#419, Strawberry #327. For you it also closes the loop with the downloader: import `apple_music_downloads.csv` or an Exportify/TuneMyMusic export, match to the library, show what's missing, send missing ones to Discover or the downloader. |
| 7 | **Global hotkeys and Discord Rich Presence** (desktop) | A quiet staple (Harmonoid, foobar2000, Dopamine; Feishin #72). Both are small plugins. |
| 8 | **Last.fm scrobbling and love sync**, plus importing history from Last.fm/ListenBrainz | Strawberry #13/#247, Navidrome #1547, AIMP, Pulsar, Harmonoid. Cadence has only ListenBrainz. |

### Tier 2: differentiators that suit Cadence

| # | Feature | Why |
|---|---|---|
| 9 | **Library health tools: duplicate finder, "fake lossless" detector (spectrogram cutoff), FLAC integrity check, true-peak and DR meter** | You already did this by script for 7000 files (bit-identical MD5, same album other release, lossy with FLAC twin). foobar2000 is the only player with DR meter/integrity; nobody has a friendly UI. Reuses the downloader's logic. |
| 10 | **Radio / Autoplay when the queue ends**, **Song Path** (Plexamp "Sonic Adventure": a route between two songs), **On this day** and **Forgotten favorites** | Plexamp's most-loved features; bliss analysis already gives similarity. Feishin #7/#808/#1996, MusicBee Auto-DJ is "hard to replace". |
| 11 | **Year in review** (Wrapped-style, shareable card) on top of Stats | Spotify Wrapped, Apple Replay, Namida "Your Year". Stats data already exists. |
| 12 | **Folder view** and per-folder excludes | Poweramp's biggest complaint is missing it; Musicolet, Auxio, Symfonium "file mode", Navidrome #2031. |
| 13 | **Richer tags**: multi-value artists, composer, release type, original year, album versions (remaster vs original), artist page grouped by Album/EP/Live/Compilation | Navidrome #238/#489, Strawberry #2361, Plexamp album types. |
| 14 | **MusicBrainz/AcoustID auto-tag and cover** (opt-in, 1 req/s) | MusicBee, Strawberry, Picard-style. Complements Fix song info (iTunes only). Check licence terms (AcoustID non-commercial). |
| 15 | **Playback speed/pitch, A–B loop, bookmarks, skip silence** | AIMP, Neutron, Namida, Harmonoid, Musicolet. Useful for practice, classical, audiobooks. |
| 16 | **CUE sheets** (single-file albums) | Poweramp, AIMP, Tauon, foobar2000, Navidrome #2136. Common in lossless rips. |
| 17 | **Waveform seekbar and a visualizer** | foobar2000 and Poweramp's most-shown extras, Plexamp visualizers. Pure polish; tap the output with `realfft`, stream over an IPC channel. |
| 18 | **Artist page with bio, credits, discography** | Roon, Plexamp, Navidrome #394 (62 comments). Source: MusicBrainz, Wikipedia, Deezer. |
| 19 | **Translated lyrics** under synced lines (Apple Music) | You already do romanization; translation via the AI DJ fits the Japanese/Korean listening in your library. |
| 20 | **Per-device EQ profile** (speakers, headphones, Bluetooth) | Poweramp, Musicolet, Symfonium. Cadence has AutoEq; make it follow the output device. |

### Tier 3: big bets, decide deliberately

- **Cast and remote**: UPnP/DLNA, Chromecast, AirPlay, phone as remote for the desktop (Navidrome #250/#364, Audirvana
  remote, foobar2000 Beefweb). Large; high value if you play to a hi-fi streamer.
- **Library to phone**: Cadence on Android scans its own folders, so a "sync library and ratings between desktop and
  phone" feature (MusicBee, Strawberry; Feishin #47 has 127 reactions for offline) is the hard part users name.
- **Subsonic/Jellyfin client or server mode**: Symfonium, Feishin, Namida. Only if you want Cadence beyond local files.
- **Listening Party** (synced queue with friends), **Android Auto / Wear OS**.
- **Defer**: ASIO (GPL SDK), DSD/DoP, MQA, upsampling for its own sake. Evidence says placebo or licensing pain.

## Anti-features to avoid (what users complain about)

- Clearing the library when a drive or network share is missing at startup (Cadence already keeps unreadable folders).
- Ratings and play counts lost on moves or retags.
- Audible glitches (gapless gaps, FLAC skips, wrong ReplayGain): lossless users forgive missing features, not these.
- Slow startup/scroll with huge libraries (test with 100k tracks).
- Space key stolen from a search box; clearing the queue stopping the song; unrated tracks sorting above rated ones.
- Paywalled core features and subscriptions (Plexamp Pass, Roon, Audirvana Studio).

## Suggested order

1. Favorites + ratings, with hash-keyed storage (#1, #3).
2. Smart playlist fields and sort (#2).
3. Sleep timer, queue fixes (#4, #5).
4. Playlist import/export with missing-songs list (#6).
5. Last.fm + Discord + global hotkeys (#7, #8).
6. Library health (#9), then radio/Song Path (#10), then year in review (#11).

Each of 1–5 is roughly days, not weeks, and reuses existing code (history, smart rules, scrobbler, tray).

## Open questions

- Does the queue survive a restart today, and does "Play next" exist apart from "Add to queue"?
- Is Android in scope for each feature (sleep timer and ratings, yes; global hotkeys and Discord, desktop only)?
- Licence: bliss is GPL-3.0, MusicBrainz/AcoustID/Last.fm are non-commercial; fine while Cadence stays GPL.
