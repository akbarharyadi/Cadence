<script lang="ts">
  // Ctrl+K: one box for pages, actions and your music. A Modal, so it joins the dialogs'
  // Escape stack (Escape and Android Back close it first, and the window's shortcuts stand aside).
  import { onDestroy, onMount, tick } from "svelte";
  import { app } from "#lib/store.svelte.js";
  import { actionItems, pageItems, search, type PaletteItem } from "#lib/paletteItems.js";
  import { overlays } from "#lib/palette.svelte.js";
  import { warmSearch } from "#lib/searchWarm.js";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  // The pages and actions are listed once, when the palette opens (not on every player update).
  const pages = pageItems();
  const actions = actionItems();

  let query = $state("");
  /** What's searched for: the typed text ~100 ms after the last key. */
  let q = $state("");
  let active = $state(0);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let listEl = $state<HTMLDivElement>();

  // The other names and readings: asked for the first time the palette opens. When they arrive
  // (or change) while it's open, the typed text is searched again (paletteItems reads them untracked).
  // The library's search index too, while nothing is typed yet (shared with the Search page).
  onMount(() => {
    app.wantSearchKeys();
    warmSearch();
  });
  const groups = $derived.by(() => {
    void app.searchKeys;
    try {
      return search(q, pages, actions);
    } catch (e) {
      // A broken list shows as empty; the rest of the window carries on.
      console.error("[Cadence] palette search failed:", e);
      return [];
    }
  });
  const flat = $derived(groups.flatMap((g) => g.items));
  /** The groups with each row’s place in the whole list (what Up and Down move through). */
  const rows = $derived.by(() => {
    let n = 0;
    return groups.map((g) => ({ kind: g.kind, title: g.title, items: g.items.map((item) => ({ item, i: n++ })) }));
  });

  function onInput(value: string) {
    query = value;
    clearTimeout(timer);
    if (!value.trim()) {
      q = "";
      active = 0;
      return;
    }
    timer = setTimeout(() => {
      q = value;
      active = 0;
    }, 100);
  }
  onDestroy(() => clearTimeout(timer));

  /** Enter right after a key press: the list must be the typed text's, not the last pause's. */
  function flush() {
    if (query === q) return;
    clearTimeout(timer);
    q = query;
    active = 0;
  }

  const close = () => (overlays.paletteOpen = false);

  /** Closes the palette, then does it (so a dialog it opens is on top, and Back goes where it should). */
  async function choose(item: PaletteItem | undefined) {
    if (!item) return;
    close();
    await tick();
    try {
      item.run();
    } catch (e) {
      console.error("[Cadence] palette action failed:", e);
    }
  }

  function move(to: number) {
    if (!flat.length) return;
    active = (to + flat.length) % flat.length;
    tick().then(() => listEl?.querySelector<HTMLElement>(`#pal-${active}`)?.scrollIntoView({ block: "nearest" }));
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      move(active + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      move(active - 1);
    } else if (e.key === "Home" && e.ctrlKey) {
      e.preventDefault();
      move(0);
    } else if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      flush();
      choose(flat[active]);
    } else if (e.key === "Tab") {
      // Nothing else in here to tab to.
      e.preventDefault();
    }
  }

</script>

<Modal title="Command palette" onclose={close} width="620px">
  <div class="pal">
    <label class="box">
      <Icon name="search" size={16} />
      <input
        role="combobox"
        aria-expanded="true"
        aria-controls="palette-list"
        aria-activedescendant={flat[active] ? `pal-${active}` : undefined}
        aria-label="Search pages, actions and your music"
        placeholder="Search pages, actions and your music"
        autocomplete="off"
        spellcheck="false"
        value={query}
        oninput={(e) => onInput(e.currentTarget.value)}
        onkeydown={onKey}
      />
    </label>

    <div class="list" id="palette-list" role="listbox" aria-label="Results" bind:this={listEl}>
      {#if !flat.length}
        <p class="none">Nothing matches. Try part of a title, an artist, a page or an action.</p>
      {/if}
      {#each rows as g (g.kind)}
        <div class="label" role="presentation">{g.title}</div>
        {#each g.items as { item, i } (item.id)}
          <!-- Not focusable: the box keeps the keyboard, so Up, Down and Enter always work. -->
          <div
            id="pal-{i}"
            class="item"
            class:active={i === active}
            role="option"
            tabindex="-1"
            aria-selected={i === active}
            onmousemove={() => active !== i && (active = i)}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => choose(item)}
            onkeydown={() => {}}
          >
            <Icon name={item.icon} size={17} />
            <span class="name">{item.label}</span>
            {#if item.hint}<span class="hint">{item.hint}</span>{/if}
            {#if item.keys}<span class="keys">{#each item.keys as k}<kbd>{k}</kbd>{/each}</span>{/if}
          </div>
        {/each}
      {/each}
    </div>
  </div>
</Modal>

<style>
  /* The dialog's title bar isn't needed: the box is the title. */
  :global([role="dialog"][aria-label="Command palette"] > header) {
    display: none;
  }
  .pal {
    margin-top: 12px;
  }
  .box {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 44px;
    padding: 0 14px;
    border-radius: var(--r-md);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    color: var(--text-3);
  }
  .box:focus-within {
    box-shadow: inset 0 0 0 1px var(--accent);
    color: var(--text-2);
  }
  input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: 0;
    background: none;
    font-size: 15px;
    color: var(--text);
    user-select: text;
  }
  input:focus-visible {
    outline: none;
  }
  input::placeholder {
    color: var(--text-3);
  }
  /* A fixed height, so the dialog doesn't change size (and jump) as the results change. */
  .list {
    height: min(380px, 50vh);
    overflow-y: auto;
    margin-top: 10px;
    padding-bottom: 4px;
  }
  .label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
    padding: 12px 10px 5px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 40px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    color: var(--text-2);
  }
  .item.active {
    background: var(--active);
    color: var(--text);
  }
  .item :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .item.active :global(svg) {
    color: var(--accent);
  }
  .name {
    color: var(--text);
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 0 1 auto;
    min-width: 0;
  }
  .hint {
    flex: 1;
    min-width: 0;
    font-size: 12.5px;
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .keys {
    margin-left: auto;
    display: flex;
    gap: 4px;
    flex: none;
  }
  kbd {
    font: inherit;
    font-size: 10.5px;
    color: var(--text-3);
    padding: 1px 5px;
    border-radius: 4px;
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .none {
    margin: 18px 10px;
    color: var(--text-2);
    font-size: 13px;
  }
</style>
