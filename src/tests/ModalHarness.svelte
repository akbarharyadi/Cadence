<script lang="ts">
  // A dialog with a dropdown in it, and a second dialog opened on top of it from a button: the way
  // Cadence stacks dialogs (siblings opened one after the other, not one nested in the other).
  import Modal from "#lib/components/Modal.svelte";
  import Select from "#lib/components/Select.svelte";

  let { onOuterClose, onInnerClose }: { onOuterClose: () => void; onInnerClose: () => void } = $props();

  let value = $state("name");
  let outer = $state(true);
  let inner = $state(false);
</script>

{#if outer}
  <Modal title="Outer dialog" onclose={() => { outer = false; onOuterClose(); }}>
    <Select
      label="Sort by"
      value={value}
      options={[{ value: "name", label: "Name" }, { value: "year", label: "Year" }]}
      onchange={(v) => (value = v)}
    />
    <button onclick={() => (inner = true)}>Open second dialog</button>
  </Modal>
{/if}

{#if inner}
  <Modal title="Inner dialog" onclose={() => { inner = false; onInnerClose(); }}>
    <p>Second dialog</p>
  </Modal>
{/if}
