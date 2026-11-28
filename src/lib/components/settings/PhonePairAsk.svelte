<script lang="ts">
  // The question after the camera opened a computer's pairing QR code: pair with this computer?
  // (A link can come from anywhere, so nothing is paired until this is answered yes.)
  import { phoneSync } from "#lib/phoneSync.svelte.js";
  import Modal from "../Modal.svelte";

  const offer = $derived(phoneSync.offer);
</script>

{#if offer}
  <Modal title="Pair with this computer?" onclose={() => phoneSync.decline()}>
    <p class="ask">
      <b>{offer.computer || "A computer"}</b> on your network, at <span class="mono">{offer.address}</span>, wants to send music
      to this phone. Only pair with your own computer.
    </p>
    {#if phoneSync.pairError}<p class="error" role="alert">{phoneSync.pairError}</p>{/if}
    {#snippet footer()}
      <button class="pill ghost" onclick={() => phoneSync.decline()}>Cancel</button>
      <button class="pill" onclick={() => phoneSync.accept()} disabled={phoneSync.pairing}>{phoneSync.pairing ? "Pairing…" : "Pair"}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .ask {
    margin: 0;
    color: var(--text-2);
  }
  .mono {
    user-select: text;
  }
  .error {
    margin: 10px 0 0;
    color: var(--warn);
    font-size: 13px;
  }
</style>
