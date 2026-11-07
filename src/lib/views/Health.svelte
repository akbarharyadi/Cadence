<script lang="ts">
  import { onMount } from "svelte";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { api, khz, type DupGroup, type HealthIntegrityRow, type HealthKind, type HealthQualityRow, type Track } from "#lib/api.js";
  import Icon from "#lib/components/Icon.svelte";
  import Modal from "#lib/components/Modal.svelte";
  import { android, app } from "#lib/store.svelte.js";

  const droid = android();

  onMount(() => app.loadHealth());

  const mb = (bytes: number) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`);
  const hz = (v: number) => `${(v / 1000).toFixed(1)} kHz`;

  /** "FLAC 16-bit / 44.1 kHz" or "MP3 320 kbps". */
  function formatOf(t: Track): string {
    const lossless = ["FLAC", "ALAC", "WAV", "AIFF", "PCM"].some((c) => t.codec.toUpperCase().startsWith(c));
    if (lossless) return `${t.codec} ${t.bitDepth ?? 16}-bit / ${khz(t.sampleRate ?? 44100)}`;
    return `${t.codec}${t.bitrate ? ` ${t.bitrate} kbps` : ""}`;
  }
  /** The last two folders of a file's path. */
  const where = (t: Track) => t.path.split(/[\\/]/).slice(-3, -1).join(" / ");

  const reveal = (t: Track) => revealItemInDir(t.path).catch((e) => app.showToast(String(e)));

  // ---------- duplicates ----------

  let groups = $state<DupGroup[] | null>(null);
  let unchecked = $state<string[]>([]);
  let shown = $state(25);
  let confirming = $state(false);
  let removing = $state(false);

  async function loadDuplicates() {
    groups = await api.healthDuplicates();
    unchecked = [];
  }
  // Again whenever the library changes (after deleting, or a rescan).
  $effect(() => {
    app.tracks;
    loadDuplicates();
  });

  const losers = $derived((groups ?? []).flatMap((g) => g.songs.filter((s) => !s.keep).map((s) => s.id)));
  const chosen = $derived(losers.filter((id) => !unchecked.includes(id)));
  const chosenBytes = $derived(chosen.reduce((n, id) => n + (app.trackById.get(id)?.size ?? 0), 0));
  const wasted = $derived((groups ?? []).reduce((n, g) => n + g.wasted, 0));

  function toggle(id: string) {
    unchecked = unchecked.includes(id) ? unchecked.filter((x) => x !== id) : [...unchecked, id];
  }

  async function removeChosen() {
    removing = true;
    try {
      const r = await api.healthRemove(chosen);
      confirming = false;
      app.showToast(
        `Deleted ${r.deleted} ${r.deleted === 1 ? "file" : "files"}, freed ${mb(r.freed)}${r.skipped.length ? `; ${r.skipped.length} left alone` : ""}`,
        null,
        7000,
      );
      if (r.skipped.length) console.warn("health: left alone", r.skipped);
    } catch (e) {
      app.showToast(String(e));
    } finally {
      removing = false;
    }
  }

  // ---------- file checks ----------

  const report = $derived(app.health);
  const job = $derived(app.healthJob);

  async function start(kind: HealthKind) {
    if (!(await api.healthStart(kind))) app.showToast("A check is already running.");
  }

  function qualityText(r: HealthQualityRow): string {
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
    if (r.verified === false) parts.push("The audio doesn't match the checksum stored in the file");
    if (r.errors) parts.push(`${r.errors} ${r.errors === 1 ? "piece" : "pieces"} couldn't be decoded`);
    if (r.truncated) parts.push("Shorter than the file says: cut off");
    return parts.join(". ");
  }

  const qualityRows = $derived((report?.quality ?? []).filter((r) => app.trackById.has(r.id)));
  const integrityRows = $derived((report?.integrity ?? []).filter((r) => app.trackById.has(r.id)));
  const qualityLeft = $derived(report ? report.lossless - report.qualityChecked : 0);
  const integrityLeft = $derived(report ? report.songs - report.integrityChecked : 0);
  let showAllQuality = $state(false);
  let showAllIntegrity = $state(false);
</script>

{#snippet songLine(t: Track)}
  <div class="song">
    <div class="who"><b>{t.title}</b><span>{t.artist} · {t.album}</span></div>
  </div>
{/snippet}

{#snippet rowActions(t: Track)}
  <button class="icon-btn" onclick={() => app.playTracks([t], 0, true)} aria-label="Play {t.title}" title="Play"><Icon name="play" size={15} /></button>
  {#if !droid}
    <button class="icon-btn" onclick={() => reveal(t)} aria-label="Show {t.title} in its folder" title="Show in folder"><Icon name="folder" size={15} /></button>
  {/if}
{/snippet}

<div class="page">
  <h1 class="page-title">Library Health</h1>
  <p class="secondary lead">Finds copies you don't need, lossless files that aren't, and damaged files. Nothing is changed unless you press Delete.</p>

  <!-- Duplicates -->
  <section class="card">
    <div class="head">
      <div>
        <h2>Duplicates</h2>
        {#if groups === null}
          <p class="secondary">Looking…</p>
        {:else if groups.length === 0}
          <p class="secondary">No songs in your library more than once.</p>
        {:else}
          <p class="secondary">
            {groups.length} {groups.length === 1 ? "song" : "songs"} with extra copies, {mb(wasted)} to win back. The best copy of each is kept: lossless first, then the higher quality.
            The same song on different albums (single, album, compilation) is left alone.
          </p>
        {/if}
      </div>
      {#if chosen.length}
        <button class="pill danger" onclick={() => (confirming = true)}><Icon name="trash" size={15} />Delete {chosen.length} {chosen.length === 1 ? "copy" : "copies"}…</button>
      {/if}
    </div>

    {#each (groups ?? []).slice(0, shown) as g (g.songs[0].id)}
      <div class="group">
        <div class="kind">{g.kind === "same" ? "The same song twice" : "A lossy copy of a lossless song"}</div>
        {#each g.songs as s (s.id)}
          {@const t = app.trackById.get(s.id)}
          {#if t}
            <div class="copy" class:keep={s.keep}>
              {#if s.keep}
                <span class="keeptag">Keep</span>
              {:else}
                <input type="checkbox" checked={!unchecked.includes(s.id)} onchange={() => toggle(s.id)} aria-label="Delete this copy of {t.title}" />
              {/if}
              <div class="who">
                <b>{t.title}</b>
                <span>{t.artist} · {t.album}</span>
                <span class="path">{where(t)}</span>
              </div>
              <span class="fmt">{formatOf(t)}</span>
              <span class="size">{mb(t.size)}</span>
              <div class="acts">{@render rowActions(t)}</div>
            </div>
          {/if}
        {/each}
      </div>
    {/each}
    {#if groups && groups.length > shown}
      <button class="more" onclick={() => (shown += 50)}>Show {Math.min(50, groups.length - shown)} more of {groups.length - shown}</button>
    {/if}
  </section>

  <!-- File quality -->
  <section class="card">
    <div class="head">
      <div>
        <h2>Lossless files that aren't</h2>
        <p class="secondary">
          Reads 20 seconds of each lossless file and looks at its spectrum. A song made from an MP3 or AAC stops dead at the encoder's limit (about 16 kHz);
          a "hi-res" file made from a CD has nothing above 22 kHz. It also spots 24-bit files that only hold 16-bit audio. It can't tell a high-bitrate MP3 (320 kbps)
          from a real recording, and some old recordings really do stop early, so treat the list as leads: play the song, or look at it in a spectrogram.
        </p>
        {#if report}
          <p class="secondary small">
            Checked {report.qualityChecked} of {report.lossless} lossless files{qualityLeft > 0 ? `, ${qualityLeft} to go` : ""}.
          </p>
        {/if}
      </div>
      {#if job?.kind === "quality"}
        <button class="pill ghost" onclick={() => api.healthStop()}>Stop ({job.done}/{job.total})</button>
      {:else}
        <button class="pill" disabled={!!job || qualityLeft === 0 && !!report} onclick={() => start("quality")}>
          <Icon name="wave" size={15} />{report && report.qualityChecked > 0 ? "Check new files" : "Check lossless files"}
        </button>
      {/if}
    </div>
    {#each showAllQuality ? qualityRows : qualityRows.slice(0, 40) as r (r.id)}
      {@const t = app.trackById.get(r.id)!}
      <div class="finding {r.verdict}">
        <span class="badge">{r.verdict === "lossy" ? "Lossy source" : r.verdict === "upsampled" ? "Upsampled" : r.verdict === "suspect" ? "Suspect" : "Padded"}</span>
        <div class="who">
          <b>{t.title}</b>
          <span>{t.artist} · {t.album} · {formatOf(t)}</span>
          <span class="why">{qualityText(r)}</span>
        </div>
        <div class="acts">{@render rowActions(t)}</div>
      </div>
    {/each}
    {#if !showAllQuality && qualityRows.length > 40}
      <button class="more" onclick={() => (showAllQuality = true)}>Show all {qualityRows.length}</button>
    {/if}
    {#if report && report.qualityChecked > 0 && qualityRows.length === 0 && !job}
      <p class="good"><Icon name="shield" size={15} /> Nothing suspicious in the files checked so far.</p>
    {/if}
  </section>

  <!-- Integrity -->
  <section class="card">
    <div class="head">
      <div>
        <h2>Damaged files</h2>
        <p class="secondary">
          Decodes every file from start to end and checks it against the checksum stored inside it (FLAC). Finds files that are cut off, corrupt, or changed since they were saved.
          Takes a while the first time; it runs quietly in the background.
        </p>
        {#if report}
          <p class="secondary small">
            Checked {report.integrityChecked} of {report.songs} files{integrityLeft > 0 ? `, ${integrityLeft} to go` : ""}.
          </p>
        {/if}
      </div>
      {#if job?.kind === "integrity"}
        <button class="pill ghost" onclick={() => api.healthStop()}>Stop ({job.done}/{job.total})</button>
      {:else}
        <button class="pill" disabled={!!job || integrityLeft === 0 && !!report} onclick={() => start("integrity")}>
          <Icon name="shield" size={15} />{report && report.integrityChecked > 0 ? "Check new files" : "Check every file"}
        </button>
      {/if}
    </div>
    {#each showAllIntegrity ? integrityRows : integrityRows.slice(0, 40) as r (r.id)}
      {@const t = app.trackById.get(r.id)!}
      <div class="finding bad">
        <span class="badge">Damaged</span>
        <div class="who">
          <b>{t.title}</b>
          <span>{t.artist} · {t.album} · {formatOf(t)}</span>
          <span class="why">{integrityText(r)}</span>
          <span class="path">{where(t)}</span>
        </div>
        <div class="acts">{@render rowActions(t)}</div>
      </div>
    {/each}
    {#if !showAllIntegrity && integrityRows.length > 40}
      <button class="more" onclick={() => (showAllIntegrity = true)}>Show all {integrityRows.length}</button>
    {/if}
    {#if report && report.integrityChecked > 0 && integrityRows.length === 0 && !job}
      <p class="good"><Icon name="shield" size={15} /> Every file checked so far is intact.</p>
    {/if}
  </section>
</div>

{#if confirming}
  <Modal title="Delete {chosen.length} {chosen.length === 1 ? 'file' : 'files'}?" onclose={() => !removing && (confirming = false)}>
    <p>
      This deletes {chosen.length} {chosen.length === 1 ? "copy" : "copies"} from your disk and frees {mb(chosenBytes)}. They don't go to the Recycle Bin.
      For each song, the copy marked <b>Keep</b> stays, and Cadence checks again that another copy remains before deleting anything.
    </p>
    <p class="secondary">Favorites and ratings move to the copy that stays.</p>
    {#snippet footer()}
      <button class="pill ghost" disabled={removing} onclick={() => (confirming = false)}>Cancel</button>
      <button class="pill danger" disabled={removing} onclick={removeChosen}>{removing ? "Deleting…" : "Delete"}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .lead {
    margin: 6px 0 22px;
    max-width: 70ch;
  }
  .card {
    margin-bottom: 22px;
    padding: 18px 18px 8px;
    border-radius: var(--r-md);
    background: var(--hover);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 20px;
    margin-bottom: 12px;
  }
  h2 {
    margin: 0 0 4px;
    font-family: var(--display);
    font-size: 18px;
    letter-spacing: -0.01em;
  }
  .head p {
    margin: 4px 0 0;
    font-size: 13px;
    max-width: 78ch;
  }
  .small {
    font-size: 12px;
  }
  .pill.danger {
    background: #c9485b;
    color: white;
  }
  .pill:disabled {
    opacity: 0.45;
  }
  .group {
    padding: 8px 0 10px;
    border-top: 1px solid var(--line);
  }
  .kind {
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0 4px 4px;
  }
  .copy,
  .finding {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 4px;
    border-radius: var(--r-sm);
  }
  .copy:hover,
  .finding:hover {
    background: var(--active);
  }
  .copy:not(.keep) .who {
    opacity: 0.85;
  }
  .keeptag {
    width: 40px;
    text-align: center;
    font-size: 11px;
    font-weight: 700;
    color: var(--accent);
  }
  input[type="checkbox"] {
    width: 40px;
    accent-color: var(--accent);
  }
  .who {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .who b {
    font-weight: 500;
  }
  .who span {
    font-size: 12.5px;
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .who .path {
    color: var(--text-3);
    font-size: 12px;
  }
  .who .why {
    color: var(--text);
    white-space: normal;
  }
  .fmt,
  .size {
    font-size: 12.5px;
    color: var(--text-2);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .size {
    width: 64px;
    text-align: right;
  }
  .acts {
    display: flex;
    gap: 2px;
    flex: none;
  }
  .finding {
    border-top: 1px solid var(--line);
    padding: 9px 4px;
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
  .finding.lossy .badge,
  .finding.bad .badge {
    background: rgba(201, 72, 91, 0.2);
    color: #ef7d8d;
  }
  .finding.suspect .badge {
    background: rgba(214, 160, 60, 0.2);
    color: #e3b45a;
  }
  .more {
    display: block;
    width: 100%;
    padding: 10px;
    color: var(--accent);
    font-weight: 600;
    font-size: 13px;
    border-top: 1px solid var(--line);
  }
  .good {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 4px 4px 14px;
    color: var(--text-2);
    font-size: 13px;
  }
  @media (max-width: 720px) {
    .fmt,
    .path {
      display: none;
    }
    .head {
      flex-direction: column;
    }
  }
</style>
