# Needs Attention: research for a rework

2026-11-09. Based on the user's real library (7,033 songs, 1,047 albums) and a survey of how other
players and taggers handle library problems.

## Status (2026-11-09): built

Cards worst first, one category open at a time; Sure/Check on copies, Fix all and covers (Sure
ticked, Check not); Ignore on every row (`attention.json`), optional kinds can be hidden; copies go to
the Recycle Bin with Undo, favourite/rating passed to the kept copy; untagged downloads matched to
tagged songs across scripts (りりあ。 = Riria. via romaji); batch cover search with a review grid and
Undo; artist-name check runs after each scan (when online lookups are on); `Compilations - X` = no
artist; missing year/genre and track-number gaps listed (not counted); keyboard ↑↓ Enter Space I;
Recent changes in a dialog. Not done: AcoustID, comparing damaged files' length with the catalogue,
"re-download" for damaged files.

Correction to §2: of the 14 title matches, only 5 are the same recording (artist and length within
15 s); 3 more are probably the same song (shown as Check); the rest are clips (34 s, 65 s) or other
artists.

## 1. What the page was

`src/lib/views/NeedsAttention.svelte`, lists from `app.attention` (store.svelte.ts) plus `libfix.rs`:

| Section | Today in this library | Counted in the badge |
|---|---|---|
| Albums split in two | 0 | yes |
| Same artist under two names (needs a manual "Check artist names" run) | ? | yes |
| Found info to check (after "Find info for all") | 0 | no |
| Songs without info | **32** | yes (the whole badge) |
| Albums without a cover | **~155** | no |
| Recent changes (undo) | 38 | no |

Library Health (a separate page, `health.rs` / `Health.svelte`) has more problems that the
badge doesn't count: **131 files fail the integrity check (truncated)**, 39 upsampled "hi-res",
23 lossy files saved as FLAC, 20 suspect, plus duplicates.

## 2. What the 32 "songs without info" are

All 32 are YouTube rips in `D:\Cadence Music\YouTube\`, `.m4a`, named `Artist - Title`:

- **14 of the 32 are already in the library as a proper copy** (mostly FLAC): Fake Your Death,
  My Heart, 優しさの理由, 君が笑む夕暮れ, ベテルギウス, four りりあ。 songs (tagged "Riria." in the
  FLACs), The Kill, … For these, "Find info" is the wrong fix. The right one is **"You have this
  in FLAC: remove the YouTube copy"** (one click, moved to the Recycle Bin, can be undone).
  (Matching must be careful: by title alone, 王儷婷 "Fly Me to the Moon" matched Luna Haruna's,
  a different recording. Use the artist from the file name plus length.)
- 16 start with `Compilations - `: that's the YouTube playlist name, not the artist. The file
  name parser should treat it as "no artist" and search by title only.
- The rest have a real artist in the file name (`T.M.Revolution & Nana Mizuki - Preserved Roses`),
  so they could get at least artist + title right away, without the catalogue.

So the page shows 32 problems that are really three different situations, each with its own
fix, and all 32 get the same generic button.

## 3. Problems with the current page

1. **One long scroll of equal-weight sections.** No summary at the top, no sense of what matters
   most. 155 coverless albums show 20 at a time with one button each.
2. **The fix doesn't fit the problem** (see §2): a duplicate is handled like a song that needs
   lookup.
3. **Nothing can be dismissed** except artist pairs ("Not the same"). A YouTube rip the user is
   happy with stays in the badge forever, so the badge stops meaning anything.
4. **Two pages for one job.** Damaged files and fake hi-res (Library Health) are worse problems
   than a missing cover, but they're not on Needs Attention and not in its badge.
5. **Batch work is uneven.** "Find info for all" has the right shape (runs in the background, then
   a review list with ticks, nothing written until Save), but covers, artist renames and splits are
   one at a time.
6. **No confidence shown.** "Found info to check" lists matches without how sure the match is.
7. **Undo history at the bottom**, below up to hundreds of rows.
8. **The artist check must be started by hand** and takes ~6 s per name.

## 4. What others do

Short version (details in §6): the good ones **suggest, show before → after with a confidence,
write only on Apply, and can undo** (beets, Picard, MusicBee). Google Photos' **one card per
problem type, pre-ticked, one button** is the layout. Roon and Apple show what goes wrong
otherwise: deletes without listing them, and silent failures.

## 5. Proposal

### 5.1 Layout: a summary, then one category at a time

```
Needs Attention                                              [Recent changes ⟲]
┌───────────────┐ ┌───────────────┐ ┌───────────────┐ ┌───────────────┐
│ 14 copies you │ │ 18 songs      │ │ 131 damaged   │ │ 155 albums    │
│ already have  │ │ without info  │ │ files         │ │ without cover │
│ [Remove all]  │ │ [Find info]   │ │ [Show]        │ │ [Find covers] │
└───────────────┘ └───────────────┘ └───────────────┘ └───────────────┘
 Ignored (3) · Everything else is fine ✓
```

- Cards ordered by **severity** (can't play / wrong file > duplicate > wrong info > cosmetic),
  each with the count and **one primary batch action**. Clicking a card opens its list.
- The sidebar badge counts only the first three kinds (things that are wrong), never the cosmetic
  ones (covers, genre, year).
- **All clear** state when nothing's left, with the date of the last check.

### 5.2 Every fix: suggest → review → write → undo

The "Find info for all" flow becomes the pattern for every category:

1. A background job proposes fixes (low priority, progress in the sidebar, as now).
2. A review list: each row shows **before → after** and **how sure** (High / Check), with the
   High ones ticked and the Check ones unticked. "Accept all High" is one click.
3. Save writes, and the change shows in Recent changes with Undo (as now).

### 5.3 Ignore

Every row gets **Ignore** ("this is how I want it"). Ignored items go to an "Ignored (n)" list at
the bottom of each category, from which they can be brought back. Kept by track/album id in a
small JSON (like `artistPairDismiss` today).

### 5.4 New and better detectors, in order of value for this library

| # | Detector | Fix | Here |
|---|---|---|---|
| 1 | **Lossy copy of a song you have lossless** (untagged rips first, then all lossy) | Remove the lossy copy (Recycle Bin, undo) | 14 of 32 untagged; more in the 1,379 lossy files |
| 2 | **Damaged files** (integrity: truncated / failed), moved from Library Health | Re-download (the downloader queue) / remove | 131 |
| 3 | **Songs without info**, with a better file-name parser (`Compilations - ` = no artist; `A & B - T`) | Instant artist/title from the file name, then the catalogue | 18 after #1 |
| 4 | **Same artist under two names**, run automatically in the background after a scan (only new names) | Pick the name, as now | — |
| 5 | **Albums without a cover**, batch: look up all, review a grid of found covers | Save the ticked ones | 155 |
| 6 | **Fake hi-res / lossy-as-FLAC**, moved from Library Health | Mark, or queue a better copy | 39 + 23 |
| 7 | Albums with missing songs (gaps in track numbers) | Show what's missing; Discover/downloader | 196 album discs (noisy: needs care, many singles off compilations) |
| 8 | "feat." inside the artist (18), missing year (470), missing genre (1,266) | Batch from the catalogue | cosmetic, not in the badge |

Library Health stays the place for running the long file checks (integrity and quality scans,
the spectrum views); its **findings** surface as Needs Attention cards.

### 5.5 Smaller things

- Recent changes moves to a button in the header (a side panel), not the bottom of the page.
- Confidence shown wherever the catalogue is involved.
- Keyboard: ↑/↓ through rows, Enter = accept, I = ignore (triage speed).
- The artist-name check needs no button: it runs on new names after each scan.

## 6. Survey: what other apps do

| App | Detects | Fix interaction | Worth taking |
|---|---|---|---|
| **beets** | weak matches, duplicates, missing tracks (`missing`), untagged (AcoustID `chroma`) | similarity % per candidate; strong matches applied automatically, weaker ones prompt Apply / More candidates / Skip / Use as-is / Enter search / Enter ID. Duplicates: Skip new / Keep all / Remove old / Merge / **Upgrade** (keep the better copy) | confidence tiers; a fixed set of answers per issue type; "Upgrade" for duplicates |
| **MusicBrainz Picard** | unmatched files, match quality | coloured match-quality block (red→green); unmatched files under the release, dragged onto the right track; nothing written until Save. Users want to **sort by match quality** (long-standing request) | visible confidence, sortable |
| **MusicBee** | missing artwork, missing lyrics, tags | Auto-Tag preview: **original in grey, new value in black**, Apply or Start Over; fields can be excluded; "save artwork without prompt" off by default | before → after diff |
| **Roon** | skipped/corrupt files, unidentified albums, missing files | Identify Album with candidate editions; Clean Up Library = checkbox list, one confirm; no confidence, no undo. Users complained it doesn't list what it deletes | counter-example: always list, always undo |
| **Plex** | unmatched | Match / Fix Match: pick from search results | — |
| **Lexicon DJ** | duplicates (audio signature or tags), broken, lost, unused files | "Smart Fixes": pick a fix → Scan → review results page; duplicate list **locked by default**; removed files go to an **archive**, not deleted | named scans, archive instead of delete |
| **Navidrome** | missing files | admin page with path, size, when it went missing; data kept so ratings/play counts survive a move | keep history when files move |
| **Apple Music** | missing artwork | Get Album Artwork fails silently, no list of what wasn't found | counter-example: list the failures |
| **Linear Triage** | — | one key each: Accept, Decline, Duplicate-of, Snooze; snooze ends on new activity | keyboard triage |
| **Google Photos Utilities** | suggestions by type | **one card per type**, items pre-selected to untick, batches of 30, one Apply; ✕ dismisses a card; a setting turns a whole type off | the card layout of §5.1 |

Sources: beets.readthedocs.io (tagger, duplicates, missing, chroma); picard-docs.musicbrainz.org
(status icons, matching options); musicbee.fandom.com/wiki/Tag_Preferences;
help.roonlabs.com (clean-up-library, identification); support.plex.tv (fix match);
navidrome.org/docs/usage/library/missing-files; linear.app/docs/triage; support.google.com/photos/answer/6128811.
MusicBee, Lexicon and Apple details are from wikis, reviews and forums.

### What this changes in the proposal

- §5.2 confidence: use beets' three tiers. **Sure** (pre-ticked, "Accept all"), **Check**
  (unticked), **Not found** (listed with the reason, never silently dropped, unlike Apple).
- §5.4 #1 duplicates get beets' answers in plain words: **Keep the better copy** (default: lossless
  over lossy, then bit depth/rate, then not-damaged), Keep both, Ignore.
- Deletions go to the Recycle Bin and are in Recent changes (Lexicon's archive, Roon's complaint).
  Moving the play count, rating and favourite from the removed copy to the kept one, as Navidrome
  keeps history.
- Settings → "Don't show: missing year / genre / cover" (Google Photos' per-type off switch).
- Damaged files (#2): also compare the length with the catalogue's, to catch cut-off Soulseek
  downloads that decode cleanly.
- Later: AcoustID fingerprints for files whose name says nothing (the `Compilations - ` rips).
