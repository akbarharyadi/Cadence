<script lang="ts">
  import { api, type AiSettings } from "$lib/api";
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";

  const s = $derived(app.ai?.settings);
  let keyText = $state("");
  let keyError = $state("");
  let test = $state<{ busy: boolean; ok?: boolean; text: string }>({ busy: false, text: "" });
  let preview = $state("");
  let editKey = $state(false);

  function change(patch: Partial<AiSettings>) {
    if (!s) return;
    test = { busy: false, text: "" };
    app.saveAiSettings({ ...s, ...patch });
  }

  async function saveKey() {
    keyError = "";
    try {
      app.ai = await api.setAiKey(keyText);
      keyText = "";
      editKey = false;
      runTest();
    } catch (e) {
      keyError = String(e);
    }
  }

  async function removeKey() {
    app.ai = await api.setAiKey("");
  }

  async function runTest() {
    test = { busy: true, text: "" };
    try {
      await api.aiTest();
      test = { busy: false, ok: true, text: `Connected to ${s?.cloudModel}. Ask the DJ on Listen Now.` };
      app.loadAiMixes(false);
      app.loadAiNew(false);
    } catch (e) {
      test = { busy: false, ok: false, text: String(e) };
    }
  }

  async function showPreview() {
    preview = preview ? "" : await api.aiPreview("calm songs for late-night coding");
  }
</script>

{#if s}
  <div class="title-row">
    <div>
      <h2 class="section-title">AI DJ</h2>
      <p class="desc">
        Ask for music in your own words, get daily mixes and new-music ideas, and find lyrics. Uses Z.ai GLM (or any OpenAI-compatible
        service) with your own API key.
      </p>
    </div>
    <Toggle checked={s.provider === "cloud"} label="AI DJ" onchange={(v) => change({ provider: v ? "cloud" : "off" })} />
  </div>

  {#if s.provider === "cloud"}
    <div class="grid">
      <span class="label">API key</span>
      {#if app.ai?.hasKey && !editKey}
        <div class="line">
          <span class="saved"><Icon name="lossless" size={14} />Saved in Windows Credential Manager</span>
          <button class="link small" onclick={() => (editKey = true)}>Change</button>
          <button class="link small" onclick={removeKey}>Remove</button>
        </div>
      {:else}
        <form class="key" onsubmit={(e) => { e.preventDefault(); saveKey(); }}>
          <textarea class="field" rows="4" bind:value={keyText} placeholder={"Paste your API key, or your whole .env file:\nZAI_API_KEY=…\nAI_MODEL=glm-5.3-flash\nAI_BASE_URL=https://api.z.ai/api/coding/paas/v4"} aria-label="API key or .env" spellcheck="false"></textarea>
          <div class="line">
            <button class="pill" type="submit" disabled={!keyText.trim()}>Save key</button>
            {#if editKey}<button class="pill ghost" type="button" onclick={() => { editKey = false; keyText = ""; }}>Cancel</button>{/if}
          </div>
        </form>
      {/if}
      <label class="label" for="ai-model">Model</label>
      <input id="ai-model" class="field short" value={s.cloudModel} onchange={(e) => change({ cloudModel: e.currentTarget.value.trim() })} spellcheck="false" />
      <label class="label" for="ai-url">Server</label>
      <input id="ai-url" class="field" value={s.cloudUrl} onchange={(e) => change({ cloudUrl: e.currentTarget.value.trim() })} spellcheck="false" />
      <span class="label">Web search</span>
      <div class="line">
        <Toggle checked={s.webSearch} label="Web search" onchange={(v) => change({ webSearch: v })} />
        <span class="hint inline">Lets GLM look up new releases and lyrics.</span>
      </div>
    </div>
    {#if keyError}<p class="error">{keyError}</p>{/if}

    <div class="line actions">
      <button class="pill ghost" onclick={runTest} disabled={test.busy || !app.ai?.hasKey}>{test.busy ? "Checking…" : "Test connection"}</button>
      <button class="link small" onclick={showPreview}>{preview ? "Hide what’s sent" : "See what’s sent"}</button>
    </div>
    {#if test.text}<p class={test.ok ? "ok" : "error"}>{test.text}</p>{/if}
    {#if preview}
      <p class="hint">An example request. Only song details, a few words about how each song sounds, and play counts are sent; never your files or folder names.</p>
      <pre class="preview">{preview}</pre>
    {/if}
  {/if}
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
  .grid {
    display: grid;
    grid-template-columns: 90px minmax(0, 520px);
    align-items: center;
    gap: 12px 16px;
    margin-top: 18px;
  }
  .label {
    font-size: 13px;
    color: var(--text-2);
    align-self: start;
    padding-top: 8px;
  }
  .field {
    font-size: 14px;
    padding: 8px 12px;
    width: 100%;
  }
  .field.short {
    max-width: 260px;
  }
  textarea.field {
    resize: vertical;
    font-family: inherit;
    line-height: 1.45;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .key {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .saved {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 13.5px;
    color: var(--good);
  }
  .actions {
    margin-top: 18px;
  }
  .small {
    font-size: 13px;
  }
  .hint {
    margin: 10px 0 0;
    font-size: 12.5px;
    color: var(--text-3);
    max-width: 70ch;
  }
  .hint.inline {
    margin: 0;
  }
  .error,
  .ok {
    margin: 10px 0 0;
    font-size: 13px;
    max-width: 70ch;
  }
  .error {
    color: var(--warn);
  }
  .ok {
    color: var(--good);
  }
  .preview {
    margin: 10px 0 0;
    max-height: 260px;
    overflow: auto;
    padding: 12px 14px;
    border-radius: var(--r-md);
    background: var(--ink-2);
    box-shadow: inset 0 0 0 1px var(--line);
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-2);
    white-space: pre-wrap;
    user-select: text;
  }
</style>
