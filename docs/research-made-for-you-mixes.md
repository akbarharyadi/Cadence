# "Made for you today": research and recommendations

Written 2026-11-10, after the user reported that the AI mixes all sound alike (two of three mixes
wore Aimer's *daydream* cover, and two were Japanese rock and Aimer-adjacent).

**Status:** recommendations 1–3 and the lane labels from 4 shipped in 0.2.40 (`src-tauri/src/lanes.rs`,
`ai::daily_mixes`). The lanes test on the real library: `CADENCE_DATA=<data dir> cargo test --lib real_pools
-- --ignored --nocapture` (add `CADENCE_LIVE=1` to also ask the AI service). Still open: collage covers (4),
skip feedback / "Less like this" (5), time-of-day refresh (6).

## How Cadence made them before 0.2.40

- `api::ai_mixes` → `ai::daily_mixes`: **one** LLM call per day. The model sees a 150-song
  shortlist (`ai::shortlist`), a short taste summary (top artists by plays) and the local hour.
  It is *asked* to make different mixes, and it returns only song ids.
- Already uncommitted on `main`: the shortlist is round-robin by artist with a new random seed
  each call, 4 mixes, each with a different angle (time of day, favourites, deep cuts, unexplored
  side), max 3 songs per artist, no song in two mixes (enforced in code), "New mixes" tells the
  model what to avoid, and covers avoid albums already shown.
- Material Cadence already has but the mixes don't use: per-play timestamps (`PlayEvent.at`),
  `last_played` per track, skips, sonic k-means clusters (`sonic::mixes`, the "Mixes by sound" row)
  and per-song sound words (`sonic::describe`).

The root problem: variety is only **requested** from the model. LLMs pick the "obvious" centre
of a library (the most played artist, the dominant genre) for every mix, so asking politely is
not enough. Variety has to be built into what each mix is allowed to choose from.

## What the big services do

| Service | How it gets variety | Familiar vs new |
|---|---|---|
| **Spotify Daily Mix** | Clusters the listener's own listening into separate groups (not genre tags); **one mix per cluster**, up to 6. Clusters stay stable for weeks; tracks refresh daily. | Mostly familiar favourites, a small amount of discovery (less adventurous than Discover Weekly). |
| **Spotify daylist** | Uses the microgenres the listener plays **at this time of day / day of week**; refreshes several times a day; title = microgenres + time ("bedroom pop banger early morning"); cover colour follows the time of day. | Mostly familiar, flavoured by the hour. |
| **Spotify artist / genre / decade mixes** | Each mix pinned to one axis (an artist, a genre, a decade). | Familiar. |
| **Plexamp "Mixes for you"** | Sonic analysis, clustering of recently heavy-rotation albums, plus "historical" mixes (what you used to play). Guest DJs: Stretch (adventurous segues), Contempo (same era), Groupie (one artist). | Mixed; weak on small libraries. |

Research side: the standard tools are **MMR re-ranking** (pick each next item for relevance minus
similarity to what's already picked) and **calibration** (Steck 2018: a list's mix of
genres should match the listener's overall mix, not just their top genre), with newer work adding
an **exploration bonus** for under-represented categories. All of them work on a structure
(categories / clusters) outside the ranker, which matches the Spotify approach.

## Recommendations (in priority order)

### 1. Give each mix its own lane before the model sees anything *(biggest win)*

Instead of one 150-song shortlist for all mixes, build one **pool per mix** in Rust, from
different parts of the library, and let the model curate inside each pool:

| Lane | Pool (built in code) |
|---|---|
| **Right now** (daylist-style) | Songs whose plays cluster around this hour ± 2 h and this weekday, from `PlayEvent.at`. |
| **Your heavy rotation** | Most played in the last 30 days, max 3 per artist. |
| **Rediscover** | Played ≥ 3 times ever but not in the last 60 days (`last_played`). Plexamp's "historical mix". |
| **Deep cuts** | Never or rarely played, from artists the listener *does* play. |
| **A side you don't visit** | The sonic cluster (or genre / language) with the fewest recent plays: an exploration bonus. |

Pick 4 lanes a day (rotate which one sits out), and **exclude each lane's lead artists from the
next lane's pool**. The prompt then becomes "title, order and trim each pool" which models do well,
instead of "invent 4 different ideas from one list", which they do badly. Still one LLM call (pools
sent as separate lists), so no extra cost.

### 2. Remember the last week, not just the last click

Store the track ids (and lead artists) of the last 7 days of mixes in `ai-mixes.json` and drop or
down-weight them in tomorrow's pools. Today only "New mixes" avoids the previous set; the daily
run can repeat yesterday.

### 3. Check variety in code after the model answers

A cheap MMR-style pass over each returned mix: if a mix has > 3 songs from one artist, or shares
its lead artist / main sonic cluster with an earlier mix, swap songs from its own pool. Log a
simple score (distinct artists, distinct albums, mean sonic distance between mixes) so changes can
be compared instead of judged by eye.

### 4. Make the tiles look as different as they are

- Cover: a 2×2 collage of four *different artists* in the mix, or the lane's colour (daylist tints
  covers by time of day).
- A small lane label above the title ("Right now", "Rediscover", "Deep cuts"), so the reason for
  each mix is visible at a glance and the user learns what each tile means.

### 5. Let skips steer the next day

Songs skipped inside an AI mix count against that lane tomorrow; a right-click
"Less like this" on a tile drops its lead artist / cluster from tomorrow's pools. Spotify's mixes
improve "the more you listen"; Cadence already records skips, it just doesn't feed them back.

### 6. Refresh with the day, not once a day *(optional, later)*

The "Right now" lane could refresh at morning / afternoon / evening / night boundaries (daylist
updates several times a day). Only that lane needs a new call; the others stay daily.

## Not recommended

- **More prompt rules alone.** Already tried; the model still drifts to the centre of the library.
- **Several LLM calls per day** (one per mix). Higher cost and latency for little gain over
  lane-specific pools in one call.
- **Discovery of songs not in the library inside these mixes.** "New music for you" already does
  that; mixing it in here would make the tiles unplayable in part.

## Suggested order of work

1. Lanes + per-lane pools + 7-day memory (1–3): about a day; most of the visible variety.
2. Lane labels and collage covers (4): small UI change.
3. Skip feedback and "Less like this" (5).
4. Time-of-day refresh (6), if the "Right now" lane proves popular.

## Sources

- [Spotify Daily Mix (VentureBeat)](https://venturebeat.com/mobile/spotify-daily-mix/), [How-To Geek](https://www.howtogeek.com/275575/press-play-and-go-spotifys-daily-mixes-are-the-best-auto-playlists-yet/), [TechRadar](https://www.techradar.com/news/audio/spotify-daily-mix-is-the-perfect-playlist-you-don-t-have-to-make-1329461), [Spotify newsroom](https://newsroom.spotify.com/tag/understanding-the-app), [artist/genre/decade mixes](https://techzim.co.zw/spotify-adds-decade-genre-artist-personalisation-to-mixes)
- [Spotify daylist (TechCrunch)](https://techcrunch.com/2024/09/04/spotify-launches-its-evolving-playlist-daylist-globally), [Engadget](https://www.engadget.com/spotifys-latest-playlist-auto-adapts-to-your-day-122541712.html), [Music Business Worldwide](https://musicbusinessworldwide.com/spotify-launches-ever-changing-daylist-playlist-that-adapts-to-users-moods)
- [Plexamp](https://plex.tv/plexamp/), [Guest DJ (lon.tv)](https://blog.lon.tv/2023/02/02/new-plexamp-features-guest-dj-and-nfc-tags/), [Plex forum: sonic features](https://forums.plex.tv/t/music-sonic-feature/822588)
- [Exploration-Aware Calibration](https://sol.sbc.org.br/index.php/webmedia/article/view/46084), [Exploration on Demand (MMR comparison)](https://arxiv.org/pdf/2507.21884), [Exploration in music recommendation (NUS bandit)](https://smcnus.comp.nus.edu.sg/archive/pdf/2014-2016/2014_bandit_mr.pdf)
