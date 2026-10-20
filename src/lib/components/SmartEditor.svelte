<script lang="ts">
  import { FIELDS, OPS, QUALITIES, SORTS, evaluate, kindOf, type Field, type Rule } from "$lib/smart";
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Segmented from "./Segmented.svelte";
  import Select from "./Select.svelte";

  // Edits a copy; nothing is saved until "Save".
  const p = $derived(app.smartEditing!);
  const smart = $derived(p.smart!);
  const preview = $derived(evaluate(smart, app.tracks, app.listening, p.id));
  const isNew = $derived(!p.id);

  function setField(r: Rule, f: Field) {
    r.field = f;
    const kind = kindOf(f);
    r.op = OPS[kind][0].value;
    r.value = kind === "quality" ? "lossless" : kind === "date" ? "30" : "";
  }

  async function save() {
    if (!p.name.trim()) p.name = "Smart playlist";
    const saved = await app.savePlaylist(p);
    app.smartEditing = null;
    app.go({ name: "playlist", id: saved.id });
  }
</script>

<Modal title={isNew ? "New smart playlist" : "Edit smart playlist"} onclose={() => (app.smartEditing = null)} width="720px">
  <label class="name"><span>Name</span><input class="field" bind:value={p.name} placeholder="Smart playlist" /></label>

  <div class="match">
    <span>Include songs that match</span>
    <Segmented label="Match" options={[{ value: "all", label: "all rules" }, { value: "any", label: "any rule" }]} value={smart.match} onchange={(v) => (smart.match = v)} />
  </div>

  <div class="rules">
    {#each smart.rules as r, i (i)}
      {@const kind = kindOf(r.field)}
      <div class="rule">
        <Select label="Field" options={FIELDS.map((f) => ({ value: f.value, label: f.label }))} value={r.field} onchange={(v) => setField(r, v)} width="170px" />
        <Select label="Condition" options={OPS[kind]} value={r.op} onchange={(v) => (r.op = v)} width="190px" />
        {#if kind === "quality"}
          <Select label="Quality" options={QUALITIES} value={r.value} onchange={(v) => (r.value = v)} width="170px" />
        {:else}
          <input class="field value" bind:value={r.value} inputmode={kind === "text" ? "text" : "numeric"} placeholder={kind === "text" ? "text" : kind === "date" ? "days" : "number"} aria-label="Value" />
        {/if}
        <button class="icon-btn" aria-label="Remove rule" title="Remove rule" onclick={() => smart.rules.splice(i, 1)} disabled={smart.rules.length === 1}>
          <Icon name="trash" size={15} />
        </button>
      </div>
    {/each}
    <button class="add" onclick={() => smart.rules.push({ field: "artist", op: "contains", value: "" })}><Icon name="plus" size={14} />Add rule</button>
  </div>

  <div class="tail">
    <label><span>Order</span><Select label="Order" options={SORTS} value={smart.sort} onchange={(v) => (smart.sort = v)} width="210px" /></label>
    <label><span>Keep at most</span><input class="field num" type="number" min="0" bind:value={smart.limit} aria-label="Song limit" /><em>songs (0 = no limit)</em></label>
  </div>

  {#snippet footer()}
    <span class="preview">{preview.length} {preview.length === 1 ? "song matches" : "songs match"}</span>
    <button class="pill ghost" onclick={() => (app.smartEditing = null)}>Cancel</button>
    <button class="pill" onclick={save}>{isNew ? "Create playlist" : "Save"}</button>
  {/snippet}
</Modal>

<style>
  .name {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .name span,
  .match span,
  .tail span {
    color: var(--text-2);
    font-size: 13px;
  }
  .name .field {
    flex: 1;
    font-size: 15px;
    padding: 9px 12px;
  }
  .match {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 18px 0 12px;
  }
  .rules {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-radius: var(--r-md);
    background: var(--hover);
  }
  .rule {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .value {
    flex: 1;
    min-width: 0;
    height: 34px;
  }
  .add {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    padding: 6px 10px;
    border-radius: var(--r-sm);
    font-size: 13px;
    font-weight: 600;
    color: var(--accent);
  }
  .add:hover {
    background: var(--active);
  }
  .tail {
    display: flex;
    flex-wrap: wrap;
    gap: 14px 28px;
    margin-top: 16px;
  }
  .tail label {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .num {
    width: 72px;
    height: 34px;
  }
  em {
    font-style: normal;
    font-size: 12px;
    color: var(--text-3);
  }
  .preview {
    margin-right: auto;
    align-self: center;
    color: var(--text-2);
    font-size: 13px;
  }
</style>
