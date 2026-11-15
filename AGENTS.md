# Cadence: rules for coding agents

Cadence is a FLAC music player: Rust + Tauri 2 backend (`src-tauri/`), SvelteKit 3 + Svelte 5 (runes) front end
(`src/`). Windows is the main platform; Android, Linux and macOS are built from the same code. One person owns it
and works almost entirely through AI agents, often several at once in the same working tree. These rules exist
because of that: the owner cannot review every line, so the code has to stay small, checkable and safe by default.
Read this whole file at the start of a session. Each rule comes with its reason; if a rule truly does not fit, say
so in your report instead of working around it quietly.

## 1. Before you say "done"

Run what applies and report the result honestly.

| Where | Command | Must be |
|---|---|---|
| `src-tauri/` | `cargo check`, then `cargo test --lib` | no errors, all pass; no new warnings in code you touched |
| repo root | `pnpm check` | 0 errors and 0 warnings |
| repo root | `pnpm test` | all pass |
| repo root | `pnpm check:size` | exit 0 (limits in section 2) |

You cannot hear or see the app. Say what you could not verify and what the owner should try (for example "play a
gapless album on the DAC and check the first second", "open Settings on the phone layout"). Never claim sound or
looks were checked. Do not launch the owner's running app, and do not run `tauri dev` or `tauri build` unless asked.
To test against a running app, use an isolated test instance (its own identifier and data folder), muted. A non-production identifier also gets its own keyring
namespace (`secrets.rs`), so test copies have no ListenBrainz token or AI key.

## 2. Size limits

Small files and functions keep reviews readable, and they keep parallel agents out of each other's way: two agents
editing one 2,000-line file collide, two agents editing two 300-line files do not. `pnpm check:size` enforces the
caps. The target is where well-factored code here already sits; the cap is a hard stop for new code.

| What | Target | Cap | Why the cap |
|---|---|---|---|
| Rust module, lines of code (`#[cfg(test)]` blocks not counted) | 500 | 800 | past it, one module has several jobs and every Rust agent touches it |
| Svelte component, whole file | 350 | 700 | script, markup and style all load into one review |
| Svelte `<script>` part | 150 | 300 | logic in a component cannot be unit-tested or reused; it belongs in a `.ts` module |
| `$effect` blocks per component | 3 | 6 | an error in any effect becomes a whole-page error screen, and effects trigger each other |
| TS module (`src/lib`, `src/routes`) | 250 | 500 | same as Rust |
| Test file | 300 | 500 | a failing test should point to one feature |
| `app.css` | 600 | 800 | global CSS is shared by everything; scope styles in the component instead |
| Rust function | 50 | 100 | past it, it cannot be read at once; split by step, name the steps |
| TS or Svelte function (handlers, `$effect`, `onMount` bodies included) | 40 | 60 | the same; also where UI bugs hide |

Other shape rules, none of them scripted:

- Clippy's defaults apply (7 parameters at most): more than that means a struct is missing.
- Nest at most about four levels. Return early, or pull the inner block out as a function.
- Do not build one big `match`/`if` ladder for unrelated cases; one small function per case.

Existing code that is already over a cap is grandfathered in `scripts/size-allowlist.json` (28 files, 29
functions). The rules for it:

- A grandfathered file or function must not grow beyond its recorded size plus a small tolerance (5 %, at least
  20 lines). New code for a feature goes into a new module, and the old file only gets the call.
- When you touch a grandfathered file for a feature, extract the part you are working on into its own module if
  that is cheap and safe. Never rewrite around it as a drive-by.
- Do not launch big refactors to satisfy the limits unless the owner asks. Splitting `api.rs`, `audio/mod.rs` or
  `store.svelte.ts` is a planned task with its own plan and review, not a side effect of a feature.
- Do not edit the allowlist to make the check pass. Shrinking an entry after a real split is fine
  (`node scripts/check-size.mjs --update-allowlist`, then read the diff: it must only go down).

Where to split first, when the owner asks (natural seams only):

- `src-tauri/src/api.rs`: the commands table (`cadence_commands!`) stays in one place, but the function bodies move
  next to their domain (a `library`, `playlists`, `lyrics`, `settings`, `online` group each), api.rs keeping thin
  wrappers. Each section of api.rs is already headed by a comment for its domain.
- `src-tauri/src/audio/mod.rs`: the worker thread's command handler (`handle`) and `open_output` are the two long
  parts; the status building (`emit`) and the device switching are separate concerns from the render callback.
  Never move code into or out of the render callback while splitting.
- `src/lib/store.svelte.ts`: the `AppState` class mixes navigation, library data, player state, job progress and
  event subscriptions. Split the job progress state and the event wiring (`init`) into small modules first, the way
  `showing.svelte.ts` and `selection.ts` already were.
- `src/lib/components/LyricsView.svelte`: the script (635 lines, 11 effects) is mostly line timing and scrolling
  logic: move it into `.ts` modules and test them directly. `PlayerBar.svelte` and `Home.svelte` are mostly style.

## 3. Where things go

The layout as it is (top levels only):

```
AGENTS.md  CLAUDE.md  README.md  CHANGELOG.md
package.json  vite.config.js  tsconfig.json        pnpm, SvelteKit (adapter-static), vitest (jsdom)
src/                                               the web UI
  app.css                                          design tokens (:root vars), global rules, html.phone shell
  routes/                                          +page.svelte (the shell, global keys), +layout, +error, mini/ (mini player)
  lib/
    api.ts                                         types + the only caller of `invoke` (one wrapper per command)
    store.svelte.ts                                the `app` singleton: library, player, navigation, job progress
    *.ts, *.svelte.ts                              small logic modules (searchIndex, selection, shortcuts, showing, ...)
    views/                                         one page each (Home, Search, Settings, NeedsAttention, ...)
    components/                                    reusable pieces, dialogs and settings sections (flat for now)
  tests/                                           vitest + testing-library; helpers.ts has the mocked backend
src-tauri/
  src/lib.rs, main.rs                              app start, the Tauri command list
  src/api.rs                                       every command's function, and the `cadence_commands!` table
  src/commands.rs                                  turns that table into Tauri commands (plain/fallible/bytes/blocking)
  src/ctx.rs                                       `Ctx` (state, voice, host) and the `Host` trait (emit events)
  src/state.rs                                     Settings and other saved structs, atomic save, debounced writer
  src/jobs.rs, hub.rs                              background jobs and the shared pool; engine status fan-out
  src/audio/                                       engine: worker thread, real-time render callback, output, DSP
  src/voice/                                       "follow the singer" (vocal separation, alignment)
  src/*.rs                                         ~55 domain modules at one level (see the grouping below)
  examples/                                        small command-line probes for the audio and tag code
  capabilities/, tauri.conf.json, Cargo.toml       Tauri permissions and config
docs/                                              research-<topic>.md notes (design reasoning, not user docs)
site/                                              static website, deployed as is by .github/workflows/pages.yml
scripts/                                           build and release helpers; check-size.mjs and its allowlist
.github/workflows/                                 ci.yml (checks), release.yml (tags), pages.yml (website)
```

Where a new thing goes:

- A Tauri command: a function `pub fn name(ctx: &Ctx, ...)` in the domain module's section of `api.rs` (or, better,
  in the domain module with a one-line wrapper in api.rs), a line in `cadence_commands!` (pick `plain`, `fallible`,
  or `blocking`), its handler in the list in `lib.rs`, and a typed wrapper in `src/lib/api.ts`. Names: `verb_noun`
  in Rust (`get_`, `set_`, `list_`, `start_`), the same name in camelCase in api.ts. Heavy work is `blocking`.
- A backend domain module: its own file with a `//!` header saying what it owns; its logic is plain functions that
  take `&Ctx` or `&AppState`, not Tauri types, so unit tests need no window. Tests go in the same file in
  `#[cfg(test)] mod tests`.
- A background job: a module that starts work on `jobs::background_pool()` (never its own thread pool), emits
  `<job>-progress` events with `done` and `total`, can be paused (`<job>_pause` command), and appears in the
  sidebar job indicator (`Sidebar.svelte`). See `waveform.rs` or `lyricindex.rs` for the shape.
- An online service client: a module of its own, built on `net::builder()` and `net::read_limited`, following
  section 6. Never call a service from the UI.
- A setting: a field in `Settings` in `state.rs` with a doc comment, in `Default` (for anything that talks to the
  network, ask the owner whether it starts on or off), `serde(default)` so old files load, a `set_<name>`
  command that ends with `save_settings_later()`, its field in the object `get_app_settings` returns, its type in
  `api.ts`, a control in `Settings.svelte` (or a component under `components/`) with a plain-words description,
  and, if it enables a network call, a row in the README privacy table. A setting that holds a secret goes in
  `secrets.rs` (the OS keyring), not in `Settings`.
- A page: `src/lib/views/<Name>.svelte`, registered where the others are (store navigation, Sidebar, command
  palette items in `paletteItems.ts`). A page with sub-parts gets a folder `views/<name>/` for them.
- A dialog: `components/`, built on `Modal.svelte` (it handles Escape, focus and stacking). One file per dialog.
- A reusable component: `components/`, no knowledge of the store if it can be avoided: props in, callbacks out.
- State: a new field on `app` only if most of the UI needs it. A feature's own state goes in its own
  `<feature>.svelte.ts` module (see `playlistTransfer.svelte.ts`, `updates.svelte.ts`).
- Shared pure logic (matching, formatting, sorting): a `.ts` module in `src/lib/` with no Svelte imports, so a
  test can call it directly.
- Tests: Rust in the module's own `#[cfg(test)] mod tests`; front end in `src/tests/<feature>.test.ts` using
  `helpers.ts` (`mockBackend`, `makeTrack`, `resetApp`). Fixtures are built in code, not stored as files.
- Research and design notes: `docs/research-<topic>.md`, dated, with the question first and the decision last.
- Scripts: `scripts/`, one purpose each, no new dependencies.
- Website: `site/` only; plain HTML/CSS/JS, no build step. Update it when a release changes what the site says.

New files follow this grouping so that the flat folders stop growing. Moving existing files into it is a separate
planned task: do not move files as part of a feature.

`src-tauri/src/` (new modules go in the matching folder, created with a `mod.rs`):

| Folder | Holds (existing modules that belong there) |
|---|---|
| `library/` | library, cue, health, attention, libfix, changes, tagedit, tagids, covers, autotag, identity, artists, songlang |
| `lyrics/` | lyrics, lyricsfile, lyricindex, romanize, translate |
| `online/` | net, musicbrainz, catalog, lbapi, lbimport, lbloved, lbreleases, scrobble |
| `mixes/` | dj, ai, lanes, sonic, upnext, shuffle, stats, history |
| `data/` | state (persistence parts), backup, profiles, marks, playlists, playlist_io, searchkeys |
| `shell/` | tray, hotkeys, media, winvis, sleep, secrets |
| `audio/`, `voice/` | as now |

`src/lib/components/` (new files go in the matching folder):

| Folder | Holds |
|---|---|
| `ui/` | Toggle, Select, Segmented, Icon, Rating, QualityBadge, Carousel, ContextMenu, Modal |
| `dialogs/` | FixAlbum, FixInfo, CoverPicker, TagEditor, PlaylistPicker, PlaylistExport/ImportDialog, ShortcutHelp, CommandPalette, SmartEditor, Licenses |
| `settings/` | AiSettings, BackupSettings, ListenBrainzTools, AppKeys, SkippedSongs, OutputDevice |
| `player/` | PlayerBar, NowPlaying, QueuePanel, LyricsView, SignalFlow, Waveform, EqGraph, SleepLabel |
| `shell/` | Sidebar, TitleBar, TabBar, UpdateBanner, AccessBanner, Guard |
| `lists/` | TrackList, AlbumCard, SelectionBar |

In `src/lib/`, three or more related logic modules get a folder (`lib/lyrics/`, `lib/search/`); a planned split
of `api.ts` into `lib/api/<domain>.ts` re-exporting from `api.ts` keeps imports working.

## 4. Conventions in use

- Names. Files: Rust `snake_case.rs`; components `PascalCase.svelte`; TS modules `camelCase.ts`, with
  `.svelte.ts` when they use runes. Events: `kebab-case` (`library-changed`, `analysis-progress`). Settings: Rust
  `snake_case`, JSON `camelCase` (`serde(rename_all = "camelCase")`). Import front-end modules with the alias
  `#lib/...js`.
- Tests are named as plain sentences saying what is true: Rust `a_replaced_file_is_found_again`, vitest
  `test("finds a song and its artist by an alias", ...)`. One behaviour per test.
- Comments explain why, in plain words, not what the next line does. Doc comments (`///`, `//!`, `/** */`) say
  what a thing is for and what its edge cases are. A workaround or a limit gets one line with its reason.
- User-facing text is short, plain sentences in sentence case, with curly quotes for names: `Couldn't merge “X”:
  …`. The UI says "song", never "track". No jargon, no internal names, no blame. Say what happened and what the
  person can do. Follow the strings already in the file you edit.
- CSS: colours, radii and fonts are tokens in `src/app.css` (`--accent`, `--surface`, `--r-md`); use them, do not
  write hex colours in components. Component styles are scoped; global rules go in `app.css`. The phone shell
  (Android) is `html.phone`, set by script, not only a width media query; write the phone rule next to the desktop
  one (`:global(html.phone) .thing`). `--gold` means lossless or bit-perfect and nothing else.
- Rust: errors from commands are `Result<_, String>` with a sentence a person can read; log details with `log::`.
- CHANGELOG: `## 0.2.x (YYYY-MM-DD)`, bullets with a bold lead phrase, written for listeners, in plain words: what
  changed for them, not which file. Each release's section becomes its GitHub release notes.

## 5. Efficiency is a release gate

The owner measures RAM and CPU, and found that a few always-running animations were the app's largest cost. A
player runs all day; idle must be nearly free.

- No per-frame work. No `requestAnimationFrame` loop that runs without a user gesture (the one allowed use is the
  short drag-to-scroll tick in `TrackList.svelte`, which stops when the drag ends).
- No CSS animation that is `infinite` or has no end. The display may run at 160 Hz and an animation costs CPU at
  display rate. Animate only `transform` and `opacity`; never an animated `filter`, `backdrop-filter`, or any
  property that changes layout (`width`, `height`, `top`, `margin`). A short `transition` that ends is fine.
- No new timer (`setInterval`, repeating `setTimeout`) unless it stops when the page is not showing
  (`src/lib/showing.svelte.ts`: `app.showing`, `pageShowing()`, `onShowing()`) and when nothing is playing.
- The player status object arrives about four times a second. Do not derive lists, sort, filter or re-render
  anything from it per tick; only compositor-only updates (a progress `transform`) may follow it.
- Long lists are virtualized, and selection checks are O(1) (a `Set`), never `array.includes` per row.
- Heavy backend work runs off the window's thread: a `blocking` command, or a background job. Never inside a
  `plain` command that can take longer than a few milliseconds.
- At most two heavy background threads in total: use `jobs::background_pool()` (it is sized for that, in background
  priority). A job is visible in the sidebar and can be paused (work on a single song that lasts a few seconds at most, such as the lazy waveform of the song being played, is exempt). Light work that must not wait behind a long job takes a turn on the pool between its songs (`jobs::run_soon`, as the lyrics index does). Memory used by a job is bounded.
- Caches have a size or age bound. Nothing heavy runs at startup: load on first use, on a thread, and keep the
  window opening fast. Do not hold decoded audio or large images longer than the job that needs them.

## 6. Safety rules that protect the listener

Audio
- The render callback (the real-time audio thread in `audio/`) must not allocate, lock, log, or make a system
  call. Prepare data outside it and pass it in with the lock-free structures already used there. A glitch is
  audible and cannot be undone.
- Bit-perfect pass-through must stay bit-exact: no gain, dither, resampling or float round trip in that path.
- Any code path that opens an output device or switches to one goes through the volume-safety rule: a device
  with no remembered volume starts at a safe level (Direct output sends full scale to the DAC, with no Windows
  volume after it). It lives with the device-profile code (`profiles.rs`, `audio/safety.rs`,
  `audio/opened.rs`; search for `NEW_DIRECT_VOLUME` and `start_volume`). Do not add a new way around it: every
  output is set and let go only through `opened` and `close_output`.
- Tests never open an audio device. Test the logic around it with plain data.

Robustness
- The release profile is `panic = "abort"`: a panic ends the app and the music with it. No `unwrap`, `expect`,
  indexing (`v[i]`) or byte slicing (`&s[a..b]`) on data that comes from files, tags, the network or the user. Use
  `get`, `?`, `let Some(..) else`. Slicing a string by byte offsets can cut a CJK character in half: use
  `char_indices` or `chars`. Use `saturating_*` or `checked_*` arithmetic on numbers from outside (durations,
  sizes, counts in tags). A `.expect` is acceptable only for an invariant the code just established, with the
  reason in the message.
- Treat every file as possibly damaged and every network answer as possibly hostile: bound the size you read
  (`net::read_limited`), and never trust a length field.

Data
- User data (settings, playlists, marks, history, lyrics the user picked) is written only through the atomic save
  routine in `state.rs` (`write_json`, `write_file`, or the `save_*` methods of `AppState`; settings use the
  debounced `save_settings_later`). Do not write these files with `std::fs::write`. The routine writes a temporary
  file, syncs, replaces, and keeps a `.bak`; a file that could not be read is not overwritten with defaults.
- Never write to, move or delete the user's music files, except in the features whose purpose is that (tag editor,
  cover writer, duplicate removal, the Fix tools), and then with an undo (`changes.rs`).
- Song identity is strict: same title, same artist, close duration. If it is ambiguous, there is no match. Never
  guess, never match on title alone; a wrong match merges the wrong history or deletes the wrong file.
- A new file that holds the user's decisions must be added to the backup include list (`WHOLE` or the kinds in
  `backup.rs`) and restored by it. Rebuildable caches stay out of backups.
- Old files must keep loading: new fields in saved structs are `#[serde(default)]`; never rename or remove a
  saved field without a reader for the old name.

Privacy (the README promises "no account, no analytics, no tracking")
- Every network call sits behind an online toggle in `Settings`, checked at send time (not once at startup), and
  has a row in the README Privacy table that says exactly what is sent. If you add or change a call, change the
  table in the same change.
- Requests use `net::builder()`, the app's User-Agent (`Cadence/<version>`, plus the project page for services that
  ask, as MusicBrainz does), and send no user data beyond what the table lists. Set a timeout, retry a bounded
  number of times, and honour rate limits and `Retry-After`.
- Secrets (API keys, tokens) live in the OS keyring through `secrets.rs`. They never appear in logs, error text,
  events, backups, or the diagnostics report (`diag.ts`, `backup.rs` strips them).

## 7. Contracts between the two halves

- Never change the name, argument names or return shape of an existing Tauri command or event: an old window, a
  test, or the Android build may still use it. Add a new one and move callers over.
- `src/lib/api.ts` is the only place that calls `invoke`. (One known exception, the Android update download in
  `updates.svelte.ts`; do not add more.) Events are subscribed in `store.svelte.ts` or the feature's own module,
  never from a random component.
- Command arguments are camelCase in JS and snake_case in Rust; Tauri converts. Return types derive `Serialize`
  with `rename_all = "camelCase"` and are mirrored as an `interface` in `api.ts`, with a comment on the non-obvious
  fields.

## 8. Front-end rules

- In SvelteKit 3 an uncaught error in render or in an effect becomes a whole-page error screen. Be defensive: guard
  `null` from the store, catch rejections of promises started inside an effect or handler (`.catch(...)` or
  `try/await`), and show a toast for failures (`app.showToast`). Do not rely on `Guard.svelte` to save you.
- Escape and Back are layered: menu, then dialog (`Modal.svelte` keeps a stack and closes only the top one), then
  Now Playing, then the page. A component that needs Escape checks `e.defaultPrevented` and that it is on top,
  then calls `preventDefault()`. Do not add a competing window `keydown` listener that ignores the layers.
- Keyboard shortcuts are declared in `src/lib/shortcuts.ts` (the help dialog, Settings and the palette read it).
  Add the key there and handle it where the others are handled (`routes/+page.svelte`, `AppKeys.svelte`).
- Everything usable by mouse is usable by keyboard and has an accessible name (`aria-label`, roles); long names
  wrap or truncate with a tooltip; layouts work in the narrow phone shell, in Japanese, Chinese and Korean text,
  and with an empty library.
- Use `$state`, `$derived`, `$effect` (runes). Prefer `$derived` over an effect that writes state; an effect
  that writes state it reads causes loops. Clean up listeners and timers in the effect's return function.

## 9. Tests

- Every behavioural change comes with a test: Rust unit tests, or vitest with the mocked backend in `helpers.ts`.
  A bug fix gets a test that fails without the fix.
- Tests must not touch the network, real audio devices, the owner's data folder or music library. Rust tests use
  `tempfile` directories and plain data; front-end tests use `mockBackend`.
- Fixtures use invented titles, artists and lines. Never put real song lyrics in the repository.
- Do not weaken, skip or delete a test to make it pass. If a test is wrong, say why in your report and fix its
  expectation deliberately.

## 10. Working in a shared tree

Several agents may be editing at once, and the owner is often listening to the app.

- Stay inside the files you were given. In files everyone touches (`api.rs`, `api.ts`, `state.rs`,
  `store.svelte.ts`, `Settings.svelte`, `routes/+page.svelte`) make small, additive edits, and re-read the file
  right before editing it, since it may have changed. Never reformat a file or reorder what you did not change.
- No git command that changes state (`add`, `commit`, `checkout`, `reset`, `stash`, `push`, `rebase`) unless the
  owner asks. Do not "clean up" files you did not create. `git status`, `diff` and `log` are fine.
- Do not leave scratch files in the repo; use a temporary folder outside it.
- Add dependencies only when the owner agrees: the app ships to four platforms, and each crate or package
  adds download size, licences (`static/third-party-notices.txt`) and Android and Linux build risk.

## 11. Git and releases

- Commit only when asked. Subjects are short, plain and about what changed for the user, as in the history:
  `Needs Attention rework: cards, Sure/Check, Ignore, Recycle Bin with Undo`, `Fix song info: names in Latin
  letters`, `Speed: no rescan loop during downloads`. Internal-only changes say so plainly (`CI: ...`, `Clippy
  cleanup: ...`). Explain the why in the body when it is not obvious.
- A release is a small sequence, done only when the owner asks:
  1. Add a `## 0.2.x (YYYY-MM-DD)` section to `CHANGELOG.md` (commit `What's new in 0.2.x`).
  2. Bump the version in `package.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (the `cadence` package)
     and `src-tauri/tauri.conf.json` (commit `Cadence 0.2.x`).
  3. Tag `v0.2.x` and push the tag. `.github/workflows/release.yml` then builds Windows (scanned with Defender),
     macOS and Linux, attaches them to a draft release with the CHANGELOG section as notes, publishes it once all
     three desktop builds are there (which also publishes `latest.json` that every installed app's updater reads),
     and then builds and attaches the signed Android APK. A failed desktop build leaves a draft.
  4. Pushes and tags use the owner's own GitHub account; if a push is refused, report it rather than trying other
     credentials.
- `ci.yml` runs `pnpm check`, `pnpm test`, `pnpm check:size` and `cargo test --lib` on every push to `main` and every pull request
  (docs and site edits excluded). Keep it green; do not edit workflows unless asked.
