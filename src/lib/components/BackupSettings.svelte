<script lang="ts">
  // Settings › Backup: one file with your playlists, favorites, ratings, history and settings, and
  // the automatic snapshots Cadence keeps. Restoring asks first and says what it replaces.
  import { api, type BackupInfo, type BackupPreview, type BackupRestored, type BackupSnapshot } from "#lib/api.js";
  import { open, save } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  /** LyricsView keeps the timing nudges in this window's storage. */
  const OFFSETS = "cadence-lyric-offsets";
  const onAndroid = typeof navigator !== "undefined" && navigator.userAgent.includes("Android");

  let info = $state<BackupInfo | null>(null);
  let busy = $state<"" | "backup" | "check" | "restore" | "folder">("");
  let message = $state("");
  let error = $state("");
  /** The backup the listener chose, read and checked: the question before restoring. */
  let asking = $state<BackupPreview | null>(null);
  let result = $state<BackupRestored | null>(null);
  let showAll = $state(false);

  const offsets = (): string | null => {
    try {
      return localStorage.getItem(OFFSETS);
    } catch {
      return null;
    }
  };

  async function load() {
    try {
      info = await api.backupInfo();
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    load();
    // The timing nudges go with the data, so automatic snapshots have them too (once per visit).
    const o = offsets();
    if (o) api.backupKeepOffsets(o).catch(() => {});
  });

  const when = (secs: number) =>
    secs
      ? new Date(secs * 1000).toLocaleString(undefined, { day: "numeric", month: "short", year: "numeric", hour: "numeric", minute: "2-digit" })
      : "";
  const day = (secs: number) => (secs ? new Date(secs * 1000).toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" }) : "");
  const size = (b: number) => (b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(b / 1e3))} KB`);
  const kindLabel = (s: BackupSnapshot) => (s.kind === "auto" ? "Automatic" : s.kind === "safety" ? "Before a restore" : "Saved by you");
  const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
  function today() {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  }

  const shown = $derived((info?.snapshots ?? []).slice(0, showAll ? 40 : 5));

  async function backupNow() {
    error = message = "";
    let path: string | null = null;
    if (!onAndroid) {
      try {
        const picked = await save({ title: "Save a backup", defaultPath: `Cadence backup ${today()}.zip`, filters: [{ name: "Cadence backup", extensions: ["zip"] }] });
        if (!picked) return;
        path = picked;
      } catch (e) {
        error = String(e);
        return;
      }
    }
    busy = "backup";
    try {
      const m = await api.backupCreate(path, offsets());
      message = `Saved ${plural(m.files, "file")} (${size(m.bytes)}) to ${m.path}.${m.note ? ` ${m.note}` : ""}${m.skipped.length ? ` Left out because they're damaged: ${m.skipped.join(", ")}.` : ""}`;
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }

  async function chooseFile() {
    error = message = "";
    result = null;
    if (onAndroid) {
      // The system's file picker hands back an address Cadence can't read: backups in
      // Documents › Cadence are listed below instead.
      showAll = true;
      message = info?.exportDir ? `Put the backup file in ${info.exportDir}, then pick it in the list below.` : "Pick a backup in the list below.";
      return;
    }
    try {
      const picked = await open({ title: "Restore from a backup", multiple: false, filters: [{ name: "Cadence backup", extensions: ["zip"] }] });
      if (typeof picked === "string" && picked) await ask(picked);
    } catch (e) {
      error = String(e);
    }
  }

  /** Reads and checks a backup, then asks before restoring it. */
  async function ask(path: string) {
    error = message = "";
    result = null;
    busy = "check";
    try {
      asking = await api.backupPreview(path);
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }

  async function restoreNow() {
    const p = asking;
    if (!p) return;
    asking = null;
    busy = "restore";
    try {
      const r = await api.backupRestore(p.path);
      result = r;
      if (r.lyricOffsets != null) {
        try {
          localStorage.setItem(OFFSETS, r.lyricOffsets);
        } catch {
          // A full or unavailable storage: the nudges stay in the data folder.
        }
      }
      // What the window keeps of its own (the rest reloads on the backend's events).
      api.skips().then((s) => {
        if (!s) return;
        app.skipped = s.ids;
        app.skipInstrumentals = s.instrumentals;
      }).catch(() => {});
      api.folders().then((f) => f && (app.folders = f)).catch(() => {});
      api.artistImages().then((m) => m && (app.artistImages = m)).catch(() => {});
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }

  async function chooseFolder() {
    error = message = "";
    let picked: string | null = null;
    try {
      if (onAndroid) picked = await new Promise<string | null>((resolve) => (app.folderBrowser = resolve));
      else {
        const f = await open({ directory: true, multiple: false, title: "A second place for backups" });
        picked = typeof f === "string" ? f : null;
      }
    } catch (e) {
      error = String(e);
      return;
    }
    if (picked) await setFolder(picked);
  }

  async function setFolder(folder: string | null) {
    busy = "folder";
    try {
      info = await api.backupSetFolder(folder);
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }
</script>

<div class="title-row">
  <div>
    <h2 class="section-title">Backup</h2>
    <p class="desc">
      One file with your playlists, favorites and ratings, listening history, queue, settings, artist pictures and lyrics you
      picked. Your music, keys and passwords aren't in it. Cadence also keeps an automatic snapshot of this each day it opens
      (the last 7 days and 4 weeks), on this device.
    </p>
    <p class="hint">
      {#if info?.lastAuto}Last automatic copy: {when(info.lastAuto)}.{:else if info}No automatic copy yet: one is made the next time Cadence opens.{/if}
    </p>
  </div>
  <div class="actions">
    <button class="pill" onclick={backupNow} disabled={!!busy}><Icon name="download" size={15} />{busy === "backup" ? "Backing up…" : "Back up now"}</button>
    <button class="pill ghost" onclick={chooseFile} disabled={!!busy}>{busy === "check" ? "Checking…" : busy === "restore" ? "Restoring…" : "Restore…"}</button>
  </div>
</div>

{#if message}<p class="note" role="status">{message}</p>{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if result}
  <div class="result" role="status">
    <strong>Restored the backup from {day(result.created)}.</strong>
    <span>
      {plural(result.files, "file")} put back.
      {#if result.relinked}{plural(result.relinked, "song")} found again where your music is now.{/if}
      {#if result.unresolved}{plural(result.unresolved, "song")} of your playlists, favorites or queue aren't in your library now: they're kept and come back if the files turn up.{/if}
      {#if result.unresolvedHistory}{plural(result.unresolvedHistory, "song")} only in your history aren't in your library.{/if}
      Your data from just before is in the list below (“Before a restore”).
    </span>
  </div>
{/if}

<div class="second">
  <div>
    <span class="label">Also copy to</span>
    {#if info?.folder}
      <span class="path" title={info.folder}>{info.folder}</span>
    {:else}
      <span class="secondary small">Off. Choose a folder on another drive, or one your cloud storage syncs.</span>
    {/if}
    {#if info?.folderError}<p class="error">{info.folderError}</p>{/if}
  </div>
  <div class="actions">
    <button class="link small" onclick={chooseFolder} disabled={!!busy}>{info?.folder ? "Change" : "Choose folder…"}</button>
    {#if info?.folder}<button class="link small" onclick={() => setFolder(null)} disabled={!!busy}>Turn off</button>{/if}
  </div>
</div>

{#if shown.length}
  <ul class="snapshots" aria-label="Backups on this device">
    {#each shown as s (s.path)}
      <li>
        <Icon name="file" size={17} />
        <span class="when">{when(s.created)}</span>
        <span class="secondary small">{kindLabel(s)} · {size(s.size)}</span>
        <button class="link small" onclick={() => ask(s.path)} disabled={!!busy} aria-label={`Restore the backup from ${when(s.created)}`}>Restore</button>
      </li>
    {/each}
  </ul>
  {#if (info?.snapshots.length ?? 0) > 5}
    <button class="link small more" onclick={() => (showAll = !showAll)}>{showAll ? "Show fewer" : `Show all ${info?.snapshots.length}`}</button>
  {/if}
{/if}

{#if asking}
  {@const p = asking}
  <Modal title="Restore this backup?" onclose={() => (asking = null)}>
    <p class="ask">
      The backup from <b>{when(p.created)}</b>{p.app ? ` (Cadence ${p.app})` : ""} replaces:
    </p>
    <ul class="replaces">
      {#each p.replaces as line (line)}<li>{line}</li>{:else}<li>Nothing: the backup is empty.</li>{/each}
    </ul>
    <p class="hint">Your data as it is now is copied first, so you can go back to it. Your music files aren't touched.</p>
    {#snippet footer()}
      <button class="pill ghost" onclick={() => (asking = null)}>Cancel</button>
      <button class="pill" onclick={restoreNow}>Restore</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .title-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 24px;
  }
  .desc {
    margin: 6px 0 0;
    color: var(--text-2);
    max-width: 64ch;
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
  }
  .note {
    margin: 12px 0 0;
    font-size: 13px;
    color: var(--text-2);
    user-select: text;
  }
  .error {
    margin: 8px 0 0;
    font-size: 13px;
    color: var(--warn);
  }
  .result {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 14px;
    padding: 14px 16px;
    border-radius: var(--r-md);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    font-size: 13px;
    color: var(--text-2);
  }
  .result strong {
    color: var(--text);
  }
  .second {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-top: 18px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--hover);
  }
  .second > div:first-child {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 12px;
    min-width: 0;
  }
  .label {
    color: var(--text-2);
  }
  .path {
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
  .small {
    font-size: 13px;
  }
  .snapshots {
    list-style: none;
    margin: 14px 0 0;
    padding: 0;
  }
  .snapshots li {
    display: grid;
    grid-template-columns: 20px minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 12px;
    padding: 7px 6px 7px 12px;
    border-radius: var(--r-sm);
    color: var(--text-3);
  }
  .snapshots li:hover {
    background: var(--hover);
  }
  .when {
    color: var(--text);
  }
  .more {
    margin: 6px 0 0 12px;
  }
  .link:disabled {
    color: var(--text-3);
    text-decoration: none;
  }
  .ask {
    margin: 0;
  }
  .replaces {
    margin: 10px 0 0;
    padding-left: 20px;
    color: var(--text-2);
    line-height: 1.6;
  }
  @media (max-width: 720px) {
    .title-row {
      flex-direction: column;
      gap: 12px;
    }
  }
</style>
