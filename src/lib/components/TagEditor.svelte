<script lang="ts">
  import { api, type Track } from "$lib/api";
  import { app } from "$lib/store.svelte";
  import Modal from "./Modal.svelte";

  const tracks = $derived(app.tagEditing ?? []);
  const single = $derived(tracks.length === 1);

  type Key = "title" | "artist" | "album" | "albumArtist" | "genre" | "year" | "trackNo" | "discNo";
  const fields: { key: Key; label: string; onlySingle?: boolean; numeric?: boolean }[] = [
    { key: "title", label: "Title", onlySingle: true },
    { key: "artist", label: "Artist" },
    { key: "album", label: "Album" },
    { key: "albumArtist", label: "Album artist" },
    { key: "genre", label: "Genre" },
    { key: "year", label: "Year", numeric: true },
    { key: "trackNo", label: "Track number", onlySingle: true, numeric: true },
    { key: "discNo", label: "Disc number", numeric: true },
  ];

  const valueOf = (t: Track, k: Key) => String(t[k] ?? "");
  /** The shared value, or null when the songs differ. */
  function common(k: Key): string | null {
    const values = new Set(tracks.map((t) => valueOf(t, k)));
    return values.size === 1 ? [...values][0] : null;
  }

  // Start from the current values; only fields the user touches are written.
  let values = $state<Record<string, string>>({});
  let touched = $state<Record<string, boolean>>({});
  let saving = $state(false);
  let error = $state("");
  $effect(() => {
    const v: Record<string, string> = {};
    for (const f of fields) v[f.key] = common(f.key) ?? "";
    values = v;
    touched = {};
  });

  async function save() {
    const changes: Record<string, string> = {};
    for (const f of fields) if (touched[f.key]) changes[f.key] = values[f.key];
    if (!Object.keys(changes).length) return close();
    saving = true;
    error = "";
    try {
      const n = await api.writeTags(tracks.map((t) => t.id), changes);
      app.showToast(n === 1 ? "Saved song info" : `Saved info for ${n} songs`);
      close();
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  const close = () => (app.tagEditing = null);
</script>

<Modal title={single ? "Song info" : `Info for ${tracks.length} songs`} onclose={close} width="560px">
  <form class="grid" onsubmit={(e) => { e.preventDefault(); save(); }}>
    {#each fields.filter((f) => single || !f.onlySingle) as f (f.key)}
      <label for="tag-{f.key}">{f.label}</label>
      <input
        id="tag-{f.key}"
        class="field"
        class:changed={touched[f.key]}
        inputmode={f.numeric ? "numeric" : "text"}
        bind:value={values[f.key]}
        oninput={() => (touched[f.key] = true)}
        placeholder={common(f.key) === null ? "Several values" : ""}
      />
    {/each}
    <button type="submit" hidden aria-hidden="true"></button>
  </form>
  {#if single}<p class="path" title={tracks[0].path}>{tracks[0].path}</p>{/if}
  {#if error}<p class="error">{error}</p>{/if}

  {#snippet footer()}
    <span class="note">Changes are written into the music {single ? "file" : "files"}.</span>
    <button class="pill ghost" onclick={close}>Cancel</button>
    <button class="pill" onclick={save} disabled={saving}>{saving ? "Saving…" : "Save"}</button>
  {/snippet}
</Modal>

<style>
  .grid {
    display: grid;
    grid-template-columns: 120px 1fr;
    align-items: center;
    gap: 10px 14px;
  }
  label {
    color: var(--text-2);
    font-size: 13px;
  }
  .field {
    font-size: 14px;
    padding: 8px 11px;
  }
  .field.changed {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .field::placeholder {
    color: var(--text-3);
    font-style: italic;
  }
  .path {
    margin: 16px 0 0;
    font-size: 12px;
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    user-select: text;
  }
  .error {
    margin: 12px 0 0;
    color: var(--warn);
    font-size: 13px;
  }
  .note {
    margin-right: auto;
    align-self: center;
    font-size: 12.5px;
    color: var(--text-3);
  }
</style>
