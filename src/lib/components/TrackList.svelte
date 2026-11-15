<script lang="ts">
  import { artUrl, formatTime, type Track } from "#lib/api.js";
  import { app, type MenuItem } from "#lib/store.svelte.js";
  import { registerSelection, selectingIn } from "#lib/selection.js";
  import { onMount, untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import Rating from "./Rating.svelte";
  import SelectionBar from "./SelectionBar.svelte";
  import { eqBars } from "#lib/eq.js";
  import { nextRange } from "#lib/virtualWindow.js";

  let {
    tracks,
    numbered = false,
    showArt = false,
    showAlbum = false,
    hideArtist = "",
    label = "Songs",
    identity,
    extraMenu,
    onreorder,
    onremove,
  }: {
    tracks: Track[];
    numbered?: boolean;
    showArt?: boolean;
    showAlbum?: boolean;
    hideArtist?: string;
    /** The list's accessible name. */
    label?: string;
    /** When this changes the list shows other songs (not the same ones refreshed): the selection goes. */
    identity?: unknown;
    /** Extra right-click items for a row (e.g. "Remove from playlist"). */
    extraMenu?: (t: Track, i: number) => MenuItem[];
    /** Rows can be dragged to a new position. `rows`: several selected rows dragged together (ascending). */
    onreorder?: (from: number, to: number, rows?: number[]) => void;
    /** Rows can be removed (a user playlist): Delete, and "Remove from this playlist" for a selection. */
    onremove?: (rows: number[]) => void;
  } = $props();

  let dragFrom = $state<number | null>(null);
  let dropAt = $state<number | null>(null);
  /** Several selected rows being dragged together (the dragged row is one of them). */
  let dragRows: number[] | null = null;

  // A drag is ours only if it carries this type: a stale `dragFrom` must never accept someone else's drop.
  const ROW_TYPE = "application/x-cadence-row";
  const ours = (e: DragEvent) => !!e.dataTransfer?.types.includes(ROW_TYPE);

  let edgeSpeed = 0;
  let edgeFrame = 0;
  /** The element the page scrolls in (the app's `.content`, else the document). */
  const scrollerOf = (el: HTMLElement) => (el.closest(".content") as HTMLElement | null) ?? document.documentElement;
  /** Drag over: where the scrolling part of the page is, in the window. */
  function scrollBox(): { el: HTMLElement; top: number; bottom: number } | null {
    if (!listEl) return null;
    const el = scrollerOf(listEl);
    if (el === document.documentElement) return { el, top: 0, bottom: window.innerHeight };
    const r = el.getBoundingClientRect();
    return { el, top: r.top, bottom: r.bottom };
  }
  function edgeTick() {
    edgeFrame = 0;
    if (dragFrom === null || !edgeSpeed) return;
    const box = scrollBox();
    if (!box) return;
    box.el.scrollTop += edgeSpeed;
    edgeFrame = requestAnimationFrame(edgeTick);
  }
  /** Near the top or bottom edge of the page the list scrolls (faster the closer to the edge). */
  function autoScroll(e: DragEvent) {
    const box = scrollBox();
    if (!box) return;
    const ZONE = 80;
    const up = e.clientY - box.top;
    const down = box.bottom - e.clientY;
    edgeSpeed = up < ZONE ? -Math.ceil(((ZONE - Math.max(0, up)) / ZONE) * 20) : down < ZONE ? Math.ceil(((ZONE - Math.max(0, down)) / ZONE) * 20) : 0;
    if (edgeSpeed && !edgeFrame) edgeFrame = requestAnimationFrame(edgeTick);
  }
  function clearDrag() {
    dragFrom = null;
    dropAt = null;
    dragRows = null;
    edgeSpeed = 0;
    if (edgeFrame) cancelAnimationFrame(edgeFrame);
    edgeFrame = 0;
  }

  // The dragged row can leave the page (long lists build only the rows on screen) and then it never
  // gets its own dragend, so the end of a drag is also watched on the window.
  $effect(() => {
    if (!onreorder) return;
    const over = (e: DragEvent) => {
      if (dragFrom !== null && ours(e)) autoScroll(e);
    };
    const leave = (e: DragEvent) => {
      if (e.relatedTarget === null) {
        dropAt = null;
        edgeSpeed = 0;
      }
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && clearDrag();
    window.addEventListener("dragover", over);
    window.addEventListener("dragleave", leave);
    window.addEventListener("dragend", clearDrag);
    window.addEventListener("drop", clearDrag);
    window.addEventListener("pointerup", clearDrag);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("dragover", over);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("dragend", clearDrag);
      window.removeEventListener("drop", clearDrag);
      window.removeEventListener("pointerup", clearDrag);
      window.removeEventListener("keydown", key);
      clearDrag();
    };
  });

  // ---------- selection ----------
  // By row position in `tracks` (a playlist can list a song twice: each row is its own). A plain
  // Set, changed in place, plus a version number the rows read: a toggle is O(1), selecting a
  // 30,000-row range only fills the Set, and only the rows on screen look at it again.
  const sel = new Set<number>();
  let selV = $state(0);
  /** Where Shift+click / Shift+arrows extend from. */
  let anchor = -1;
  /** The row the keyboard is on (aria-activedescendant). */
  let active = $state(-1);
  /** Touch: after a long-press, taps add and remove rows instead of playing. */
  let mode = $state(false);
  /** The arrow keys were used last (not the mouse): the active row shows a focus ring. */
  let keyed = $state(false);

  const uid = $props.id();
  const me = Symbol("track-list");

  function isSel(i: number): boolean {
    void selV;
    return sel.has(i);
  }
  const count = $derived.by(() => {
    void selV;
    return sel.size;
  });
  const multi = $derived(count > 1 || mode);

  function changed() {
    selV++;
    if (sel.size) selectingIn(me);
  }
  function selectOnly(i: number) {
    sel.clear();
    if (i >= 0) sel.add(i);
    anchor = i;
    active = i;
    changed();
  }
  function toggle(i: number) {
    if (sel.has(i)) sel.delete(i);
    else sel.add(i);
    anchor = i;
    active = i;
    changed();
  }
  /** Selects anchor..to (replacing the selection, or `add`ing to it). */
  function selectRange(from: number, to: number, add = false) {
    if (!add) sel.clear();
    const lo = Math.max(0, Math.min(from, to));
    const hi = Math.min(tracks.length - 1, Math.max(from, to));
    for (let k = lo; k <= hi; k++) sel.add(k);
    active = to;
    changed();
  }
  function selectAll() {
    sel.clear();
    const n = tracks.length;
    for (let k = 0; k < n; k++) sel.add(k);
    if (active < 0 || active >= n) active = 0;
    anchor = active;
    changed();
  }
  function clearSelection() {
    sel.clear();
    mode = false;
    anchor = -1;
    changed();
  }
  /** The selected rows, top to bottom. */
  const rowsSelected = () => [...sel].filter((i) => i < tracks.length).sort((a, b) => a - b);
  const selectedTracks = () => rowsSelected().map((i) => tracks[i]);

  onMount(() =>
    registerSelection({
      id: me,
      // Android Back: selection mode (or several selected songs) goes first; one selected row is just where you are.
      leave: () => {
        if (!mode && sel.size < 2) return false;
        clearSelection();
        return true;
      },
      clear: () => {
        if (sel.size || mode) clearSelection();
      },
    }),
  );

  /** One key per row that tells two rows of the same song apart: the id, and which of its rows it is. */
  function rowKeys(list: Track[]): string[] {
    const seen = new Map<string, number>();
    return list.map((t) => {
      const n = seen.get(t.id) ?? 0;
      seen.set(t.id, n + 1);
      return n ? `${t.id}#${n}` : t.id;
    });
  }

  // The same list refreshed (library reload, a song removed): the selection follows its songs to
  // their new rows. Another list (`identity` changed): it's cleared. Only when something is
  // selected does this look at the rows (once per new list, never per frame or player tick).
  let shownTracks: Track[] = untrack(() => tracks);
  let shownIdentity: unknown = untrack(() => identity);
  $effect.pre(() => {
    const next = tracks;
    const id = identity;
    untrack(() => {
      try {
        const old = shownTracks;
        shownTracks = next;
        if (id !== shownIdentity) {
          shownIdentity = id;
          sel.clear();
          mode = false;
          anchor = active = -1;
          changed();
          return;
        }
        if (old === next || (!sel.size && active < 0 && anchor < 0)) return;
        const before = rowKeys(old);
        const where = new Map<string, number>();
        rowKeys(next).forEach((k, i) => where.set(k, i));
        const follow = (i: number, keep: boolean) => {
          if (i < 0) return -1;
          const j = where.get(before[i]);
          return j ?? (keep ? Math.min(i, next.length - 1) : -1);
        };
        const kept = [...sel].map((i) => follow(i, false)).filter((j) => j >= 0);
        sel.clear();
        for (const j of kept) sel.add(j);
        anchor = follow(anchor, true);
        active = follow(active, true);
        if (!sel.size) mode = false;
        changed();
      } catch {
        sel.clear();
        mode = false;
        anchor = active = -1;
        changed();
      }
    });
  });

  // ---------- pointer ----------
  /** Touch or mouse, from the last press on the list (a long-press arrives as a contextmenu event). */
  let pointer = "mouse";
  /** The click that ends a long-press must not also toggle the row. */
  let swallowClick = false;

  function rowClick(e: MouseEvent, i: number) {
    keyed = false;
    if (swallowClick) {
      swallowClick = false;
      return;
    }
    if (mode) {
      toggle(i);
      if (!sel.size) mode = false;
    } else if (e.shiftKey) {
      const from = anchor >= 0 && anchor < tracks.length ? anchor : i;
      anchor = from;
      selectRange(from, i, e.ctrlKey || e.metaKey);
    } else if (e.ctrlKey || e.metaKey) toggle(i);
    else selectOnly(i);
  }

  function rowContextMenu(e: MouseEvent, i: number) {
    if (pointer === "touch") {
      // Long-press: selection mode, with this row selected (the menu is in the bar's "More").
      e.preventDefault();
      swallowClick = true;
      if (!mode) {
        mode = true;
        selectOnly(i);
      } else if (!sel.has(i)) toggle(i);
      return;
    }
    menu(e, i);
  }

  /** Right-click / "More": the whole selection when the row is part of it, else just this row. */
  function menu(e: MouseEvent, i: number) {
    if (!tracks[i]) return;
    if (!sel.has(i)) selectOnly(i);
    if (sel.size > 1) openSelectionMenu(e);
    else app.trackMenu(e, tracks, i, extraMenu?.(tracks[i], i) ?? []);
  }

  function openSelectionMenu(e: MouseEvent) {
    const rows = rowsSelected();
    if (!rows.length) return;
    if (rows.length === 1) {
      const i = rows[0];
      app.trackMenu(e, tracks, i, extraMenu?.(tracks[i], i) ?? []);
      return;
    }
    app.songsMenu(e, rows.map((i) => tracks[i]), onremove ? [{ label: "Remove from this playlist", icon: "trash", action: () => removeSelected() }] : []);
  }

  function removeSelected() {
    const rows = rowsSelected();
    if (!onremove || !rows.length) return;
    sel.clear();
    mode = false;
    anchor = -1;
    // The keyboard stays where the first removed row was (the next song moves up into it).
    active = rows[0];
    changed();
    onremove(rows);
  }

  // ---------- keyboard ----------
  let typed = "";
  let typedTimer: ReturnType<typeof setTimeout> | undefined;
  /** Single keys that are window shortcuts (see AppKeys.svelte): never typed into the type-ahead. */
  const TYPEAHEAD_NEVER = new Set(["/", "?"]);
  $effect(() => () => clearTimeout(typedTimer));

  /** Type-ahead: the next song whose title starts with what was typed (the same letter again: the one after). */
  function typeAhead(ch: string) {
    clearTimeout(typedTimer);
    typed += ch.toLowerCase();
    typedTimer = setTimeout(() => (typed = ""), 800);
    const q = /^(.)\1+$/u.test(typed) ? typed[0] : typed;
    const n = tracks.length;
    const from = q.length === 1 ? active + 1 : Math.max(0, active);
    for (let k = 0; k < n; k++) {
      const i = (from + k) % n;
      if (tracks[i].title.toLowerCase().startsWith(q)) {
        selectOnly(i);
        reveal(i);
        return;
      }
    }
  }

  /** How many rows a page (PageUp / PageDown) is. */
  function pageRows(): number {
    if (!listEl) return 10;
    return Math.max(1, Math.floor(scrollerOf(listEl).clientHeight / ROW) - 1);
  }

  /** Scrolls row `i` into view (the virtual window then builds it): one measurement, one scroll. */
  function reveal(i: number) {
    const el = listEl;
    if (!el) return;
    const scroller = scrollerOf(el);
    const box = scroller === document.documentElement ? { top: 0, bottom: window.innerHeight } : scroller.getBoundingClientRect();
    let top: number;
    if (virtual) top = el.getBoundingClientRect().top + i * ROW;
    else {
      const row = el.querySelector<HTMLElement>(`[data-row-i="${i}"]`);
      if (!row) return;
      top = row.getBoundingClientRect().top;
    }
    // The sticky top bar covers the top 52 px; on phones the player and the tab bar cover the bottom.
    const above = box.top + 60;
    const below = box.bottom - (document.documentElement.classList.contains("phone") ? 150 : 12);
    if (top < above) scroller.scrollTop -= above - top;
    else if (top + ROW > below) scroller.scrollTop += top + ROW - below;
  }

  function onKey(e: KeyboardEvent) {
    // The row's own buttons (heart, stars, album) keep their keys.
    if (e.target !== listEl || e.altKey) return;
    const n = tracks.length;
    if (!n) return;
    const mod = e.ctrlKey || e.metaKey;
    const k = e.key;
    const cur = active >= 0 && active < n ? active : -1;
    let to = -1;
    if (k === "ArrowDown") to = cur < 0 ? 0 : Math.min(n - 1, cur + 1);
    else if (k === "ArrowUp") to = cur < 0 ? 0 : Math.max(0, cur - 1);
    else if (k === "Home") to = 0;
    else if (k === "End") to = n - 1;
    else if (k === "PageDown") to = cur < 0 ? 0 : Math.min(n - 1, cur + pageRows());
    else if (k === "PageUp") to = Math.max(0, cur - pageRows());
    if (to >= 0) {
      e.preventDefault();
      keyed = true;
      if (e.shiftKey) {
        const from = anchor >= 0 && anchor < n ? anchor : cur >= 0 ? cur : to;
        anchor = from;
        selectRange(from, to, mod);
      } else if (mod) active = to; // moves without selecting (Ctrl+Space then picks)
      else selectOnly(to);
      reveal(to);
      return;
    }
    if (mod && !e.shiftKey && k.toLowerCase() === "a") {
      e.preventDefault();
      selectAll();
    } else if (k === "Enter" && !mod && !e.shiftKey) {
      if (cur < 0) return;
      e.preventDefault();
      play(cur);
    } else if (k === " " && mod) {
      if (cur < 0) return;
      e.preventDefault();
      keyed = true;
      toggle(cur);
    } else if (k === "Escape") {
      // Used here: nothing behind the list (Back, a panel) also takes this Escape.
      if (!sel.size && !mode) return;
      e.preventDefault();
      e.stopPropagation();
      clearSelection();
    } else if ((k === "Delete" || k === "Backspace") && !mod) {
      if (!onremove || !sel.size) return;
      e.preventDefault();
      removeSelected();
    } else if (k === "ContextMenu" || (k === "F10" && e.shiftKey)) {
      if (cur < 0) return;
      e.preventDefault();
      const row = listEl?.querySelector<HTMLElement>(`[data-row-i="${cur}"]`);
      const r = row?.getBoundingClientRect();
      menu(new MouseEvent("contextmenu", { clientX: (r?.left ?? 0) + 60, clientY: (r?.top ?? 0) + ROW / 2 }), cur);
    } else if (!mod && !TYPEAHEAD_NEVER.has(k) && (/^[\p{L}\p{N}]$/u.test(k) || (typed && k.length === 1))) {
      // Letters and digits start it (other keys stay shortcuts; Space with nothing typed stays play / pause).
      // The window's own one-key shortcuts ("/" search, "?" help) are never part of it.
      e.preventDefault();
      typeAhead(k);
    }
  }

  const multiDisc = $derived(numbered && new Set(tracks.map((t) => t.discNo ?? 1)).size > 1);

  const isCurrent = (t: Track) => app.current?.id === t.id;
  const play = (i: number) => app.playTracks(tracks, i, true);

  // Long lists only build the rows on screen (plus a margin), so opening Songs with thousands of
  // songs stays quick and page animations don't stall. Rows are all ROW px tall; the space of
  // the rows that aren't built is kept as padding, so the page scrolls and sizes as before.
  const ROW = 54;
  const virtual = $derived(tracks.length > 80 && !multiDisc);
  let listEl = $state<HTMLDivElement>();
  let first = $state(0);
  let last = $state(60);
  // The window moves in steps (`nextRange`): most scroll events change nothing here (the browser
  // sends at most one per frame).
  $effect(() => {
    const el = listEl;
    if (!virtual || !el) return;
    const scroller = scrollerOf(el);
    const count = tracks.length;
    const update = (fresh = false) => {
      const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      const r = nextRange(fresh ? { first: 0, last: 0 } : { first, last }, { top, height: scroller.clientHeight, count, row: ROW });
      if (r.first !== first) first = r.first;
      if (r.last !== last) last = r.last;
    };
    untrack(() => update(true)); // reading first/last here mustn't make this effect re-run on every scroll
    const onScroll = () => update();
    scroller.addEventListener("scroll", onScroll, { passive: true });
    const resize = new ResizeObserver(onScroll);
    resize.observe(scroller);
    return () => {
      scroller.removeEventListener("scroll", onScroll);
      resize.disconnect();
    };
  });
  // While a row is being dragged it stays built (a removed row never gets its dragend), even
  // when scrolled far off: then it is the one extra row besides the window, taken out of the
  // flow (absolute, at its own place) so the padding below still adds up.
  const items = $derived.by(() => {
    if (!virtual) return tracks.map((t, i) => ({ t, i, pinned: false }));
    const out: { t: Track; i: number; pinned: boolean }[] = [];
    const from = dragFrom;
    const outside = from !== null && (from < first || from >= last) && from < tracks.length;
    if (outside && from! < first) out.push({ t: tracks[from!], i: from!, pinned: true });
    for (let i = first; i < last; i++) out.push({ t: tracks[i], i, pinned: false });
    if (outside && from! >= last) out.push({ t: tracks[from!], i: from!, pinned: true });
    return out;
  });
  /** The row under the pointer (its stars are built), from one listener on the list. */
  let hoverRow = $state(-1);
  const rowAt = (target: EventTarget | null) => Number((target as Element | null)?.closest?.("[data-row-i]")?.getAttribute("data-row-i") ?? -1);
  /** The active row, when it is built (a row scrolled out of the window has no element to point at). */
  const activeId = $derived(active >= 0 && active < tracks.length && (!virtual || (active >= first && active < last)) ? `${uid}-r${active}` : undefined);
</script>

<!-- One tab stop: the list has the keyboard and points at the active row (aria-activedescendant),
     so rows can come and go with the virtual window without losing focus. -->
<div
  class="list"
  class:numbered
  class:multi
  class:keyed
  class:virtual
  role="listbox"
  aria-label={label}
  aria-multiselectable="true"
  aria-activedescendant={activeId}
  tabindex="0"
  bind:this={listEl}
  onkeydown={onKey}
  onpointerdown={(e) => {
    pointer = e.pointerType || "mouse";
    swallowClick = false;
  }}
  onpointerover={(e) => (hoverRow = rowAt(e.target))}
  onpointerleave={() => (hoverRow = -1)}
  style={virtual ? `padding-top: ${first * ROW}px; padding-bottom: ${Math.max(0, tracks.length - last) * ROW}px` : undefined}
>
  {#each items as { t, i, pinned } (t.id + i)}
    {#if multiDisc && (i === 0 || (tracks[i - 1].discNo ?? 1) !== (t.discNo ?? 1))}
      <div class="disc">Disc {t.discNo ?? 1}</div>
    {/if}
    <!-- The list has the keyboard for its rows (see onKey); a row itself is not a tab stop. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
    <div
      class="row"
      id="{uid}-r{i}"
      data-row-i={i}
      class:current={isCurrent(t)}
      class:skipped={app.isSkipped(t)}
      class:selected={isSel(i)}
      class:active={active === i}
      class:pinned
      style={pinned ? `top: ${i * ROW}px` : undefined}
      class:drop-before={dropAt === i && dragFrom !== null && dragFrom > i}
      class:drop-after={dropAt === i && dragFrom !== null && dragFrom < i}
      draggable={!!onreorder}
      ondragstart={(e) => {
        dragFrom = i;
        dragRows = sel.size > 1 && sel.has(i) ? rowsSelected() : null;
        e.dataTransfer?.setData(ROW_TYPE, String(i));
        e.dataTransfer?.setData("text/plain", String(i));
        if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
      }}
      ondragover={(e) => { if (onreorder && dragFrom !== null && ours(e)) { e.preventDefault(); dropAt = i; } }}
      ondragend={clearDrag}
      ondrop={(e) => {
        if (!ours(e)) return;
        e.preventDefault();
        const from = dragFrom;
        const rows = dragRows;
        clearDrag();
        if (from === null || from === i) return;
        // Several selected rows move together; dropped onto one of them, nothing moves.
        if (rows) {
          if (!rows.includes(i)) onreorder?.(from, i, rows);
        } else onreorder?.(from, i);
      }}
      role="option"
      aria-selected={isSel(i)}
      onclick={(e) => rowClick(e, i)}
      ondblclick={() => !mode && play(i)}
      oncontextmenu={(e) => rowContextMenu(e, i)}
    >
      <div class="lead">
        {#if showArt}
          <div class="thumb">
            {#if app.albumById.get(t.albumId)?.artSmall}
              <img src={artUrl(app.albumById.get(t.albumId)?.artSmall)} alt="" loading="lazy" />
            {/if}
            {#if isCurrent(t) && app.status?.playing}<div class="eq-anim overlay" use:eqBars><i></i><i></i><i></i></div>{/if}
          </div>
        {:else if isCurrent(t) && app.status?.playing}
          <div class="eq-anim" use:eqBars><i></i><i></i><i></i></div>
        {:else}
          <span class="num">{numbered ? (t.trackNo ?? i + 1) : i + 1}</span>
        {/if}
        <button class="row-play" tabindex="-1" aria-label="Play {t.title}" onclick={(e) => { e.stopPropagation(); play(i); }}>
          <Icon name="play" size={14} />
        </button>
      </div>
      <div class="main">
        <div class="title">
          {#if app.isSkipped(t)}<span class="skip-mark" title="Skipped when albums, playlists and shuffles play"><Icon name="skip" size={13} /></span>{/if}{t.title}
        </div>
        {#if t.artist !== hideArtist}<div class="sub">{t.artist}</div>{/if}
      </div>
      {#if showAlbum}
        <button class="album" tabindex="-1" onclick={(e) => { e.stopPropagation(); app.go({ name: "album", id: t.albumId }); }}>{t.album}</button>
      {/if}
      <div class="marks" class:rated={app.ratingOf(t) > 0}>
        <!-- Built only where they show (rated, pointed at, the keyboard's row): five buttons per
             row were a third of a long list's elements. The empty span keeps their room. -->
        <span class="stars">
          {#if app.ratingOf(t) > 0 || hoverRow === i || active === i}<Rating value={app.ratingOf(t)} size={13} onrate={(n) => app.setRating([t], n)} label="Rating for {t.title}" />{/if}
        </span>
        <button
          class="heart"
          class:loved={app.isLoved(t)}
          aria-label={app.isLoved(t) ? `Remove ${t.title} from Favorites` : `Add ${t.title} to Favorites`}
          aria-pressed={app.isLoved(t)}
          title={app.isLoved(t) ? "Remove from Favorites" : "Add to Favorites"}
          onclick={(e) => { e.stopPropagation(); app.toggleLoved(t); }}
        >
          <Icon name={app.isLoved(t) ? "heartFill" : "heart"} size={16} />
        </button>
      </div>
      <div class="time">{formatTime(t.duration)}</div>
      <button class="more" tabindex="-1" aria-label="More options for {t.title}" title="More options" onclick={(e) => { e.stopPropagation(); menu(e, i); }}>
        <Icon name="more" size={16} />
      </button>
    </div>
  {/each}
</div>

<!-- Not over Now Playing or an open Up Next (the selection stays: the bar is back when they close). -->
{#if multi && !app.nowPlayingOpen && !app.queueOpen}
  <SelectionBar
    {count}
    onplay={() => app.playTracks(selectedTracks(), 0, true)}
    onqueue={() => app.enqueue(selectedTracks(), false)}
    onplaylist={() => (app.picker = selectedTracks())}
    onremove={onremove ? removeSelected : undefined}
    onmore={openSelectionMenu}
    onclear={clearSelection}
  />
{/if}

<style>
  .list {
    outline: none;
    position: relative;
  }
  /* The row being dragged, far outside the built window: out of the flow, at its own place. */
  .row.pinned {
    position: absolute;
    left: 0;
    right: 0;
  }
  .list .row.pinned::before {
    display: none;
  }
  .disc {
    font-family: var(--display);
    font-weight: 600;
    font-size: 15px;
    color: var(--text-2);
    padding: 20px 12px 8px;
  }
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 14px;
    height: 54px;
    padding: 0 12px;
    border-radius: var(--r-sm);
    outline: none;
  }
  /* A short list builds every row: those off screen skip their rendering. (A long one builds only
     the rows on screen, where this would only add work to every frame of a scroll.) */
  .list:not(.virtual) .row {
    content-visibility: auto;
    contain-intrinsic-size: auto 54px;
  }
  /* Hairline between rows, hidden next to a highlighted row. */
  .row + .row::before {
    content: "";
    position: absolute;
    top: 0;
    left: 54px;
    right: 12px;
    height: 1px;
    background: var(--line);
  }
  .row:hover::before,
  .row:hover + .row::before,
  .row.selected::before,
  .row.selected + .row::before,
  .row.current::before,
  .row.current + .row::before {
    opacity: 0;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.selected {
    background: var(--active);
  }
  .row.drop-before {
    box-shadow: inset 0 2px 0 var(--accent);
  }
  .row.drop-after {
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  /* The keyboard's row, while the list has the keyboard (arrows used, not the mouse). */
  .list.keyed:focus .row.active {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .row.current {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  /* Several selected: tinted with the accent so the set reads at a glance (in every theme; above
     the playing row's tint, whose title stays accent-coloured). */
  .list.multi .row.selected {
    background: color-mix(in srgb, var(--accent) 22%, var(--active));
  }
  .list.multi .row.selected:hover {
    background: color-mix(in srgb, var(--accent) 28%, var(--active));
  }
  .row.current .title {
    color: var(--accent);
    font-weight: 600;
  }
  /* Skipped songs stay listed, but quieter. */
  .row.skipped .main,
  .row.skipped .album,
  .row.skipped .thumb {
    opacity: 0.45;
  }
  .skip-mark {
    display: inline-grid;
    vertical-align: -2px;
    margin-right: 6px;
  }
  .lead {
    position: relative;
    width: 28px;
    flex: none;
    display: grid;
    place-items: center;
  }
  .list:has(.thumb) .lead {
    width: 40px;
  }
  .num {
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
    font-size: 13px;
  }
  .thumb {
    width: 40px;
    height: 40px;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
    position: relative;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .row-play {
    position: absolute;
    inset: 0;
    display: none;
    place-items: center;
    color: var(--text);
    border-radius: 6px;
  }
  .thumb + .row-play {
    background: rgba(0, 0, 0, 0.5);
    color: white;
  }
  .row:hover .row-play {
    display: grid;
  }
  .row:hover .num,
  .row:hover .lead > .eq-anim {
    visibility: hidden;
  }
  .main {
    flex: 1;
    min-width: 0;
  }
  .title,
  .sub,
  .album {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title {
    font-weight: 500;
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .album {
    flex: 0 1 32%;
    min-width: 0;
    text-align: left;
    color: var(--text-2);
    font-size: 13px;
  }
  .album:hover {
    color: var(--text);
    text-decoration: underline;
  }
  /* Stars show on hover (always when the song is rated); the heart shows when it's a favourite. */
  .marks {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  /* Room for five 13 px stars (each with 2 px padding, 1 px apart), built or not. */
  .stars {
    opacity: 0;
    display: inline-block;
    min-width: 89px;
  }
  .row:hover .stars,
  .row.selected .stars,
  .marks.rated .stars,
  .stars:focus-within {
    opacity: 1;
  }
  .heart {
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    color: var(--text-2);
    opacity: 0;
  }
  .heart.loved {
    opacity: 1;
    color: var(--accent);
  }
  .row:hover .heart,
  .row.selected .heart,
  .heart:focus-visible {
    opacity: 1;
  }
  .heart:hover {
    background: var(--active);
    color: var(--accent);
  }
  .time {
    width: 48px;
    text-align: right;
    color: var(--text-3);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }
  .more {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: var(--text-2);
    opacity: 0;
  }
  .row:hover .more,
  .row.selected .more,
  .more:focus-visible {
    opacity: 1;
  }
  .more:hover {
    background: var(--active);
    color: var(--text);
  }

  /* Little "now playing" bars. */
  .eq-anim {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 14px;
  }
  .eq-anim.overlay {
    position: absolute;
    inset: 0;
    height: auto;
    padding: 12px 11px;
    background: rgba(0, 0, 0, 0.45);
  }
  /* Scaled, not resized, so the page isn't laid out again; they bounce by eqBars. */
  .eq-anim i {
    width: 3px;
    align-self: stretch;
    background: var(--accent);
    border-radius: 1px;
    transform-origin: bottom;
    transform: scaleY(0.6);
  }
  .eq-anim.overlay i {
    background: white;
  }
  /* Phones: the album column would squeeze the titles; it's still one tap away in the menu. */
  @media (max-width: 720px) {
    .album,
    .stars {
      display: none;
    }
    .heart {
      opacity: 1;
    }
    .heart:not(.loved) {
      opacity: 0.5;
    }
    .row {
      padding: 0 6px;
    }
  }
  /* Android wider than a phone: touch, so the same hover-free rules (the album column may stay
     where there is room). */
  :global(html.phone) .stars {
    display: none;
  }
  :global(html.phone) .heart {
    opacity: 1;
  }
  :global(html.phone) .heart:not(.loved) {
    opacity: 0.5;
  }
  :global(html.phone) .row {
    padding: 0 6px;
  }
</style>
