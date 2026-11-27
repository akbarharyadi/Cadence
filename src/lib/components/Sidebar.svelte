<script lang="ts">
  import { openPalette } from "#lib/palette.svelte.js";
  import { startExport, startExportAll, startImport } from "#lib/playlistTransfer.svelte.js";
  import { MOD } from "#lib/shortcuts.js";
  import { newSmart } from "#lib/smart.js";
  import { phoneSync } from "#lib/phoneSync.svelte.js";
  import type { Playlist } from "#lib/api.js";
  import { app, type View } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";
  import ProgressBar from "./ui/ProgressBar.svelte";

  let searchInput: HTMLInputElement;

  const library: { label: string; icon: string; view: View }[] = [
    { label: "Recently Added", icon: "clock", view: { name: "recent" } },
    { label: "Albums", icon: "album", view: { name: "albums" } },
    { label: "Artists", icon: "artist", view: { name: "artists" } },
    { label: "Songs", icon: "song", view: { name: "songs" } },
    { label: "Needs Attention", icon: "edit", view: { name: "attention" } },
    { label: "Favorites", icon: "heart", view: { name: "favorites" } },
    { label: "Library Health", icon: "shield", view: { name: "health" } },
  ];

  // What Cadence is doing in the background, one at a time (they run one after another).
  // `unit` "%" shows a share instead of "x of y"; "MB" shows megabytes.
  // `share` (0–1) fills the bar when it should move more finely than `done` (a song at a time).
  const job = $derived<{ label: string; done: number; total: number; about: string; unit?: string; share?: number } | null>(
    app.voice?.installing
      ? {
          label: "Downloading voice models",
          done: Math.round(app.voice.installing[0] / 1e6),
          total: Math.round(app.voice.installing[1] / 1e6),
          unit: "MB",
          about: "Needed once, so lyrics can follow the singer. Everything runs on this computer.",
        }
      : app.voice?.batch?.running
        ? {
            label: app.voice.batch.preparing ? "Finding songs with lyrics" : "Following the singer",
            done: app.voice.batch.done,
            total: app.voice.batch.total,
            share: app.voice.batch.total
              ? (app.voice.batch.done + (app.voice.job ? app.voice.job.done / Math.max(app.voice.job.total, 1) : 0)) / app.voice.batch.total
              : 0,
            unit: "songs",
            about: app.voice.job
              ? `Listening to “${app.voice.job.title}” (${Math.round((app.voice.job.done / Math.max(app.voice.job.total, 1)) * 100)}%). Each song is listened to once.`
              : "Each song is listened to once, in the background.",
          }
        : app.voice?.job
        ? {
            label: "Following the singer",
            done: app.voice.job.done,
            total: app.voice.job.total,
            unit: "%",
            about: `Listening to “${app.voice.job.title}” once, to place its lyrics on the singing.`,
          }
        : app.fixAllJob
        ? {
            label: "Finding song info",
            done: app.fixAllJob.done,
            total: app.fixAllJob.total,
            unit: "songs",
            about: `Looking up songs without info in the iTunes and Deezer catalogues, one every few seconds (${app.fixAllJob.found ?? 0} found so far).`,
          }
        : app.coverJob
        ? {
            label: "Finding covers",
            done: app.coverJob.done,
            total: app.coverJob.total,
            unit: "albums",
            about: `Looking for album covers in the iTunes and Deezer catalogues, one every few seconds (${app.coverJob.found ?? 0} found so far).`,
          }
        : app.artistJob
        ? {
            label: "Checking artist names",
            done: app.artistJob.done,
            total: app.artistJob.total,
            unit: "names",
            about: "Asking the catalogue which artists in your library also go by a name in Latin letters.",
          }
        : app.namesJob
        ? {
            label: "Learning artist names",
            done: app.namesJob.done,
            total: app.namesJob.total,
            unit: "names",
            about: "Asking MusicBrainz what else your artists are called, about one name a second (Settings › Learn artist names online).",
          }
        : app.lbImportJob
        ? {
            label: "Reading ListenBrainz history",
            done: 0,
            total: 0,
            about: `${app.lbImportJob.seen.toLocaleString()} listens read so far, a thousand at a time (Settings › Scrobbling). Nothing is added until you say so.`,
          }
        : app.lyricIndexJob
        ? {
            label: "Indexing lyrics",
            done: app.lyricIndexJob.done,
            total: app.lyricIndexJob.total,
            unit: "songs",
            about: "Reading your saved lyrics once, so Search can find a song by a line of it. Everything stays on this computer.",
          }
        : app.healthJob
        ? {
            label: app.healthJob.kind === "quality" ? "Checking file quality" : "Checking files",
            done: app.healthJob.done,
            total: app.healthJob.total,
            about: app.healthJob.kind === "quality" ? "Looking for lossless files made from lossy ones." : "Decoding every file and checking it against its checksum.",
          }
        : app.backupRunning
        ? {
            label: "Backing up your data",
            done: 0,
            total: 0,
            about: "The day's snapshot of your playlists, favorites, history and settings (Settings › Backup).",
          }
        : app.waveJob
        ? {
            label: app.waveJob.paused ? "Waveforms paused" : "Preparing waveforms",
            done: app.waveJob.done,
            total: app.waveJob.total,
            unit: "songs",
            about: "Drawing each song's waveform once for the seek bar, in the background (Settings › Waveform seek bar).",
          }
        : phoneSync.syncJob
        ? {
            label: phoneSync.syncJob.paused ? "Sync paused" : "Syncing with your computer",
            done: phoneSync.syncJob.done,
            total: phoneSync.syncJob.total,
            unit: "songs",
            about: "Copying the music chosen on your computer to this phone (Settings › Sync with computer).",
          }
        : phoneSync.keysJob
        ? {
            label: phoneSync.keysJob.paused ? "Phone songs paused" : "Preparing songs for your phone",
            done: phoneSync.keysJob.done,
            total: phoneSync.keysJob.total,
            unit: "songs",
            about: "Reading each song kept on the phone once, so the phone can check what it already has (Settings › Sync with phone).",
          }
        : app.scan
      ? { label: "Importing", ...app.scan, about: "" }
      : app.analysis
        ? { label: "Measuring loudness", ...app.analysis, about: "Measured once per song, for Sound Check." }
        : app.sonic?.running
          ? {
              label: "Listening to songs",
              done: app.sonic.analyzed,
              total: app.sonic.total,
              about: "Cadence listens to each song once, in the background, for mixes by sound and Smart EQ.",
            }
          : null,
  );

  /** Lit: the page on screen, or for a page inside a section (an album, an artist), the section it
   *  was opened from (`app.section`). */
  const isActive = (key: string) => app.section === key;

  function onSearch(e: Event) {
    const q = (e.target as HTMLInputElement).value;
    app.search = q;
    if (q.trim().length > 0) {
      if (app.view.name === "search") app.view = { name: "search", query: q };
      else app.go({ name: "search", query: q });
    } else if (app.view.name === "search") app.back();
  }

  /** Escape in the search box: empty it, and leave the results for the page before them. */
  function onSearchKey(e: KeyboardEvent) {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    const wasSearching = app.view.name === "search";
    app.search = "";
    if (wasSearching) app.back();
    searchInput?.blur();
  }

  function newPlaylistMenu(e: MouseEvent) {
    app.openMenu(e, [
      { label: "New playlist", icon: "playlist", action: () => app.newPlaylist() },
      {
        label: "New smart playlist…",
        icon: "smart",
        action: () => (app.smartEditing = { id: "", name: "", trackIds: [], smart: newSmart(), created: 0, updated: 0 }),
      },
      { label: "Import playlist…", icon: "file", action: () => void startImport() },
      ...(app.playlists.length ? [{ label: "Export all playlists…", icon: "download", action: () => startExportAll() }] : []),
    ]);
  }

  /** Right-click on a playlist. */
  function playlistMenu(e: MouseEvent, p: Playlist) {
    // (A smart playlist's rules are worked out in the window, so the phone can't get it yet.)
    const phone = p.smart ? [] : phoneSync.menuItem("playlist", p.id);
    app.openMenu(e, [{ label: "Export…", icon: "download", action: () => startExport(p) }, ...phone]);
  }

</script>

<aside class="sidebar">
  <div class="brand" data-tauri-drag-region>
    <svg class="mark" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 15c2.5-7 4.5-7 6 0s3.5 7 6 0 3.5-7 6 0" /></svg>
    <span>Cadence</span>
  </div>

  <div class="find-row">
    <label class="search" title="Search your music ({MOD} F)">
      <Icon name="search" size={15} />
      <input bind:this={searchInput} data-global-search="sidebar" placeholder="Search your music" value={app.search} oninput={onSearch} onkeydown={onSearchKey} spellcheck="false" />
    </label>
    <button class="palette-btn" onclick={openPalette} aria-label="Command palette" title="Command palette ({MOD} K): pages, actions and your music"><Icon name="more" size={16} /></button>
  </div>

  <nav>
    <button class="item" class:active={isActive("home")} onclick={() => app.go({ name: "home" })}>
      <Icon name="home" size={18} />Listen Now
    </button>
    <button class="item" class:active={isActive("discover")} onclick={() => app.go({ name: "discover" })}>
      <Icon name="sparkle" size={18} />Discover
    </button>
    <button class="item" class:active={isActive("stats")} onclick={() => app.go({ name: "stats" })}>
      <Icon name="gauge" size={18} />Your Stats
    </button>

    <div class="group">Library</div>
    {#each library as item}
      <button class="item" class:active={isActive(item.view.name)} onclick={() => app.go(item.view)}>
        <Icon name={item.icon} size={18} />{item.label}
        {#if item.view.name === "attention" && app.attentionCount > 0}
          <span class="badge" aria-label="{app.attentionCount} to fix">{app.attentionCount > 999 ? "999+" : app.attentionCount}</span>
        {/if}
      </button>
    {/each}

    <div class="group with-action">
      Playlists
      <button class="add" onclick={newPlaylistMenu} aria-label="New playlist" title="New playlist"><Icon name="plus" size={15} /></button>
    </div>
    {#each app.playlists as p (p.id)}
      <button class="item" class:active={isActive(`playlist:${p.id}`)} onclick={() => app.go({ name: "playlist", id: p.id })} oncontextmenu={(e) => playlistMenu(e, p)}>
        <Icon name={p.smart ? "smart" : "playlist"} size={18} /><span class="label">{p.name}</span>
      </button>
    {:else}
      <button class="item muted" onclick={newPlaylistMenu}><Icon name="plus" size={18} />New playlist</button>
    {/each}
  </nav>

  <div class="bottom">
    {#if job}
      <div class="job" title={job.about || undefined}>
        <div class="bar"><ProgressBar value={job.share != null ? job.share : job.total ? job.done / job.total : 0.06} /></div>
        <span>
          {job.label}{!job.total
            ? "…"
            : job.unit === "%"
              ? ` ${Math.round((job.done / job.total) * 100)}%`
              : ` ${job.done} of ${job.total}${job.unit ? ` ${job.unit}` : ""}`}
        </span>
      </div>
    {/if}
    <button class="item" class:active={isActive("sound")} onclick={() => app.go({ name: "sound" })}>
      <Icon name="eq" size={18} />Sound
      {#if app.dsp?.eq.enabled || app.dsp?.crossfeed.enabled}<span class="dot" title="Equalizer or crossfeed is on"></span>{/if}
    </button>
    <button class="item" class:active={isActive("settings")} onclick={() => app.go({ name: "settings" })}>
      <Icon name="settings" size={18} />Settings
    </button>
  </div>
</aside>

<style>
  .sidebar {
    width: 240px;
    flex: none;
    display: flex;
    flex-direction: column;
    /* The player is the window's top bar, so the sidebar runs to the bottom edge. */
    padding: 0 12px 14px;
    background: rgba(16, 17, 26, 0.55);
    border-right: 1px solid var(--line);
    position: relative;
    z-index: 1;
  }
  .brand {
    height: 60px;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 10px 0;
    font-family: var(--display);
    font-weight: 700;
    font-size: 19px;
    letter-spacing: -0.02em;
  }
  .brand > * {
    pointer-events: none;
  }
  .mark {
    width: 24px;
    height: 24px;
    fill: none;
    stroke: var(--accent);
    stroke-width: 2.6;
    stroke-linecap: round;
  }
  .find-row {
    display: flex;
    gap: 6px;
    margin: 6px 0 18px;
  }
  .palette-btn {
    flex: none;
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    border-radius: var(--r-sm);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    color: var(--text-3);
  }
  .palette-btn:hover {
    background: var(--active);
    color: var(--text);
  }
  .search {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 8px 0 11px;
    border-radius: var(--r-sm);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    color: var(--text-3);
  }
  .search:focus-within {
    box-shadow: inset 0 0 0 1px var(--accent);
    color: var(--text-2);
  }
  .search input {
    flex: 1;
    border: 0;
    outline: 0;
    background: none;
    min-width: 0;
    font-size: 13px;
  }
  .search input:focus-visible {
    outline: none;
  }
  .search input::placeholder {
    color: var(--text-3);
  }
  nav {
    flex: 1;
    overflow-y: auto;
  }
  .group {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
    padding: 18px 10px 6px;
  }
  .with-action {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .add {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    display: grid;
    place-items: center;
    color: var(--text-3);
  }
  .add:hover {
    background: var(--active);
    color: var(--text);
  }
  .label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .item.muted {
    color: var(--text-3);
  }
  .item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    height: 36px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    text-align: left;
    color: var(--text-2);
    font-weight: 500;
    transition: background 0.12s, color 0.12s;
  }
  /* How many things Needs Attention has to fix. */
  .badge {
    margin-left: auto;
    min-width: 20px;
    height: 20px;
    padding: 0 6px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    font-size: 11.5px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
    color: var(--text);
    background: color-mix(in srgb, var(--accent) 28%, transparent);
  }
  .item :global(svg) {
    color: var(--text-3);
    transition: color 0.12s;
  }
  .item:hover {
    background: var(--hover);
    color: var(--text);
  }
  .item.active {
    background: var(--active);
    color: var(--text);
  }
  .item.active :global(svg) {
    color: var(--accent);
  }
  .dot {
    margin-left: auto;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }
  .bottom {
    padding-top: 8px;
  }
  .job {
    padding: 4px 10px 12px;
    font-size: 12px;
    color: var(--text-2);
  }
  .job .bar {
    margin-bottom: 7px;
  }
</style>
