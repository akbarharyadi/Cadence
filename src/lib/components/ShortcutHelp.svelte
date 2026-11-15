<script lang="ts">
  // "?": every keyboard shortcut, from the one list in shortcuts.ts.
  import { overlays } from "#lib/palette.svelte.js";
  import { SHORTCUT_GROUPS } from "#lib/shortcuts.js";
  import Modal from "./Modal.svelte";
</script>

<Modal title="Keyboard shortcuts" onclose={() => (overlays.helpOpen = false)} width="560px">
  <div class="groups">
    {#each SHORTCUT_GROUPS as g (g.title)}
      <section>
        <h3>{g.title}</h3>
        <dl>
          {#each g.items as s (s.id)}
            <dt>{#each s.keys as k}<kbd>{k}</kbd>{/each}</dt>
            <dd>{s.action}</dd>
          {/each}
        </dl>
      </section>
    {/each}
  </div>
</Modal>

<style>
  .groups {
    max-height: min(60vh, 520px);
    overflow-y: auto;
    padding-right: 4px;
  }
  h3 {
    margin: 16px 0 8px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
  }
  section:first-child h3 {
    margin-top: 4px;
  }
  dl {
    display: grid;
    grid-template-columns: 150px 1fr;
    gap: 7px 14px;
    margin: 0;
    align-items: center;
  }
  dt {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  dd {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
  }
  kbd {
    font: inherit;
    font-size: 11.5px;
    padding: 2px 7px;
    border-radius: 5px;
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line-strong);
    color: var(--text);
  }
  @media (max-width: 720px) {
    dl {
      grid-template-columns: 110px 1fr;
    }
  }
</style>
