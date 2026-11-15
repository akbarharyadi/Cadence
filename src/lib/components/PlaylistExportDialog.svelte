<script lang="ts">
  // "Export…" for one playlist, or "Export all playlists…" (one file each, into a folder).
  // M3U8 for other players and devices, CSV for a spreadsheet.
  import { open, save } from "@tauri-apps/plugin-dialog";
  import { api } from "#lib/api.js";
  import { keepExportOptions, onAndroid, savedExportOptions, transfer, type ExportTarget } from "#lib/playlistTransfer.svelte.js";
  import { app } from "#lib/store.svelte.js";
  import Modal from "./Modal.svelte";
  import Segmented from "./Segmented.svelte";

  let { target }: { target: ExportTarget } = $props();

  let opts = $state(savedExportOptions());
  let busy = $state(false);
  let error = $state("");

  const all = $derived(target.kind === "all");
  const ids = $derived(target.kind === "one" ? target.ids : target.items.flatMap((i) => i.ids));
  /** Songs that are tracks of a single-file album: one file path for several songs. */
  const cue = $derived(ids.some((id) => app.trackById.get(id)?.cue));
  const missing = $derived(ids.filter((id) => !app.trackById.has(id)).length);
  const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
  const fileName = (name: string) => name.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().replace(/[. ]+$/, "") || "Playlist";

  async function run() {
    error = "";
    busy = true;
    const options = { format: opts.format, relative: opts.relative, forwardSlashes: opts.forwardSlashes, crlf: opts.crlf };
    keepExportOptions(options);
    try {
      if (target.kind === "one") {
        let path: string | null = null;
        if (!onAndroid) {
          const picked = await save({
            title: "Export playlist",
            defaultPath: `${fileName(target.name)}.${options.format}`,
            filters: [options.format === "csv" ? { name: "CSV", extensions: ["csv"] } : { name: "M3U8 playlist", extensions: ["m3u8"] }],
          });
          if (!picked) return;
          path = picked;
        }
        const r = await api.playlistExport(target.name, target.ids, path, options);
        app.showToast(
          `Saved “${target.name}” (${plural(r.written, "song")}) to ${r.path}${r.skipped ? `; ${plural(r.skipped, "song")} not in your library left out` : ""}`,
          null,
          8000,
        );
      } else {
        let folder: string | null = null;
        if (!onAndroid) {
          const picked = await open({ directory: true, multiple: false, title: "Export playlists to a folder" });
          if (typeof picked !== "string" || !picked) return;
          folder = picked;
        }
        const r = await api.playlistExportAll(target.items, folder, options);
        app.showToast(`Saved ${plural(r.files, "playlist")} (${plural(r.written, "song")}) to ${r.folder}`, null, 8000);
      }
      transfer.exporting = null;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={all ? "Export all playlists" : "Export playlist"} onclose={() => (transfer.exporting = null)} width="480px">
  <p class="what">
    {#if target.kind === "one"}
      “{target.name}”: {plural(target.ids.length, "song")}.
    {:else}
      {plural(target.items.length, "playlist")}, one file each. Smart playlists are saved with the songs they have now.
    {/if}
  </p>

  <div class="row">
    <span class="label">Format</span>
    <Segmented
      label="Format"
      value={opts.format}
      onchange={(v) => (opts.format = v)}
      options={[
        { value: "m3u8", label: "M3U8" },
        { value: "csv", label: "CSV" },
      ]}
    />
  </div>
  <p class="hint">{opts.format === "csv" ? "A spreadsheet: title, artist, album, album artist, length, track number, year and path." : "Works with most players and devices."}</p>

  <div class="row">
    <span class="label">Paths</span>
    <Segmented
      label="Paths"
      value={opts.relative ? "relative" : "full"}
      onchange={(v) => (opts.relative = v === "relative")}
      options={[
        { value: "full", label: "Full paths" },
        { value: "relative", label: "Relative to the file" },
      ]}
    />
  </div>
  {#if opts.relative}
    <p class="hint">For copying your music and playlists to a phone or another computer: keep the file where its folder still leads to the music. Songs on another drive keep their full path.</p>
  {/if}

  <label class="check">
    <input type="checkbox" checked={opts.forwardSlashes} onchange={(e) => (opts.forwardSlashes = e.currentTarget.checked)} />
    <span>Use forward slashes (/) in paths</span>
  </label>
  {#if opts.format === "m3u8"}
    <label class="check">
      <input type="checkbox" checked={opts.crlf} onchange={(e) => (opts.crlf = e.currentTarget.checked)} />
      <span>Windows line endings (CRLF) <span class="secondary">for devices that need them</span></span>
    </label>
  {/if}

  {#if cue}
    <p class="hint">Some songs are tracks of one audio file (a CUE sheet): each is written with that file’s path and its own title and length.</p>
  {/if}
  {#if missing}
    <p class="hint">{plural(missing, "song")} {missing === 1 ? "isn’t" : "aren’t"} in your library right now and will be left out.</p>
  {/if}
  {#if all}
    <p class="hint">Files with the same names in the folder you choose are replaced.</p>
  {/if}
  {#if onAndroid}
    <p class="hint">Saved in Documents › Cadence on this phone.</p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#snippet footer()}
    <button class="pill ghost" onclick={() => (transfer.exporting = null)}>Cancel</button>
    <button class="pill" onclick={run} disabled={busy || (all && target.kind === "all" && !target.items.length)}>
      {busy ? "Saving…" : all && !onAndroid ? "Choose folder…" : "Export…"}
    </button>
  {/snippet}
</Modal>

<style>
  .what {
    margin: 0 0 14px;
    color: var(--text-2);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-top: 12px;
  }
  .label {
    width: 64px;
    color: var(--text-2);
    font-size: 13px;
  }
  .hint {
    margin: 6px 0 0 78px;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    margin: 12px 0 0 78px;
    font-size: 13.5px;
  }
  .error {
    margin: 14px 0 0;
    font-size: 13px;
    color: var(--warn);
  }
  @media (max-width: 720px) {
    .row {
      flex-direction: column;
      align-items: flex-start;
      gap: 6px;
    }
    .hint,
    .check {
      margin-left: 0;
    }
  }
</style>
