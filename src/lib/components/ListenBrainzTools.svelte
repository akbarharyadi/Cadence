<script lang="ts">
  // Settings › Scrobbling, once ListenBrainz is connected: send favorites as loved songs, restore
  // the listening history from ListenBrainz, and sync favorites both ways. Each of the two actions
  // shows what it found and asks before it changes anything.
  import { listen } from "@tauri-apps/api/event";
  import { save } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { api, type LbApplied, type LbImportStatus, type LbSyncDone, type LbSyncPreview } from "#lib/api.js";
  import Modal from "./Modal.svelte";
  import Toggle from "./Toggle.svelte";

  const onAndroid = typeof navigator !== "undefined" && navigator.userAgent.includes("Android");

  let loved = $state(true);
  let waiting = $state(0);
  let importing = $state(false);
  let status = $state<LbImportStatus | null>(null);
  let syncing = $state(false);
  let preview = $state<LbSyncPreview | null>(null);
  let syncDone = $state<LbSyncDone | null>(null);
  let syncBusy = $state(false);
  let error = $state("");
  let message = $state("");

  const n = (v: number | undefined | null) => (v ?? 0).toLocaleString();
  const plural = (v: number, one: string, many = `${one}s`) => `${n(v)} ${v === 1 ? one : many}`;
  /** " (3 fit more than one song)", or nothing. */
  const fits = (v: number) => (v > 0 ? ` (${n(v)} fit more than one song)` : "");

  onMount(() => {
    api.appSettings().then((s) => (loved = s.listenbrainzLoved ?? true)).catch(() => {});
    api.lbLovedWaiting().then((w) => (waiting = w ?? 0)).catch(() => {});
    api.lbImportStatus().then((s) => (status = s ?? null)).catch(() => {});
    let off: (() => void) | undefined;
    let gone = false;
    listen<LbImportStatus>("listenbrainz-import-progress", (e) => {
      if (e.payload) status = e.payload;
    })
      .then((u) => (gone ? u() : (off = u)))
      .catch(() => {});
    return () => {
      gone = true;
      off?.();
    };
  });

  // ---------- restoring the history ----------
  async function run<T>(f: () => Promise<T>): Promise<T | undefined> {
    error = "";
    try {
      return await f();
    } catch (e) {
      error = String(e);
      return undefined;
    }
  }

  async function start() {
    const s = await run(() => api.lbImportStart());
    if (s) status = s;
  }
  const pause = () => run(() => api.lbImportPause());
  async function discard() {
    await run(() => api.lbImportDiscard());
    const s = await run(() => api.lbImportStatus());
    if (s) status = s;
  }
  async function apply() {
    const a = await run(() => api.lbImportApply());
    if (a) {
      const s = await run(() => api.lbImportStatus());
      if (s) status = s;
    }
  }
  async function saveUnmatched() {
    message = "";
    let path: string | null = null;
    if (!onAndroid) {
      const picked = await save({
        title: "Save the listens that weren’t matched",
        defaultPath: "ListenBrainz listens not in library.csv",
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!picked) return;
      path = picked;
    }
    const r = await run(() => api.lbImportSaveUnmatched(path));
    if (r) message = `Saved ${plural(r.rows, "listen")} to ${r.path}.`;
  }
  /** The import's state as the template reads it. */
  const s = $derived(status);
  const applied = $derived<LbApplied | null>(status?.applied ?? null);

  // ---------- syncing favorites ----------
  async function openSync() {
    syncing = true;
    preview = null;
    syncDone = null;
    error = "";
    syncBusy = true;
    const p = await run(() => api.lbLovedPreview());
    syncBusy = false;
    if (p) preview = p;
  }
  async function applySync() {
    syncBusy = true;
    const d = await run(() => api.lbLovedApply());
    syncBusy = false;
    if (d) {
      syncDone = d;
      preview = null;
      api.lbLovedWaiting().then((w) => (waiting = w ?? 0)).catch(() => {});
    }
  }
  const nothingToSync = $derived(!!preview && preview.toSend === 0 && preview.toHeart === 0);
</script>

<div class="tools">
  <div class="row">
    <div>
      <strong>Loved songs</strong>
      <p class="hint">Hearting a song marks it as loved on ListenBrainz too (and un-hearting takes it back). It’s sent in the background when the song’s recording can be identified.</p>
    </div>
    <Toggle checked={loved} label="Send favorites to ListenBrainz as loved songs" onchange={(v) => { loved = v; void api.setListenbrainzLoved(v); }} />
  </div>
  {#if waiting > 0}<p class="hint">{plural(waiting, "loved song")} waiting to be sent.</p>{/if}
  <div class="buttons">
    <button class="pill ghost" onclick={() => (importing = true)}>Restore history from ListenBrainz…</button>
    <button class="pill ghost" onclick={openSync}>Sync favorites…</button>
  </div>
  {#if error && !importing && !syncing}<p class="error">{error}</p>{/if}
</div>

{#if importing}
  <Modal title="Restore history from ListenBrainz" onclose={() => (importing = false)} width="560px">
    {#if !s || s.phase === "idle"}
      <p>Cadence reads your listens from ListenBrainz, newest first, in the background (one request a second), and matches each one to a song in your library. Then it shows you what it would add. Nothing changes until you say so.</p>
    {:else if s.phase === "scanning"}
      <p class="busy">Reading your listens… {n(s.seen)} so far, {n(s.matched)} matched to your songs.</p>
      <p class="hint">You can close this: it carries on in the background (see the sidebar).</p>
    {:else if s.phase === "paused"}
      <p>Paused after {n(s.seen)} listens. It carries on from there.</p>
    {:else if s.phase === "failed"}
      <p class="error">{s.message || "It stopped."}</p>
      <p class="hint">{n(s.seen)} listens were read before that. It can carry on from there.</p>
    {:else if s.phase === "ready" && s.summary}
      <p class="summary">
        {n(s.summary.onListenbrainz)} listens on ListenBrainz, {n(s.summary.matched)} match songs in your library, {n(s.summary.alreadyHere)} already in your history, will add {n(s.summary.willAdd)}.
      </p>
      {#if s.summary.unmatched > 0}
        <p class="hint">
          {plural(s.summary.unmatched, "listen")} didn’t match a song for sure and {s.summary.unmatched === 1 ? "is" : "are"} left out{fits(s.summary.ambiguous)}. Nothing is guessed.
          <button class="link" onclick={saveUnmatched}>Save the list</button>
        </p>
      {/if}
      <p class="hint">A copy of your data is saved first, so you can go back to it (Settings › Backup).</p>
    {:else if s.phase === "applying"}
      <p class="busy">Adding the plays…</p>
    {:else if s.phase === "done" && applied}
      <p class="summary">Added {plural(applied.added, "play")}: {n(applied.live)} to your recent history and {n(applied.archived)} to its archive.</p>
      {#if applied.safety}<p class="hint">Your data as it was is in {applied.safety}.</p>{/if}
    {/if}
    {#if message}<p class="hint">{message}</p>{/if}
    {#if error}<p class="error">{error}</p>{/if}
    {#snippet footer()}
      {#if !s || s.phase === "idle"}
        <button class="pill ghost" onclick={() => (importing = false)}>Cancel</button>
        <button class="pill" onclick={start}>Start reading</button>
      {:else if s.phase === "scanning"}
        <button class="pill ghost" onclick={pause}>Pause</button>
        <button class="pill" onclick={() => (importing = false)}>Close</button>
      {:else if s.phase === "paused" || s.phase === "failed"}
        <button class="pill ghost" onclick={discard}>Discard</button>
        <button class="pill" onclick={start}>{s.seen > 0 ? "Resume" : "Start again"}</button>
      {:else if s.phase === "ready" && s.summary}
        <button class="pill ghost" onclick={discard}>Discard</button>
        <button class="pill" onclick={apply} disabled={s.summary.willAdd === 0}>Add {plural(s.summary.willAdd, "play")}</button>
      {:else}
        <button class="pill" onclick={() => (importing = false)}>Close</button>
      {/if}
    {/snippet}
  </Modal>
{/if}

{#if syncing}
  <Modal title="Sync favorites" onclose={() => (syncing = false)} width="520px">
    {#if syncBusy && !preview}
      <p class="busy">Checking your loved songs on ListenBrainz…</p>
    {:else if syncDone}
      <p class="summary">Hearted {plural(syncDone.hearted, "song")} here. {plural(syncDone.queued, "favorite")} will be sent to ListenBrainz in the background.</p>
    {:else if preview}
      <p class="summary">{plural(preview.favorites, "favorite")} here, {plural(preview.loved, "loved song")} on ListenBrainz, {n(preview.inBoth)} in both.</p>
      <ul>
        <li>Send {plural(preview.toSend, "favorite")} to ListenBrainz as loved.</li>
        <li>Heart {plural(preview.toHeart, "song")} here that you love there.</li>
      </ul>
      {#if preview.notFound > 0}
        <p class="hint">
          {plural(preview.notFound, "loved song")} {preview.notFound === 1 ? "isn’t" : "aren’t"} in your library for sure{fits(preview.ambiguous)}: left out.
        </p>
      {/if}
      <p class="hint">Nothing is ever un-hearted here.</p>
    {/if}
    {#if error}<p class="error">{error}</p>{/if}
    {#snippet footer()}
      {#if syncDone || (!preview && !syncBusy)}
        <button class="pill" onclick={() => (syncing = false)}>Close</button>
      {:else}
        <button class="pill ghost" onclick={() => (syncing = false)}>Cancel</button>
        <button class="pill" onclick={applySync} disabled={!preview || syncBusy || nothingToSync}>{nothingToSync ? "Nothing to do" : "Apply"}</button>
      {/if}
    {/snippet}
  </Modal>
{/if}

<style>
  .tools {
    margin-top: 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 24px;
  }
  .row p {
    margin: 4px 0 0;
    max-width: 64ch;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 4px;
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .error {
    margin: 8px 0 0;
    font-size: 13px;
    color: var(--warn);
  }
  .summary {
    font-weight: 600;
  }
  .busy {
    color: var(--text-2);
  }
  ul {
    margin: 8px 0 0;
    padding-left: 20px;
    color: var(--text-2);
  }
  .link {
    color: var(--accent);
    text-decoration: underline;
    padding: 0;
    margin-left: 6px;
  }
</style>
