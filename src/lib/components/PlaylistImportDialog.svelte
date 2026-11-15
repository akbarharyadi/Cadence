<script lang="ts">
  // The result of reading a playlist file, before anything is saved: what was found in the library,
  // what wasn't (and why), and a choice to create the playlist, skip it, or save the missing list.
  import { save } from "@tauri-apps/plugin-dialog";
  import { api, formatDuration, type PlaylistImport } from "#lib/api.js";
  import { onAndroid, transfer } from "#lib/playlistTransfer.svelte.js";
  import { app } from "#lib/store.svelte.js";
  import Modal from "./Modal.svelte";
  import Segmented from "./Segmented.svelte";

  let { item, remaining = 0 }: { item: PlaylistImport; remaining?: number } = $props();

  const SHOWN = 200;
  // `name` starts as the file's own; it's the only part of `item` that is edited here.
  let name = $state((() => item.name || "Imported playlist")());
  let choice = $state<"new" | "replace">("new");
  let busy = $state(false);
  let error = $state("");
  let message = $state("");

  const matched = $derived(item.matched ?? []);
  const misses = $derived(item.misses ?? []);
  const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
  const fileName = (s: string) => s.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().replace(/[. ]+$/, "") || "Playlist";

  const same = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();
  const existing = $derived(name.trim() ? app.playlists.find((p) => same(p.name, name)) : undefined);
  /** A smart playlist's songs are its rules', so only a hand-made playlist can be replaced. */
  const canReplace = $derived(!!existing && !existing.smart);
  const replacing = $derived(canReplace && choice === "replace");
  /** "Name 2", "Name 3"…: the first not used. */
  const numbered = $derived.by(() => {
    const base = name.trim();
    let n = 2;
    while (app.playlists.some((p) => same(p.name, `${base} ${n}`))) n++;
    return `${base} ${n}`;
  });
  const finalName = $derived(!existing ? name.trim() : replacing ? existing.name : numbered);
  /** Replacing would leave the playlist with far fewer songs than it has (300 → 1): asked first. */
  const shrinks = $derived(replacing && !!existing && existing.trackIds.length >= 4 && matched.length < existing.trackIds.length / 2);
  /** The question above was asked for this choice (another name or choice asks again). */
  let confirmed = $state("");
  const confirmKey = $derived(`${existing?.id ?? ""}|${choice}|${matched.length}`);
  const asking = $derived(shrinks && confirmed === `asked:${confirmKey}`);

  function next() {
    transfer.imports = transfer.imports.slice(1);
  }

  async function create() {
    if (busy || !matched.length || !name.trim()) return;
    if (shrinks && confirmed !== `yes:${confirmKey}`) {
      // First: say how many songs it would have, and how many it has now.
      if (asking) confirmed = `yes:${confirmKey}`;
      else {
        confirmed = `asked:${confirmKey}`;
        return;
      }
    }
    error = "";
    busy = true;
    try {
      const ids = [...matched];
      const before = replacing && existing ? $state.snapshot(existing) : null;
      const base = before ?? { id: "", name: finalName, trackIds: [], smart: null, created: 0, updated: 0 };
      const saved = await app.savePlaylist({ ...base, trackIds: ids });
      const said = `${before ? "Replaced the songs of" : "Created"} “${saved.name}” with ${plural(ids.length, "song")}`;
      if (before) {
        // Undo: the playlist's songs as they were (saved again).
        app.showToast(said, {
          label: "Undo",
          run: () => {
            app.toast = app.toastAction = null;
            app
              .savePlaylist({ ...saved, trackIds: before.trackIds })
              .then(() => app.showToast(`“${saved.name}” has its ${plural(before.trackIds.length, "song")} back`))
              .catch((e) => app.showToast(`Couldn’t put the songs back: ${e}`));
          },
        });
      } else {
        app.showToast(said);
      }
      if (remaining === 0) app.go({ name: "playlist", id: saved.id });
      next();
    } catch (e) {
      error = `Couldn’t save the playlist: ${e}`;
    } finally {
      busy = false;
    }
  }

  async function saveMissing() {
    error = message = "";
    try {
      let path: string | null = null;
      if (!onAndroid) {
        const picked = await save({
          title: "Save the missing list",
          defaultPath: `${fileName(name)} - not in library.csv`,
          filters: [{ name: "CSV", extensions: ["csv"] }],
        });
        if (!picked) return;
        path = picked;
      }
      const r = await api.playlistMissingSave(item.source, path);
      message = `Saved ${plural(r.rows, "entry", "entries")} to ${r.path}.`;
    } catch (e) {
      error = String(e);
    }
  }

  const label = (m: { artist: string; title: string }) => [m.artist, m.title].filter(Boolean).join(" – ") || "(no title)";
</script>

<Modal title="Import playlist" onclose={() => (transfer.imports = [])} width="560px">
  {#if item.error}
    <p class="file">{item.source}</p>
    <p class="error" role="alert">{item.error}</p>
  {:else}
    <label class="field">
      <span class="label">Name</span>
      <input bind:value={name} spellcheck="false" aria-label="Playlist name" />
    </label>
    <p class="file">{item.format} file with {plural(item.total, "entry", "entries")}: {item.source}</p>

    <div class="counts" role="status">
      <span class="ok"><b>{matched.length}</b> matched</span>
      <span><b>{item.notFound}</b> not found</span>
      <span><b>{item.ambiguous}</b> ambiguous</span>
    </div>

    {#if existing}
      <div class="clash">
        <p>You already have a playlist called “{existing.name}”.</p>
        {#if canReplace}
          <Segmented
            label="When the name is taken"
            value={choice}
            onchange={(v) => {
              choice = v;
              confirmed = "";
            }}
            options={[
              { value: "new", label: `Create “${numbered}”` },
              { value: "replace", label: "Replace its songs" },
            ]}
          />
        {:else}
          <p class="hint">It’s a smart playlist, so this one is created as “{numbered}”.</p>
        {/if}
      </div>
    {/if}

    {#if !matched.length}
      <p class="hint">None of its entries are in your library, so there is nothing to create.</p>
    {/if}

    {#if asking && existing}
      <p class="error" role="alert">
        Replacing leaves “{existing.name}” with {plural(matched.length, "song")}; it has {plural(existing.trackIds.length, "song")} now.
        Replace anyway? (You can undo it right after.)
      </p>
    {/if}

    {#if misses.length}
      <p class="sub">Not added</p>
      <ul class="misses" aria-label="Entries not added">
        {#each misses.slice(0, SHOWN) as m, i (i)}
          <li>
            <span class="name">{label(m)}{m.duration ? ` (${formatDuration(m.duration)})` : ""}</span>
            <span class="why" class:amb={m.ambiguous}>{m.ambiguous ? "Ambiguous: " : ""}{m.reason}</span>
          </li>
        {/each}
      </ul>
      {#if misses.length > SHOWN || item.moreMisses > 0}
        <p class="hint">and {misses.length - Math.min(SHOWN, misses.length) + (item.moreMisses ?? 0)} more. They are all in the missing list.</p>
      {/if}
    {/if}
    {#if message}<p class="note" role="status">{message}</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
  {/if}

  {#snippet footer()}
    {#if !item.error && misses.length}
      <button class="pill ghost left" onclick={saveMissing} disabled={busy}>Save the missing list…</button>
    {/if}
    {#if remaining > 0}
      <button class="pill ghost" onclick={next} disabled={busy}>{item.error ? "Next file" : "Skip this file"}</button>
    {/if}
    <button class="pill ghost" onclick={() => (transfer.imports = [])}>{item.error && remaining === 0 ? "Close" : "Cancel"}</button>
    {#if !item.error}
      <button class="pill" onclick={create} disabled={busy || !matched.length || !name.trim()}>{asking ? "Replace anyway" : "Create playlist"}</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .field {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .label {
    color: var(--text-2);
    font-size: 13px;
  }
  .field input {
    flex: 1;
    min-width: 0;
    background: var(--hover);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
    padding: 6px 10px;
    color: var(--text);
  }
  .field input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .file {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--text-3);
    overflow-wrap: anywhere;
    user-select: text;
  }
  .counts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    margin-top: 14px;
    font-size: 14px;
    color: var(--text-2);
  }
  .counts b {
    color: var(--text);
  }
  .ok b {
    color: var(--accent);
  }
  .clash {
    margin-top: 14px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--hover);
    font-size: 13px;
  }
  .clash p {
    margin: 0 0 8px;
  }
  .clash p:last-child {
    margin: 0;
  }
  .sub {
    margin: 16px 0 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-2);
  }
  .misses {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 240px;
    overflow-y: auto;
    border-radius: var(--r-sm);
    background: var(--hover);
  }
  .misses li {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 6px 10px;
    font-size: 13px;
  }
  .name {
    overflow-wrap: anywhere;
    user-select: text;
  }
  .why {
    font-size: 12px;
    color: var(--text-3);
  }
  .why.amb {
    color: var(--warn);
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .note {
    margin: 12px 0 0;
    font-size: 13px;
    color: var(--text-2);
    user-select: text;
  }
  .error {
    margin: 12px 0 0;
    font-size: 13px;
    color: var(--warn);
  }
  .left {
    margin-right: auto;
  }
</style>
