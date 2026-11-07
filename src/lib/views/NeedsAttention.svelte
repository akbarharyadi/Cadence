<script lang="ts">
  // Needs Attention: the library's info problems, each with its fix. Albums split in two (songs of
  // one album, in one folder, under different album artists), songs without tags, and albums
  // without a cover. Worked out from the library as it is; nothing runs in the background.
  import { api, type Album, type Track } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import Icon from "#lib/components/Icon.svelte";

  const fold = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase().trim();
  const folder = (path: string) => path.replace(/[\\/][^\\/]*$/, "");

  interface Split {
    key: string;
    album: string;
    folder: string;
    /** Each album artist the songs are under, most songs first. */
    variants: { albumArtist: string; tracks: Track[] }[];
  }

  /** One album (same name, same folder) shown as several: its songs have different album artists. */
  const splits = $derived.by(() => {
    const groups = new Map<string, Track[]>();
    for (const t of app.tracks) {
      // Songs without tags are grouped under their folder's name: they're listed below instead.
      if (app.needsInfo(t) || !t.album.trim()) continue;
      const key = `${folder(t.path)}|${fold(t.album)}`;
      const g = groups.get(key);
      if (g) g.push(t);
      else groups.set(key, [t]);
    }
    const out: Split[] = [];
    for (const [key, tracks] of groups) {
      if (new Set(tracks.map((t) => t.albumId)).size < 2) continue;
      const by = new Map<string, Track[]>();
      for (const t of tracks) by.set(t.albumArtist, [...(by.get(t.albumArtist) ?? []), t]);
      const variants = [...by].map(([albumArtist, tracks]) => ({ albumArtist, tracks })).sort((a, b) => b.tracks.length - a.tracks.length);
      out.push({ key, album: tracks[0].album, folder: folder(tracks[0].path), variants });
    }
    return out.sort((a, b) => a.album.localeCompare(b.album));
  });

  const untagged = $derived(app.tracks.filter((t) => app.needsInfo(t)));
  let allUntagged = $state(false);

  /** Real albums (not the made-up ones of songs without tags) without a cover. */
  const coverless = $derived(app.albums.filter((a: Album) => !a.art && a.artist !== "Unknown Artist"));
  let allCoverless = $state(false);

  let busy = $state<string | null>(null);

  /** Puts every song of the split album under `albumArtist`, so it's one album again. */
  async function merge(s: Split, albumArtist: string) {
    const ids = s.variants.filter((v) => v.albumArtist !== albumArtist).flatMap((v) => v.tracks.map((t) => t.id));
    busy = s.key;
    try {
      const { later } = await api.writeTags(ids, { albumArtist });
      app.showToast(`“${s.album}” is one album now, by ${albumArtist}${later ? ". The playing song’s file is updated when it finishes." : ""}`);
    } catch (e) {
      app.showToast(`Couldn't merge “${s.album}”: ${e}`);
    } finally {
      busy = null;
    }
  }

  const songs = (n: number) => `${n} ${n === 1 ? "song" : "songs"}`;
  const nothing = $derived(!splits.length && !untagged.length && !coverless.length);
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

  {#if untagged.length}
    <section>
      <h2 class="section-title">Songs without info <span class="n">{untagged.length}</span></h2>
      <p class="secondary about">No artist or album in the file (often downloads named “Artist - Title”). Fix song info finds them in the iTunes and Deezer catalogues.</p>
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
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
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
    flex: 1;
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
    gap: 6px;
    flex: none;
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
