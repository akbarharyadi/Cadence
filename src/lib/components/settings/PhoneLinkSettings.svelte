<script lang="ts">
  // Settings › Sync with computer (the phone's side): pair with the computer's code once, then sync.
  // What to keep is chosen on the computer; the songs land in the phone's Music › Cadence folder.
  import { api } from "#lib/api.js";
  import { onMount } from "svelte";
  import { phoneName, phoneSync } from "#lib/phoneSync.svelte.js";
  import type { PhoneLink } from "#lib/phone.js";
  import ProgressBar from "../ui/ProgressBar.svelte";

  let link = $state<PhoneLink | null>(null);
  let address = $state("");
  let code = $state("");
  let busy = $state(false);
  let error = $state("");

  const job = $derived(phoneSync.syncJob);
  const last = $derived(phoneSync.lastSync);
  const plural = (n: number, one: string) => `${n.toLocaleString()} ${n === 1 ? one : `${one}s`}`;
  const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);
  const when = (secs: number) => new Date(secs * 1000).toLocaleString(undefined, { day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });

  async function load() {
    try {
      link = await api.phoneLink();
      if (!address && link.address) address = link.address;
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => void load());

  // A sync that just ended changed the song count and the last result.
  $effect(() => {
    if (last) void load();
  });

  async function step(fn: () => Promise<unknown>) {
    busy = true;
    error = "";
    try {
      await fn();
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  const pair = () => step(async () => {
    link = await api.phoneLinkPair(address, code, phoneName());
    code = "";
  });
  const sync = () => step(() => api.phoneSyncNow());
</script>

{#if link?.supported}
  <div>
    <h2 class="section-title">Sync with computer</h2>
    <p class="desc">
      Copy the music you choose on your computer to this phone, at full quality. On the computer, turn on Settings › Sync with
      phone and pick what to keep there. Both need to be on the same Wi-Fi, or connected by cable with USB tethering on.
    </p>
  </div>

  {#if !link.paired}
    <form class="pair" onsubmit={(e) => { e.preventDefault(); void pair(); }}>
      <label>Computer's address<input class="field" bind:value={address} placeholder="192.168.1.20:47300" autocomplete="off" inputmode="url" /></label>
      <label>Code<input class="field" bind:value={code} placeholder="6 digits" inputmode="numeric" maxlength="7" autocomplete="one-time-code" /></label>
      <button class="pill" type="submit" disabled={busy || !address.trim() || code.replace(/\D/g, "").length !== 6}>{busy ? "Pairing…" : "Pair"}</button>
    </form>
  {:else}
    <div class="box">
      <div class="row">
        <span>Paired with <b>{link.computer || "your computer"}</b> <span class="secondary small">· {link.address}</span></span>
        <button class="link small" onclick={() => step(api.phoneLinkForget)} disabled={busy || !!job}>Forget</button>
      </div>
      <p class="small secondary">
        {plural(link.songs, "song")} from the computer on this phone, in {link.folder}.
        {#if link.lastSync}Last sync {when(link.lastSync)}.{/if}
      </p>
    </div>

    {#if job}
      <div class="progress" role="status">
        <span class="small">
          {job.phase === "connecting" ? "Connecting…" : job.phase === "removing" ? "Removing songs no longer chosen…" : `${job.paused ? "Paused" : "Copying"} ${job.done} of ${plural(job.total, "song")}`}
          {#if job.bytesTotal}<span class="secondary"> · {size(job.bytesDone)} of {size(job.bytesTotal)}</span>{/if}
        </span>
        <ProgressBar value={job.bytesTotal ? job.bytesDone / job.bytesTotal : 0.06} />
        {#if job.current}<span class="small secondary current">{job.current}</span>{/if}
        <div class="actions">
          <button class="pill ghost" onclick={() => api.phoneSyncPause(!job.paused)}>{job.paused ? "Resume" : "Pause"}</button>
          <button class="link small" onclick={() => api.phoneSyncStop()}>Stop</button>
        </div>
      </div>
    {:else}
      <div class="actions">
        <button class="pill" onclick={sync} disabled={busy}>Sync now</button>
      </div>
      {#if last?.message || link.lastResult}
        <p class="small" class:warn={last?.failed} role="status">{last?.message || link.lastResult}</p>
      {/if}
    {/if}
  {/if}
  {#if error}<p class="small warn" role="alert">{error}</p>{/if}
{/if}

<style>
  .desc {
    margin: 6px 0 0;
    color: var(--text-2);
  }
  .pair {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 14px;
  }
  .pair label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--text-2);
    font-size: 13px;
  }
  .pair button {
    align-self: flex-start;
  }
  .box {
    margin-top: 14px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--hover);
  }
  .box p {
    margin: 4px 0 0;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .progress {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 14px;
  }
  .current {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-top: 12px;
  }
  .small {
    font-size: 13px;
  }
  .warn {
    color: var(--warn);
  }
</style>
