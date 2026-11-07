<script lang="ts">
  // Needs Attention: the library's info problems, each with its fix. Albums split in two (songs of
  // one album, in one folder, under different album artists), artists under two names (米津玄師 /
  // Kenshi Yonezu), songs without tags, and albums without a cover. Worked out from the library as
  // it is (`app.attention`); the catalogue is only asked when the user starts Fix all or the artist
  // check. Every change here can be undone (Recent changes, at the bottom).
  import { api, type ArtistPairs, type ChangeInfo, type FoundInfo } from "#lib/api.js";
  import { app, type SplitAlbum } from "#lib/store.svelte.js";
  import Icon from "#lib/components/Icon.svelte";

  const folder = (path: string) => path.replace(/[\\/][^\\/]*$/, "");
  const splits = $derived(app.attention.splits);
  const untagged = $derived(app.attention.untagged);
  const coverless = $derived(app.attention.coverless);
  let allUntagged = $state(false);
  let allCoverless = $state(false);

  let busy = $state<string | null>(null);
  const songs = (n: number) => `${n} ${n === 1 ? "song" : "songs"}`;

  /** Puts every song of the split album under `albumArtist`, so it's one album again. */
  async function merge(s: SplitAlbum, albumArtist: string) {
    const ids = s.variants.filter((v) => v.albumArtist !== albumArtist).flatMap((v) => v.tracks.map((t) => t.id));
    busy = s.key;
    try {
      const { later, change } = await api.writeTags(ids, { albumArtist });
      app.showToast(
        `“${s.album}” is one album now, by ${albumArtist}${later ? ". The playing song’s file is updated when it finishes." : ""}`,
        app.undoAction(change),
      );
    } catch (e) {
      app.showToast(`Couldn't merge “${s.album}”: ${e}`);
    } finally {
      busy = null;
    }
  }

  // ---------- artists under two names ----------

  /** Artists under two names, from the catalogue check (kept between runs). */
  let artistPairs = $state<ArtistPairs | null>(null);
  const loadPairs = async () => {
    artistPairs = await api.artistPairs();
    app.artistPairsCount = artistPairs.pairs.length;
  };
  // Again whenever the library changes or a check ends.
  $effect(() => {
    void app.tracks;
    void app.fixJobsDone;
    loadPairs();
  });

  async function checkArtists() {
    if (!(await api.artistCheckStart())) app.showToast("Already checking artist names");
  }

  /** A rename waiting for the user to confirm: it changes every song of the artist. */
  let confirming = $state<{ from: string; to: string; songs: number } | null>(null);

  /** Writes `to` in place of `from` in every song, so the artist shows up once. */
  async function mergeArtist(from: string, to: string) {
    confirming = null;
    busy = `artist|${from}`;
    try {
      const r = await api.mergeArtist(from, to);
      app.showToast(`${songs(r.songs)} now say “${to}” instead of “${from}”`, app.undoAction(r.change));
    } catch (e) {
      app.showToast(`Couldn't rename “${from}”: ${e}`);
    } finally {
      busy = null;
      loadPairs();
    }
  }

  async function notSame(native: string, latin: string) {
    await api.artistPairDismiss(native, latin);
    loadPairs();
  }

  // ---------- Fix all: find, check, save ----------

  /** What Fix all found, to check before it's saved; unticked songs are left as they are. */
  let found = $state<FoundInfo[]>([]);
  let skipped = $state<Set<string>>(new Set());
  $effect(() => {
    void app.fixAllFoundCount;
    api.fixAllFound().then((f) => (found = f));
  });

  async function fixAll() {
    if (!(await api.fixAllStart())) app.showToast("Already looking up songs without info");
  }

  async function saveFound() {
    const ids = found.filter((f) => !skipped.has(f.trackId)).map((f) => f.trackId);
    busy = "fixall";
    try {
      const r = await api.fixAllSave(ids);
      skipped = new Set();
      app.showToast(`Saved the info of ${songs(r.songs)}`, app.undoAction(r.change));
    } catch (e) {
      app.showToast(String(e));
    } finally {
      busy = null;
    }
  }

  async function discardFound() {
    await api.fixAllDiscard();
    skipped = new Set();
  }

  const toggleSkip = (id: string) => {
    const s = new Set(skipped);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    skipped = s;
  };

  // ---------- recent changes ----------

  let changes = $state<ChangeInfo[]>([]);
  let allChanges = $state(false);
  $effect(() => {
    void app.changesVersion;
    api.recentChanges().then((c) => (changes = c));
  });
  const when = (at: number) => {
    const mins = Math.round((Date.now() / 1000 - at) / 60);
    if (mins < 1) return "just now";
    if (mins < 60) return `${mins} min ago`;
    if (mins < 60 * 24) return `${Math.round(mins / 60)} h ago`;
    return new Date(at * 1000).toLocaleDateString();
  };

  const nothing = $derived(!splits.length && !untagged.length && !coverless.length && !artistPairs?.pairs.length && !found.length);
</script>

<div class="page">
  <header>
    <h1 class="page-title">Needs Attention</h1>
    <p class="secondary">Info in your library that looks wrong or is missing, with a way to fix each.</p>
  </header>

  {#if nothing}
    <p class="secondary empty">Nothing needs attention: every album is whole, every song has its info, and every album has a cover.</p>
  {/if}

  {#if splits.length}
    <section>
      <h2 class="section-title">Albums split in two <span class="n">{splits.length}</span></h2>
      <p class="secondary about">
        Songs of the same album, in the same folder, have different album artists, so the album shows up more than once. Pick the album
        artist they should all have.
      </p>
      {#each splits as s (s.key)}
        <div class="row">
          <Icon name="album" size={18} />
          <div class="lines">
            <span class="title">{s.album}</span>
            <span class="sub" title={s.folder}>{s.variants.map((v) => `${v.albumArtist} (${songs(v.tracks.length)})`).join(" · ")}</span>
          </div>
          <div class="actions">
            {#each s.variants.slice(0, 3) as v (v.albumArtist)}
              <button class="pill ghost small" disabled={busy !== null} onclick={() => merge(s, v.albumArtist)} title="Make “{v.albumArtist}” the album artist of all {songs(s.variants.reduce((n, x) => n + x.tracks.length, 0))}">
                Use {v.albumArtist}
              </button>
            {/each}
            {#if !s.variants.some((v) => v.albumArtist === "Various Artists")}
              <button class="pill ghost small" disabled={busy !== null} onclick={() => merge(s, "Various Artists")}>Various Artists</button>
            {/if}
          </div>
        </div>
      {/each}
    </section>
  {/if}

  {#if artistPairs?.pairs.length}
    <section>
      <h2 class="section-title">Same artist under two names <span class="n">{artistPairs.pairs.length}</span></h2>
      <p class="secondary about">
        Some songs name the artist in Japanese or Chinese letters and others in Latin ones, so the artist shows up twice. Pick the name
        all their songs should have.
      </p>
      {#each artistPairs.pairs as p (p.native + p.latin)}
        <div class="row">
          <Icon name="artist" size={18} />
          <div class="lines">
            <span class="title">{p.native} · {p.latin}</span>
            <span class="sub">{p.native} ({songs(p.nativeSongs)}) · {p.latin} ({songs(p.latinSongs)})</span>
          </div>
          <div class="actions">
            {#if confirming && (confirming.from === p.native || confirming.from === p.latin)}
              {@const c = confirming}
              <span class="confirm">Rename “{c.from}” to “{c.to}” in {songs(c.songs)}?</span>
              <button class="pill small" disabled={busy !== null} onclick={() => mergeArtist(c.from, c.to)}>Rename</button>
              <button class="pill ghost small" onclick={() => (confirming = null)}>Cancel</button>
            {:else}
              <button class="pill ghost small" disabled={busy !== null} onclick={() => (confirming = { from: p.latin, to: p.native, songs: p.latinSongs })}>Use {p.native}</button>
              <button class="pill ghost small" disabled={busy !== null} onclick={() => (confirming = { from: p.native, to: p.latin, songs: p.nativeSongs })}>Use {p.latin}</button>
              <button class="pill ghost small" disabled={busy !== null} onclick={() => notSame(p.native, p.latin)} title="They're different artists: don't suggest this again">Not the same</button>
            {/if}
          </div>
        </div>
      {/each}
    </section>
  {/if}

  {#if artistPairs && (artistPairs.unchecked > 0 || app.artistJob)}
    <section>
      <h2 class="section-title">Artist names</h2>
      <p class="secondary about">
        {#if app.artistJob}
          Checking {app.artistJob.done} of {app.artistJob.total} names in Japanese or Chinese letters; artists also listed under a Latin name will show up above.
        {:else}
          {artistPairs.unchecked} artist {artistPairs.unchecked === 1 ? "name is" : "names are"} in Japanese or Chinese letters. The catalogue can tell which
          of them are also in your library under a Latin name (about 6 seconds a name, in the background; only the names are sent).
        {/if}
      </p>
      <div class="actions inline">
        {#if app.artistJob}
          <button class="pill ghost small" onclick={() => api.artistCheckStop()}>Stop</button>
        {:else}
          <button class="pill ghost small" onclick={checkArtists}><Icon name="search" size={14} />Check artist names</button>
        {/if}
      </div>
    </section>
  {/if}

  {#if found.length && !app.fixAllJob}
    <section>
      <h2 class="section-title">Found info to check <span class="n">{found.length}</span></h2>
      <p class="secondary about">Fix all found these songs in the catalogues. Untick any that are wrong, then save; nothing is written before that.</p>
      {#each found as f (f.trackId)}
        <label class="row check">
          <input type="checkbox" checked={!skipped.has(f.trackId)} onchange={() => toggleSkip(f.trackId)} />
          <span class="art">{#if f.cover}<img src={f.cover} alt="" loading="lazy" />{/if}</span>
          <span class="lines">
            <span class="title">{f.title}</span>
            <span class="sub">{f.artist} · {f.album}{f.year ? ` (${f.year})` : ""}</span>
            <span class="sub was" title="What it's called now">now: {f.now}</span>
          </span>
        </label>
      {/each}
      <div class="actions inline">
        <button class="pill small" disabled={busy !== null || skipped.size === found.length} onclick={saveFound}>
          Save {songs(found.length - skipped.size)}
        </button>
        <button class="pill ghost small" disabled={busy !== null} onclick={discardFound}>Discard</button>
      </div>
    </section>
  {/if}

  {#if untagged.length}
    <section>
      <h2 class="section-title">Songs without info <span class="n">{untagged.length}</span></h2>
      <p class="secondary about">No artist or album in the file (often downloads named “Artist - Title”). Fix song info finds them in the iTunes and Deezer catalogues.</p>
      <div class="actions inline">
        {#if app.fixAllJob}
          <span class="secondary">Looking up {app.fixAllJob.done} of {app.fixAllJob.total}, {app.fixAllJob.found ?? 0} found…</span>
          <button class="pill ghost small" onclick={() => api.fixAllStop()}>Stop</button>
        {:else if !found.length}
          <button class="pill ghost small" onclick={fixAll} title="Looks each one up in the background, one every few seconds; you check what it finds before anything is saved">
            <Icon name="sparkle" size={14} />Find info for all {untagged.length}
          </button>
        {/if}
      </div>
      {#each allUntagged ? untagged : untagged.slice(0, 20) as t (t.id)}
        <div class="row">
          <Icon name="song" size={18} />
          <div class="lines">
            <span class="title">{t.title}</span>
            <span class="sub" title={t.path}>{folder(t.path)}</span>
          </div>
          <div class="actions">
            <button class="pill ghost small" onclick={() => (app.fixing = t)}><Icon name="sparkle" size={14} />Fix song info</button>
          </div>
        </div>
      {/each}
      {#if untagged.length > 20 && !allUntagged}
        <button class="link more" onclick={() => (allUntagged = true)}>Show all {untagged.length}</button>
      {/if}
    </section>
  {/if}

  {#if coverless.length}
    <section>
      <h2 class="section-title">Albums without a cover <span class="n">{coverless.length}</span></h2>
      {#each allCoverless ? coverless : coverless.slice(0, 20) as a (a.id)}
        <div class="row">
          <span class="blank"></span>
          <div class="lines">
            <button class="title link-like" onclick={() => app.go({ name: "album", id: a.id })}>{a.title}</button>
            <span class="sub">{a.artist}{a.year ? ` · ${a.year}` : ""} · {songs(a.trackIds.length)}</span>
          </div>
          <div class="actions">
            <button class="pill ghost small" onclick={() => (app.coverEditing = a.id)}><Icon name="album" size={14} />Change cover</button>
          </div>
        </div>
      {/each}
      {#if coverless.length > 20 && !allCoverless}
        <button class="link more" onclick={() => (allCoverless = true)}>Show all {coverless.length}</button>
      {/if}
    </section>
  {/if}

  {#if changes.length}
    <section>
      <h2 class="section-title">Recent changes</h2>
      <p class="secondary about">What Cadence changed in your songs’ files. Undo puts them back as they were, cover included.</p>
      {#each allChanges ? changes : changes.slice(0, 5) as c (c.id)}
        <div class="row">
          <Icon name="clock" size={18} />
          <div class="lines">
            <span class="title">{c.label}</span>
            <span class="sub">{when(c.at)} · {songs(c.songs)}</span>
          </div>
          <div class="actions">
            <button class="pill ghost small" onclick={() => app.undoChange(c.id)}>Undo</button>
          </div>
        </div>
      {/each}
      {#if changes.length > 5 && !allChanges}
        <button class="link more" onclick={() => (allChanges = true)}>Show all {changes.length}</button>
      {/if}
    </section>
  {/if}
</div>

<style>
  header {
    margin-bottom: 8px;
  }
  header .secondary {
    margin: 4px 0 0;
  }
  section {
    margin-top: 26px;
  }
  .n {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-3);
    margin-left: 6px;
  }
  .about {
    margin: -4px 0 10px;
    font-size: 13px;
    max-width: 70ch;
  }
  .empty {
    margin-top: 18px;
  }
  /* Long names don't squeeze the title: the buttons go onto the next line instead. */
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    color: var(--text-3);
    min-width: 0;
  }
  .row:hover {
    background: var(--hover);
  }
  .blank {
    width: 18px;
    flex: none;
  }
  .lines {
    flex: 1 1 260px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .title,
  .sub {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }
  .title {
    color: var(--text);
    font-weight: 600;
    font-size: 14px;
  }
  .link-like:hover {
    text-decoration: underline;
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
    flex: 0 1 auto;
    max-width: 100%;
    margin-left: auto;
  }
  .actions .pill {
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .confirm {
    align-self: center;
    font-size: 13px;
    color: var(--text);
    margin-right: 4px;
  }
  .row.check {
    cursor: pointer;
  }
  .row .art {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
  }
  .row .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .sub.was {
    color: var(--text-3);
  }
  .actions.inline {
    align-items: center;
    gap: 10px;
    margin: 0 10px 8px;
    font-size: 13px;
  }
  .small {
    padding: 4px 11px;
    font-size: 12.5px;
    gap: 5px;
  }
  .more {
    margin: 6px 10px 0;
    font-size: 13px;
  }
</style>
