<script lang="ts">
  // The open-source projects Cadence is built from, with their license notices
  // (static/third-party-notices.txt, made by scripts/third-party-notices.py).
  import { openUrl } from "@tauri-apps/plugin-opener";
  import Modal from "./Modal.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let text = $state("");
  let error = $state("");
  fetch("/third-party-notices.txt")
    .then((r) => (r.ok ? r.text() : Promise.reject(r.status)))
    .then((t) => (text = t))
    .catch(() => (error = "The license list couldn't be opened."));
</script>

<Modal title="Open-source licenses" {onclose} width="760px">
  <p class="intro">
    Cadence is free software under the GNU General Public License v3.0. It’s built from the open-source projects below; these
    are their licenses and notices.
  </p>
  {#if error}
    <p class="error">{error}</p>
  {:else if !text}
    <p class="intro">Loading…</p>
  {:else}
    <pre aria-label="License notices">{text}</pre>
  {/if}

  {#snippet footer()}
    <button class="link note" onclick={() => openUrl("https://github.com/akbarharyadi/cadence")}>Source code on GitHub</button>
    <button class="pill" onclick={onclose}>Done</button>
  {/snippet}
</Modal>

<style>
  .intro {
    margin: 0 0 12px;
    font-size: 13.5px;
    color: var(--text-2);
  }
  pre {
    margin: 0;
    max-height: min(520px, 58vh);
    overflow: auto;
    padding: 12px 14px;
    border-radius: var(--r-sm);
    background: rgba(0, 0, 0, 0.25);
    box-shadow: inset 0 0 0 1px var(--line);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    user-select: text;
    color: var(--text-2);
  }
  .error {
    color: var(--warn);
    font-size: 13px;
  }
  .note {
    margin-right: auto;
    align-self: center;
    font-size: 13px;
  }
</style>
