<script lang="ts">
  // Needs Attention: what's wrong or missing in the library, one card per kind of problem (worst
  // first), and the open kind's list below. Every fix is the same: Cadence suggests, the listener
  // checks (sure ones ticked, the rest not), saves, and can undo it (Recent changes). Anything can
  // be ignored ("it's how I want it"), and kinds of missing info can be hidden.
  //
  // The lists come from the library as it is (`app.attention`, `app.issues`): duplicates and the
  // file checks from Library Health, catalogue lookups only when the listener starts one (and the
  // artist-name check after a scan).
  import { untrack } from "svelte";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { api, khz, type Album, type ArtistPairs, type ChangeInfo, type CoverFound, type DupGroup, type FoundInfo, type HealthIntegrityRow, type HealthQualityRow, type Track } from "#lib/api.js";
  import { android, app, type SplitAlbum } from "#lib/store.svelte.js";
  import Icon from "#lib/components/Icon.svelte";
  import Modal from "#lib/components/Modal.svelte";

  type Kind = "damaged" | "copies" | "splits" | "names" | "untagged" | "quality" | "covers" | "incomplete" | "year" | "genre";
  interface KindInfo {
    id: Kind;
    title: string;
    /** In the sidebar's count: something is wrong, not just missing. */
    counted: boolean;
    /** Missing info many libraries have on purpose: can be hidden. */
    optional: boolean;
  }
  // Worst first: files that can't play right, copies taking space, wrong info, then missing info.
  const KINDS: KindInfo[] = [
    { id: "damaged", title: "Damaged files", counted: true, optional: false },
    { id: "copies", title: "Extra copies", counted: true, optional: false },
    { id: "splits", title: "Albums split in two", counted: true, optional: false },
    { id: "names", title: "Artists under two names", counted: true, optional: false },
    { id: "untagged", title: "Songs without info", counted: true, optional: false },
    { id: "quality", title: "Lossless files that aren’t", counted: false, optional: true },
    { id: "covers", title: "Albums without a cover", counted: false, optional: true },
    { id: "incomplete", title: "Albums with songs missing", counted: false, optional: true },
    { id: "year", title: "Albums without a year", counted: false, optional: true },
    { id: "genre", title: "Albums without a genre", counted: false, optional: true },
  ];

  const droid = android();
  const issues = $derived(app.issues);
  const songs = (n: number) => `${n} ${n === 1 ? "song" : "songs"}`;
  const albumsN = (n: number) => `${n} ${n === 1 ? "album" : "albums"}`;
  const mb = (bytes: number) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`);
  const mins = (s: number) => `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, "0")}`;
  const folder = (path: string) => path.replace(/[\\/][^\\/]*$/, "");
  const where = (t: Track) => t.path.split(/[\\/]/).slice(-3, -1).join(" / ");
  function formatOf(t: Track): string {
    const lossless = ["FLAC", "ALAC", "WAV", "AIFF", "PCM"].some((c) => t.codec.toUpperCase().startsWith(c));
    if (lossless) return `${t.codec} ${t.bitDepth ?? 16}-bit / ${khz(t.sampleRate ?? 44100)}`;
    return t.codec;
  }
  const reveal = (t: Track) => revealItemInDir(t.path).catch((e) => app.showToast(String(e)));

  let busy = $state<string | null>(null);

  // ---------- artists under two names (asked about after each scan) ----------

  let artistPairs = $state<ArtistPairs | null>(null);
  const loadPairs = async () => {
    artistPairs = await api.artistPairs();
    app.artistPairsCount = artistPairs.pairs.length;
  };
  $effect(() => {
    void app.tracks;
    void app.fixJobsDone;
    loadPairs();
  });
  const pairs = $derived(issues.hidden.has("names") ? [] : (artistPairs?.pairs ?? []));

  // ---------- what the background lookups found, to check ----------

  let found = $state<FoundInfo[]>([]);
  let foundOff = $state<Set<string>>(new Set());
  $effect(() => {
    void app.fixAllFoundCount;
    api.fixAllFound().then((f) => {
      found = f;
      // Sure ones ticked, the rest left for the listener to look at.
      foundOff = new Set(f.filter((x) => !x.sure).map((x) => x.trackId));
    });
  });
  let covers = $state<{ found: CoverFound[]; missed: string[] }>({ found: [], missed: [] });
  let coversOff = $state<Set<string>>(new Set());
  $effect(() => {
    void app.coversFoundCount;
    void app.fixJobsDone;
    api.coversList().then((c) => {
      covers = c;
      coversOff = new Set(c.found.filter((x) => !x.sure).map((x) => x.albumId));
    });
  });

  // Copies to remove: the sure ones ticked.
  let copiesOn = $state<Set<string>>(new Set());
  let copiesSeen = new Set<string>();
  $effect(() => {
    const fresh = issues.copies.filter((g) => !copiesSeen.has(loser(g)));
    if (!fresh.length) return;
    for (const g of fresh) copiesSeen.add(loser(g));
    untrack(() => (copiesOn = new Set([...copiesOn, ...fresh.filter((g) => g.sure).map(loser)])));
  });
  const loser = (g: DupGroup) => g.songs.find((s) => !s.keep)?.id ?? g.songs[0].id;
  const keeper = (g: DupGroup) => g.songs.find((s) => s.keep)?.id ?? g.songs[0].id;

  const toggle = (set: Set<string>, id: string) => {
    const s = new Set(set);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    return s;
  };

  // ---------- the cards ----------

  const count = (k: Kind): number => (k === "names" ? pairs.length : issues[k].length);
  const shown = $derived(KINDS.filter((k) => count(k.id) > 0 || (k.id === "untagged" && found.length > 0) || (k.id === "covers" && covers.found.length > 0)));
  const hiddenKinds = $derived(KINDS.filter((k) => issues.hidden.has(k.id)));
  let open = $state<Kind | null>(null);
  // The first card with something in it, until the listener picks one.
  const current = $derived(open && shown.some((k) => k.id === open) ? open : (shown[0]?.id ?? null));

  /** A one-line summary for each card. */
  function blurb(k: Kind): string {
    const n = count(k);
    switch (k) {
      case "damaged":
        return `Cut off or corrupt: ${songs(n)} that won’t play right`;
      case "copies": {
        const bytes = issues.copies.reduce((s, g) => s + g.wasted, 0);
        return `${songs(n)} you have a better copy of · ${mb(bytes)}`;
      }
      case "splits":
        return `${albumsN(n)} showing up more than once`;
      case "names":
        return `${n} ${n === 1 ? "artist" : "artists"} listed twice`;
      case "untagged":
        return found.length ? `${found.length} found, ready to check` : `No artist or album in ${n === 1 ? "the file" : "the files"}`;
      case "quality":
        return `${songs(n)} made from lossy or CD-quality audio`;
      case "covers":
        return covers.found.length ? `${covers.found.length} found, ready to check` : `${albumsN(n)} without artwork`;
      case "incomplete":
        return `Gaps in the track numbers of ${albumsN(n)}`;
      case "year":
        return `${albumsN(n)}`;
      case "genre":
        return `${albumsN(n)}`;
    }
  }

  // ---------- ignoring ----------

  async function ignore(kind: Kind, keys: string[], label: string) {
    const full = keys.map((k) => `${kind}|${k}`);
    await api.attentionIgnore(full, true);
    app.showToast(label, { label: "Undo", run: () => api.attentionIgnore(full, false) });
  }
  const ignoredOf = (kind: Kind) => app.attentionPrefs.ignored.filter((k) => k.startsWith(`${kind}|`));
  async function hideKind(k: KindInfo) {
    await api.attentionHide(k.id, true);
    app.showToast(`“${k.title}” hidden`, { label: "Undo", run: () => api.attentionHide(k.id, false) });
  }

  // ---------- fixes ----------

  async function removeCopies() {
    const ids = [...copiesOn].filter((id) => issues.copies.some((g) => loser(g) === id));
    if (!ids.length) return;
    busy = "copies";
    try {
      const r = await api.healthRemove(ids);
      copiesOn = new Set([...copiesOn].filter((id) => !ids.includes(id)));
      const left = r.skipped.length ? `; ${r.skipped.length} left alone (${r.skipped[0]}${r.skipped.length > 1 ? ", …" : ""})` : "";
      app.showToast(
        `${r.recycled ? "Moved" : "Deleted"} ${r.deleted} ${r.deleted === 1 ? "copy" : "copies"}${r.recycled ? " to the Recycle Bin" : ""}, ${mb(r.freed)} freed${left}`,
        app.undoAction(r.change),
        8000,
      );
    } catch (e) {
      app.showToast(String(e));
    } finally {
      busy = null;
    }
  }

  /** Puts every song of the split album under `albumArtist`, so it's one album again. */
  async function merge(s: SplitAlbum, albumArtist: string) {
    const ids = s.variants.filter((v) => v.albumArtist !== albumArtist).flatMap((v) => v.tracks.map((t) => t.id));
    busy = s.key;
    try {
      const { later, change } = await api.writeTags(ids, { albumArtist });
      app.showToast(`“${s.album}” is one album now, by ${albumArtist}${later ? ". The playing song’s file is updated when it finishes." : ""}`, app.undoAction(change));
    } catch (e) {
      app.showToast(`Couldn't merge “${s.album}”: ${e}`);
    } finally {
      busy = null;
    }
  }

  /** A rename waiting for the listener to confirm: it changes every song of the artist. */
  let confirming = $state<{ from: string; to: string; songs: number } | null>(null);
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

  async function fixAll() {
    if (!(await api.fixAllStart())) app.showToast("Already looking up songs without info");
  }
  async function saveFound() {
    const ids = found.filter((f) => !foundOff.has(f.trackId)).map((f) => f.trackId);
    busy = "fixall";
    try {
      const r = await api.fixAllSave(ids);
      app.showToast(`Saved the info of ${songs(r.songs)}`, app.undoAction(r.change));
    } catch (e) {
      app.showToast(String(e));
    } finally {
      busy = null;
    }
  }

  async function findCovers() {
    if (!(await api.coversStart(issues.covers.map((a) => a.id)))) app.showToast("Already looking for covers");
  }
  async function saveCovers() {
    const ids = covers.found.filter((c) => !coversOff.has(c.albumId)).map((c) => c.albumId);
    busy = "covers";
    try {
      const r = await api.coversSave(ids);
      await app.loadLibrary();
      app.showToast(`New covers for ${albumsN(r.albums)}${r.failed ? `; ${r.failed} couldn’t be saved` : ""}`, app.undoAction(r.change));
    } catch (e) {
      app.showToast(String(e));
    } finally {
      busy = null;
    }
  }

  async function startCheck(kind: "integrity" | "quality") {
    if (!(await api.healthStart(kind))) app.showToast("A file check is already running");
  }

  function qualityText(r: HealthQualityRow): string {
    const hz = (v: number) => `${(v / 1000).toFixed(1)} kHz`;
    const parts: string[] = [];
    if (r.verdict === "lossy") parts.push(`Nothing above ${hz(r.cutoffHz ?? 0)}: probably made from an MP3 or AAC`);
    else if (r.verdict === "upsampled") parts.push(`A hi-res file with nothing above ${hz(r.cutoffHz ?? 0)}: made from CD-quality audio`);
    else if (r.verdict === "suspect") parts.push(`Stops at ${hz(r.cutoffHz ?? 0)}: maybe from a high-bitrate lossy file, maybe an old recording`);
    if (r.realBits) parts.push(`A 24-bit file that only holds ${r.realBits}-bit audio`);
    return parts.join(". ");
  }
  function integrityText(r: HealthIntegrityRow): string {
    const parts: string[] = [];
    if (r.failed) parts.push(`Can't be read to the end (${r.failed})`);
    if (r.verified === false && !r.truncated) parts.push("The audio doesn't match the checksum stored in the file");
    if (r.errors) parts.push(`${r.errors} ${r.errors === 1 ? "piece" : "pieces"} couldn't be decoded`);
    if (r.truncated) parts.push("Shorter than the file says: cut off");
    return parts.join(". ");
  }

  // ---------- long lists: the first 30, then all ----------

  let full = $state<Set<Kind>>(new Set());
  const page = <T,>(k: Kind, list: T[]) => (full.has(k) ? list : list.slice(0, 30));

  // ---------- recent changes ----------

  let changes = $state<ChangeInfo[]>([]);
  let changesOpen = $state(false);
  $effect(() => {
    void app.changesVersion;
    api.recentChanges().then((c) => (changes = c));
  });
  const when = (at: number) => {
    const m = Math.round((Date.now() / 1000 - at) / 60);
    if (m < 1) return "just now";
    if (m < 60) return `${m} min ago`;
    if (m < 60 * 24) return `${Math.round(m / 60)} h ago`;
    return new Date(at * 1000).toLocaleDateString();
  };

  // ---------- keyboard: ↑/↓ through the rows, Enter fixes, I ignores, Space ticks ----------

  let list = $state<HTMLElement>();
  function keys(e: KeyboardEvent) {
    if (!list || e.ctrlKey || e.metaKey || e.altKey || document.querySelector("[role=dialog]")) return;
    const target = e.target instanceof Element ? e.target : document.body;
    if (target.closest("input:not([type=checkbox]), textarea, [contenteditable]")) return;
    const rows = [...list.querySelectorAll<HTMLElement>("[data-row]")];
    if (!rows.length) return;
    const at = rows.indexOf(document.activeElement as HTMLElement);
    const go = (i: number) => {
      e.preventDefault();
      rows[Math.max(0, Math.min(rows.length - 1, i))].focus();
    };
    if (e.key === "ArrowDown" || e.key === "j") go(at + 1);
    else if (e.key === "ArrowUp" || e.key === "k") go(at < 0 ? 0 : at - 1);
    else if (at >= 0 && e.key === "Enter") {
      e.preventDefault();
      rows[at].querySelector<HTMLElement>("[data-primary]")?.click();
    } else if (at >= 0 && (e.key === "i" || e.key === "I")) {
      e.preventDefault();
      rows[at].querySelector<HTMLElement>("[data-ignore]")?.click();
      // The next row takes its place.
      requestAnimationFrame(() => list?.querySelectorAll<HTMLElement>("[data-row]")[at]?.focus());
    } else if (at >= 0 && e.key === " ") {
      const box = rows[at].querySelector<HTMLInputElement>("input[type=checkbox]");
      if (box) {
        e.preventDefault();
        box.click();
      }
    }
  }

  const health = $derived(app.health);
  const integrityLeft = $derived(health ? health.songs - health.integrityChecked : 0);
  const qualityLeft = $derived(health ? health.lossless - health.qualityChecked : 0);
</script>

<svelte:window onkeydown={keys} />

{#snippet ignoreBtn(kind: Kind, key: string, label: string)}
  <button class="pill ghost small" data-ignore onclick={() => ignore(kind, [key], label)} title="It's how I want it: don't list it again (I)">Ignore</button>
{/snippet}

{#snippet ignoredNote(kind: Kind)}
  {@const ig = ignoredOf(kind)}
  {#if ig.length}
    <p class="ignored">
      {ig.length} ignored ·
      <button class="link" onclick={() => api.attentionIgnore(ig, false)}>Show {ig.length === 1 ? "it" : "them"} again</button>
    </p>
  {/if}
{/snippet}

{#snippet more(kind: Kind, total: number)}
  {#if total > 30 && !full.has(kind)}
    <button class="link more" onclick={() => (full = new Set([...full, kind]))}>Show all {total}</button>
  {/if}
{/snippet}

{#snippet songActions(t: Track)}
  <button class="icon-btn" onclick={() => app.playTracks([t], 0, true)} aria-label="Play {t.title}" title="Play"><Icon name="play" size={15} /></button>
  {#if !droid}
    <button class="icon-btn" onclick={() => reveal(t)} aria-label="Show {t.title} in its folder" title="Show in folder"><Icon name="folder" size={15} /></button>
  {/if}
{/snippet}

{#snippet sureTag(sure: boolean)}
  <span class="sure" class:check={!sure} title={sure ? "Cadence is sure: ticked" : "Probably, but have a look: not ticked"}>{sure ? "Sure" : "Check"}</span>
{/snippet}

<div class="page">
  <header>
    <div>
      <h1 class="page-title">Needs Attention</h1>
      <p class="secondary">What looks wrong or is missing in your library. Nothing changes until you say so, and every change can be undone.</p>
    </div>
    {#if changes.length}
      <button class="pill ghost" onclick={() => (changesOpen = true)}><Icon name="clock" size={15} />Recent changes</button>
    {/if}
  </header>

  {#if !shown.length}
    <div class="clear">
      <Icon name="shield" size={28} />
      <div>
        <h2>Everything looks right</h2>
        <p class="secondary">
          No damaged files, extra copies, split albums or songs without info{#if health && integrityLeft > 0}
            . {integrityLeft} {integrityLeft === 1 ? "file hasn’t" : "files haven’t"} been checked for damage yet:
            <button class="link" onclick={() => startCheck("integrity")}>check now</button>{/if}.
        </p>
      </div>
    </div>
  {:else}
    <div class="cards" role="tablist" aria-label="Kinds of problem">
      {#each shown as k (k.id)}
        <button class="card" class:on={current === k.id} class:counted={k.counted} role="tab" aria-selected={current === k.id} onclick={() => (open = k.id)}>
          <span class="n">{count(k.id) || (k.id === "untagged" ? found.length : covers.found.length)}</span>
          <span class="t">{k.title}</span>
          <span class="b">{blurb(k.id)}</span>
        </button>
      {/each}
    </div>
  {/if}

  <div class="list" bind:this={list}>
    {#if current === "damaged"}
      <section>
        <div class="head">
          <p class="secondary about">
            These files are cut off or don’t match the checksum inside them, so they stop early or click. Get the song again (or play it to hear
            where); the damaged file can then go.
            {#if integrityLeft > 0}{integrityLeft} files not checked yet.{/if}
          </p>
          <div class="batch">
            {#if app.healthJob?.kind === "integrity"}
              <span class="secondary">Checking {app.healthJob.done} of {app.healthJob.total}…</span>
              <button class="pill ghost small" onclick={() => api.healthStop()}>Stop</button>
            {:else if integrityLeft > 0}
              <button class="pill ghost small" disabled={!!app.healthJob} onclick={() => startCheck("integrity")}><Icon name="shield" size={14} />Check the rest</button>
            {/if}
          </div>
        </div>
        {#each page("damaged", issues.damaged) as r (r.id)}
          {@const t = app.trackById.get(r.id)!}
          <div class="row" tabindex="-1" data-row>
            <span class="badge bad">Damaged</span>
            <div class="lines">
              <span class="title">{t.title}</span>
              <span class="sub">{t.artist} · {t.album} · {formatOf(t)}</span>
              <span class="why">{integrityText(r)}</span>
              <span class="sub dim" title={t.path}>{where(t)}</span>
            </div>
            <div class="actions">
              {@render songActions(t)}
              {@render ignoreBtn("damaged", r.id, `“${t.title}” ignored`)}
            </div>
          </div>
        {/each}
        {@render more("damaged", issues.damaged.length)}
        {@render ignoredNote("damaged")}
      </section>
    {:else if current === "copies"}
      <section>
        <div class="head">
          <p class="secondary about">
            Songs you have twice, where one copy is better: lossless over lossy, a tagged file over a download named “Artist – Title”. The better copy
            stays and keeps the favourite and rating; the other goes to the Recycle Bin. <b>Check</b> ones may be another cut (a clip, a music
            video’s intro): compare the lengths before ticking.
          </p>
          <div class="batch">
            <button class="pill small" disabled={busy !== null || !copiesOn.size} onclick={removeCopies} data-batch>
              <Icon name="trash" size={14} />{droid ? "Delete" : "Move"} {copiesOn.size} {copiesOn.size === 1 ? "copy" : "copies"}{droid ? "" : " to the Recycle Bin"}
            </button>
          </div>
        </div>
        {#each page("copies", issues.copies) as g (loser(g))}
          {@const keep = app.trackById.get(keeper(g))}
          {@const go = app.trackById.get(loser(g))}
          {#if keep && go}
            <div class="row copy" tabindex="-1" data-row>
              <input type="checkbox" checked={copiesOn.has(loser(g))} onchange={() => (copiesOn = toggle(copiesOn, loser(g)))} aria-label="Remove the copy of {keep.title}" />
              <div class="lines">
                <span class="title">{keep.title} {@render sureTag(g.sure)}</span>
                <span class="sub"><b class="keep">Keep</b> {keep.artist} · {keep.album} · {formatOf(keep)} · {mins(keep.duration)}</span>
                <span class="sub"
                  ><b class="go">Remove</b>
                  {g.kind === "untagged" ? go.title : `${go.album}`} · {formatOf(go)} · {mins(go.duration)} {#if Math.abs(go.duration - keep.duration) > 3}<span class="diff">
                      ({go.duration > keep.duration ? "+" : "−"}{Math.round(Math.abs(go.duration - keep.duration))} s)</span
                    >{/if}</span
                >
                <span class="sub dim" title={go.path}>{where(go)}</span>
              </div>
              <div class="actions">
                <button class="icon-btn" onclick={() => app.playTracks([go], 0, true)} title="Play the copy to remove" aria-label="Play the copy to remove"><Icon name="play" size={15} /></button>
                {#if !droid}
                  <button class="icon-btn" onclick={() => reveal(go)} title="Show in folder" aria-label="Show the copy in its folder"><Icon name="folder" size={15} /></button>
                {/if}
                {@render ignoreBtn("copies", loser(g), `Both copies of “${keep.title}” stay`)}
              </div>
            </div>
          {/if}
        {/each}
        {@render more("copies", issues.copies.length)}
        {@render ignoredNote("copies")}
      </section>
    {:else if current === "splits"}
      <section>
        <p class="secondary about">
          Songs of the same album, in the same folder, have different album artists, so the album shows up more than once. Pick the album artist
          they should all have.
        </p>
        {#each issues.splits as s (s.key)}
          <div class="row" tabindex="-1" data-row>
            <Icon name="album" size={18} />
            <div class="lines">
              <span class="title">{s.album}</span>
              <span class="sub" title={s.folder}>{s.variants.map((v) => `${v.albumArtist} (${songs(v.tracks.length)})`).join(" · ")}</span>
            </div>
            <div class="actions">
              {#each s.variants.slice(0, 3) as v, i (v.albumArtist)}
                <button class="pill ghost small" data-primary={i === 0 ? true : undefined} disabled={busy !== null} onclick={() => merge(s, v.albumArtist)}>Use {v.albumArtist}</button>
              {/each}
              {#if !s.variants.some((v) => v.albumArtist === "Various Artists")}
                <button class="pill ghost small" disabled={busy !== null} onclick={() => merge(s, "Various Artists")}>Various Artists</button>
              {/if}
              {@render ignoreBtn("splits", s.key, `“${s.album}” left as it is`)}
            </div>
          </div>
        {/each}
        {@render ignoredNote("splits")}
      </section>
    {:else if current === "names"}
      <section>
        <p class="secondary about">
          Some songs name the artist in Japanese or Chinese letters and others in Latin ones, so the artist shows up twice. Pick the name all
          their songs should have. (New names are checked with the catalogue after each scan.)
        </p>
        {#each pairs as p (p.native + p.latin)}
          <div class="row" tabindex="-1" data-row>
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
                <button class="pill ghost small" data-primary disabled={busy !== null} onclick={() => (confirming = { from: p.native, to: p.latin, songs: p.nativeSongs })}>Use {p.latin}</button>
                <button class="pill ghost small" disabled={busy !== null} onclick={() => (confirming = { from: p.latin, to: p.native, songs: p.latinSongs })}>Use {p.native}</button>
                <button class="pill ghost small" data-ignore disabled={busy !== null} onclick={() => notSame(p.native, p.latin)} title="They're different artists: don't suggest this again">Not the same</button>
              {/if}
            </div>
          </div>
        {/each}
        {#if app.artistJob}
          <p class="secondary progress">Checking {app.artistJob.done} of {app.artistJob.total} names… <button class="link" onclick={() => api.artistCheckStop()}>Stop</button></p>
        {:else if artistPairs && artistPairs.unchecked > 0}
          <p class="secondary progress">
            {artistPairs.unchecked} {artistPairs.unchecked === 1 ? "name" : "names"} not checked yet.
            <button class="link" onclick={() => api.artistCheckStart()}>Check now</button>
          </p>
        {/if}
      </section>
    {:else if current === "untagged"}
      <section>
        {#if found.length && !app.fixAllJob}
          <div class="head">
            <p class="secondary about">
              Found in the catalogues. <b>Sure</b> ones (the file name has the same artist) are ticked; <b>Check</b> ones were found by title and length
              only. Nothing is written before you save.
            </p>
            <div class="batch">
              <button class="pill small" data-batch disabled={busy !== null || foundOff.size === found.length} onclick={saveFound}>Save {songs(found.length - foundOff.size)}</button>
              <button class="pill ghost small" disabled={busy !== null} onclick={() => api.fixAllDiscard()}>Discard</button>
            </div>
          </div>
          {#each found as f (f.trackId)}
            <label class="row check" tabindex="-1" data-row>
              <input type="checkbox" checked={!foundOff.has(f.trackId)} onchange={() => (foundOff = toggle(foundOff, f.trackId))} />
              <span class="art">{#if f.cover}<img src={f.cover} alt="" loading="lazy" />{/if}</span>
              <span class="lines">
                <span class="title">{f.title} {@render sureTag(f.sure)}</span>
                <span class="sub">{f.artist} · {f.album}{f.year ? ` (${f.year})` : ""}</span>
                <span class="sub dim" title="What it's called now">now: {f.now}</span>
              </span>
            </label>
          {/each}
          {#if issues.untagged.some((t) => !found.some((f) => f.trackId === t.id))}<h3 class="sub-head">Still without info</h3>{/if}
        {:else}
          <div class="head">
            <p class="secondary about">No artist or album in the file (often downloads named “Artist - Title”). Cadence finds them in the iTunes and Deezer catalogues; you check what it found before anything is saved.</p>
            <div class="batch">
              {#if app.fixAllJob}
                <span class="secondary">Looking up {app.fixAllJob.done} of {app.fixAllJob.total}, {app.fixAllJob.found ?? 0} found…</span>
                <button class="pill ghost small" onclick={() => api.fixAllStop()}>Stop</button>
              {:else}
                <button class="pill small" data-batch onclick={fixAll}><Icon name="sparkle" size={14} />Find info for all {issues.untagged.length}</button>
              {/if}
            </div>
          </div>
        {/if}
        {#each page("untagged", issues.untagged.filter((t) => !found.some((f) => f.trackId === t.id))) as t (t.id)}
          <div class="row" tabindex="-1" data-row>
            <Icon name="song" size={18} />
            <div class="lines">
              <span class="title">{t.title}</span>
              <span class="sub" title={t.path}>{folder(t.path)}</span>
            </div>
            <div class="actions">
              {@render songActions(t)}
              <button class="pill ghost small" data-primary onclick={() => (app.fixing = t)}><Icon name="sparkle" size={14} />Fix song info</button>
              {@render ignoreBtn("untagged", t.id, `“${t.title}” stays as it is`)}
            </div>
          </div>
        {/each}
        {@render more("untagged", issues.untagged.length)}
        {@render ignoredNote("untagged")}
      </section>
    {:else if current === "quality"}
      <section>
        <div class="head">
          <p class="secondary about">
            Lossless files whose sound stops at a lossy encoder’s limit, “hi-res” files made from CD audio, and 24-bit files holding 16-bit audio.
            Leads, not proof: some old recordings really do stop early. A better copy from another source fixes it.
            {#if qualityLeft > 0}{qualityLeft} lossless files not checked yet.{/if}
          </p>
          <div class="batch">
            {#if app.healthJob?.kind === "quality"}
              <span class="secondary">Checking {app.healthJob.done} of {app.healthJob.total}…</span>
              <button class="pill ghost small" onclick={() => api.healthStop()}>Stop</button>
            {:else if qualityLeft > 0}
              <button class="pill ghost small" disabled={!!app.healthJob} onclick={() => startCheck("quality")}><Icon name="wave" size={14} />Check the rest</button>
            {/if}
          </div>
        </div>
        {#each page("quality", issues.quality) as r (r.id)}
          {@const t = app.trackById.get(r.id)!}
          <div class="row" tabindex="-1" data-row>
            <span class="badge {r.verdict}">{r.verdict === "lossy" ? "Lossy source" : r.verdict === "upsampled" ? "Upsampled" : r.verdict === "suspect" ? "Suspect" : "Padded"}</span>
            <div class="lines">
              <span class="title">{t.title}</span>
              <span class="sub">{t.artist} · {t.album} · {formatOf(t)}</span>
              <span class="why">{qualityText(r)}</span>
            </div>
            <div class="actions">
              {@render songActions(t)}
              {@render ignoreBtn("quality", r.id, `“${t.title}” ignored`)}
            </div>
          </div>
        {/each}
        {@render more("quality", issues.quality.length)}
        {@render ignoredNote("quality")}
      </section>
    {:else if current === "covers"}
      <section>
        {#if covers.found.length && !app.coverJob}
          <div class="head">
            <p class="secondary about">
              Found in the catalogues. <b>Sure</b> ones are the same album by the same artist; <b>Check</b> ones match the album’s title only. Saved covers
              go into the songs’ files.
            </p>
            <div class="batch">
              <button class="pill small" data-batch disabled={busy !== null || coversOff.size === covers.found.length} onclick={saveCovers}>
                Save {albumsN(covers.found.length - coversOff.size)}
              </button>
              <button class="pill ghost small" disabled={busy !== null} onclick={() => api.coversDiscard()}>Discard</button>
            </div>
          </div>
          <div class="grid">
            {#each covers.found as c (c.albumId)}
              {@const a = app.albumById.get(c.albumId)}
              {#if a}
                <label class="tile" class:off={coversOff.has(c.albumId)} tabindex="-1" data-row>
                  <img src={c.thumb} alt="" loading="lazy" />
                  <span class="tick"><input type="checkbox" checked={!coversOff.has(c.albumId)} onchange={() => (coversOff = toggle(coversOff, c.albumId))} /></span>
                  <span class="title">{a.title}</span>
                  <span class="sub">{a.artist}</span>
                  <span class="sub dim" title="The catalogue's album">{c.source}: {c.foundTitle} · {c.foundArtist}</span>
                  {@render sureTag(c.sure)}
                </label>
              {/if}
            {/each}
          </div>
          {#if covers.missed.length}
            <p class="secondary progress">No cover found for {albumsN(covers.missed.length)}; Change cover lets you search with other words or pick a file.</p>
          {/if}
          {#if issues.covers.some((a) => !covers.found.some((c) => c.albumId === a.id))}<h3 class="sub-head">Still without a cover</h3>{/if}
        {:else}
          <div class="head">
            <p class="secondary about">Albums without artwork. Cadence can look for each one in the iTunes and Deezer catalogues (one every few seconds, in the background); you check what it found before anything is saved.</p>
            <div class="batch">
              {#if app.coverJob}
                <span class="secondary">Looking for {app.coverJob.done} of {app.coverJob.total}, {app.coverJob.found ?? 0} found…</span>
                <button class="pill ghost small" onclick={() => api.coversStop()}>Stop</button>
              {:else}
                <button class="pill small" data-batch onclick={findCovers}><Icon name="search" size={14} />Find covers for {albumsN(issues.covers.length)}</button>
              {/if}
            </div>
          </div>
        {/if}
        {#each page("covers", issues.covers.filter((a) => !covers.found.some((c) => c.albumId === a.id))) as a (a.id)}
          <div class="row" tabindex="-1" data-row>
            <span class="blank"><Icon name="album" size={18} /></span>
            <div class="lines">
              <button class="title link-like" onclick={() => app.go({ name: "album", id: a.id })}>{a.title}</button>
              <span class="sub">{a.artist}{a.year ? ` · ${a.year}` : ""} · {songs(a.trackIds.length)}{covers.missed.includes(a.id) ? " · none found" : ""}</span>
            </div>
            <div class="actions">
              <button class="pill ghost small" data-primary onclick={() => (app.coverEditing = a.id)}><Icon name="album" size={14} />Change cover</button>
              {@render ignoreBtn("covers", a.id, `“${a.title}” stays without a cover`)}
            </div>
          </div>
        {/each}
        {@render more("covers", issues.covers.length)}
        {@render ignoredNote("covers")}
      </section>
    {:else if current === "incomplete"}
      <section>
        <p class="secondary about">The track numbers have gaps, so songs of the album are missing (or numbered wrongly). Fix Album Info lines the files up with the album’s real track list.</p>
        {#each page("incomplete", issues.incomplete) as x (x.album.id)}
          {@render albumRow("incomplete", x.album, `Missing track ${x.missing.slice(0, 8).join(", ")}${x.missing.length > 8 ? ", …" : ""}`)}
        {/each}
        {@render more("incomplete", issues.incomplete.length)}
        {@render ignoredNote("incomplete")}
      </section>
    {:else if current === "year" || current === "genre"}
      {@const k = current}
      <section>
        <p class="secondary about">No {k === "year" ? "release year" : "genre"} in any of the album’s songs. Fix Album Info fills it in from the catalogue, with the rest of the album’s info.</p>
        {#each page(k, issues[k]) as a (a.id)}
          {@render albumRow(k, a, "")}
        {/each}
        {@render more(k, issues[k].length)}
        {@render ignoredNote(k)}
      </section>
    {/if}

    {#if current}
      {@const k = KINDS.find((x) => x.id === current)}
      {#if k?.optional}
        <p class="ignored"><button class="link" onclick={() => hideKind(k)}>Don’t show “{k.title}”</button></p>
      {/if}
    {/if}
  </div>

  {#if hiddenKinds.length}
    <p class="ignored hidden-kinds">
      Hidden:
      {#each hiddenKinds as k, i (k.id)}
        {i ? " · " : ""}<button class="link" onclick={() => api.attentionHide(k.id, false)} title="Show again">{k.title}</button>
      {/each}
    </p>
  {/if}
  {#if shown.length}
    <p class="keys secondary">Keys: ↑ ↓ move · Enter fix · Space tick · I ignore</p>
  {/if}
</div>

{#snippet albumRow(kind: Kind, a: Album, note: string)}
  <div class="row" tabindex="-1" data-row>
    <span class="art">{#if a.artSmall}<img src={a.artSmall} alt="" loading="lazy" />{/if}</span>
    <div class="lines">
      <button class="title link-like" onclick={() => app.go({ name: "album", id: a.id })}>{a.title}</button>
      <span class="sub">{a.artist}{a.year ? ` · ${a.year}` : ""} · {songs(a.trackIds.length)}</span>
      {#if note}<span class="why">{note}</span>{/if}
    </div>
    <div class="actions">
      <button class="pill ghost small" data-primary onclick={() => (app.fixingAlbum = a)}><Icon name="sparkle" size={14} />Fix album info</button>
      {@render ignoreBtn(kind, a.id, `“${a.title}” left as it is`)}
    </div>
  </div>
{/snippet}

{#if changesOpen}
  <Modal title="Recent changes" onclose={() => (changesOpen = false)} width="560px">
    <p class="secondary about">What Cadence changed in your songs’ files, newest first. Undo puts them back as they were (covers too); removed copies come back from the Recycle Bin.</p>
    {#each changes as c (c.id)}
      <div class="row">
        <Icon name="clock" size={18} />
        <div class="lines">
          <span class="title">{c.label}</span>
          <span class="sub">{when(c.at)} · {c.songs} {c.songs === 1 ? "file" : "files"}</span>
        </div>
        <div class="actions">
          <button class="pill ghost small" onclick={() => app.undoChange(c.id)}>Undo</button>
        </div>
      </div>
    {/each}
  </Modal>
{/if}

<style>
  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
    margin-bottom: 18px;
  }
  header .secondary {
    margin: 4px 0 0;
    max-width: 70ch;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
    gap: 10px;
    margin-bottom: 18px;
  }
  .card {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 12px 14px;
    border-radius: var(--r-md);
    background: var(--hover);
    border: 1px solid transparent;
    text-align: left;
    color: var(--text-2);
    transition: background-color 0.12s, border-color 0.12s;
  }
  .card:hover {
    background: var(--active);
  }
  .card.on {
    border-color: color-mix(in srgb, var(--accent) 70%, transparent);
    background: color-mix(in srgb, var(--accent) 10%, var(--hover));
  }
  .card .n {
    font-family: var(--display);
    font-size: 24px;
    font-weight: 700;
    color: var(--text-3);
    line-height: 1.1;
  }
  .card.counted .n {
    color: var(--accent);
  }
  .card .t {
    font-weight: 600;
    color: var(--text);
    font-size: 14px;
  }
  .card .b {
    font-size: 12px;
  }
  .clear {
    display: flex;
    gap: 16px;
    align-items: center;
    padding: 22px;
    border-radius: var(--r-md);
    background: var(--hover);
    color: var(--accent);
  }
  .clear h2 {
    margin: 0 0 4px;
    font-family: var(--display);
    font-size: 18px;
    color: var(--text);
  }
  .clear p {
    margin: 0;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }
  .about {
    margin: 0 0 10px;
    font-size: 13px;
    max-width: 78ch;
    flex: 1 1 380px;
  }
  .batch {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    flex: none;
  }
  .sub-head {
    margin: 22px 0 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
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
    outline: none;
  }
  .row:hover,
  .row:focus-visible,
  .row:focus {
    background: var(--hover);
  }
  .row:focus-visible {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .row.check,
  .tile {
    cursor: pointer;
  }
  .blank {
    width: 18px;
    flex: none;
    display: grid;
    place-items: center;
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
  .sub.dim {
    color: var(--text-3);
  }
  .why {
    font-size: 12.5px;
    color: var(--text);
  }
  .keep,
  .go {
    display: inline-block;
    width: 52px;
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .keep {
    color: var(--accent);
  }
  .go {
    color: #ef7d8d;
  }
  .diff {
    color: #e3b45a;
  }
  .sure {
    display: inline-block;
    margin-left: 6px;
    padding: 1px 7px;
    border-radius: 999px;
    font-size: 10.5px;
    font-weight: 700;
    vertical-align: 1px;
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--accent);
  }
  .sure.check {
    background: rgba(214, 160, 60, 0.2);
    color: #e3b45a;
  }
  .badge {
    flex: none;
    width: 92px;
    text-align: center;
    padding: 3px 0;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 700;
    background: color-mix(in srgb, var(--accent) 22%, transparent);
    color: var(--accent);
  }
  .badge.bad,
  .badge.lossy {
    background: rgba(201, 72, 91, 0.2);
    color: #ef7d8d;
  }
  .badge.suspect {
    background: rgba(214, 160, 60, 0.2);
    color: #e3b45a;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
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
    font-size: 13px;
    color: var(--text);
    margin-right: 4px;
  }
  input[type="checkbox"] {
    accent-color: var(--accent);
    width: 16px;
    height: 16px;
    flex: none;
  }
  .art {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 14px;
  }
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px;
    border-radius: var(--r-md);
    outline: none;
    min-width: 0;
  }
  .tile:hover,
  .tile:focus {
    background: var(--hover);
  }
  .tile img {
    width: 100%;
    aspect-ratio: 1;
    object-fit: cover;
    border-radius: 6px;
    margin-bottom: 4px;
    transition: opacity 0.15s;
  }
  .tile.off img {
    opacity: 0.35;
  }
  .tile .tick {
    position: absolute;
    top: 14px;
    left: 14px;
  }
  .tile .sure {
    align-self: flex-start;
    margin: 2px 0 0;
  }
  .progress {
    margin: 10px;
    font-size: 13px;
  }
  .ignored {
    margin: 10px;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .hidden-kinds {
    margin-top: 24px;
  }
  .link {
    color: var(--accent);
  }
  .link:hover {
    text-decoration: underline;
  }
  .more {
    margin: 6px 10px 0;
    font-size: 13px;
  }
  .keys {
    margin: 28px 10px 0;
    font-size: 12px;
    opacity: 0.7;
  }
  .small {
    padding: 4px 11px;
    font-size: 12.5px;
    gap: 5px;
  }
  @media (max-width: 720px) {
    header {
      flex-direction: column;
    }
    .keys {
      display: none;
    }
  }
</style>
