<script lang="ts">
  // The Sound page's output device: which one Cadence plays on, what happens when it isn't
  // connected, and whether each device keeps its own sound settings.
  import { onMount } from "svelte";
  import { api, type DeviceRef, type OutputDevices } from "#lib/api.js";
  import { deviceName } from "#lib/devices.js";
  import { app, coalesced } from "#lib/store.svelte.js";
  import Select from "./Select.svelte";
  import Toggle from "./Toggle.svelte";

  /** Windows tells Cadence when devices change; elsewhere the list has a Refresh button. */
  let { windows = false }: { windows?: boolean } = $props();

  let list = $state<OutputDevices | null>(null);
  let error = $state("");
  let gone = false;
  /** One request at a time; asked again while one runs: one more after it (never dropped). */
  const load = coalesced(async () => {
    try {
      const l = (await api.outputDevices()) ?? null;
      if (gone) return;
      list = l;
      error = "";
    } catch (e) {
      if (!gone) error = String(e);
    }
  });
  onMount(() => {
    void load();
    return () => (gone = true);
  });

  // The system's devices changed: listed again. Only when that count moves (no timer). The
  // count is first taken from the first status there is (no status yet isn't a count of 0), so
  // the first status after the page opened doesn't list them a second time.
  const changes = $derived(app.status?.devices);
  const seen = { count: undefined as number | undefined };
  $effect(() => {
    const n = changes;
    if (n === undefined) return;
    if (seen.count !== undefined && n !== seen.count) void load();
    seen.count = n;
  });

  const device = $derived(app.status?.device ?? null);
  const missing = $derived(app.status?.deviceMissing ?? null);
  const missingName = $derived(missing?.name ?? null);
  // "Keep waiting" hides the message until the device is missing again.
  let dismissed = $state<string | null>(null);
  $effect(() => {
    if (missingName === null) dismissed = null;
  });

  const options = $derived.by(() => {
    const devices = list?.devices ?? [];
    return [
      { value: "", label: windows ? "System default (follow Windows)" : "System default" },
      ...devices.map((d) => ({
        value: d.id,
        label: `${deviceName(d.name)}${d.isDefault ? " (system default)" : ""}${d.present ? "" : " (not connected)"}`,
      })),
    ];
  });
  const systemDefault = $derived(list?.devices.find((d) => d.isDefault) ?? null);

  async function choose(id: string) {
    if (!list) return;
    const d = id ? list.devices.find((x) => x.id === id) : undefined;
    const chosen: DeviceRef | null = d ? { id: d.id, name: d.name } : null;
    list = { ...list, chosen };
    try {
      await api.setOutputDevice(chosen);
    } catch (e) {
      error = String(e);
    }
  }

  async function setProfiles(on: boolean) {
    if (!list) return;
    list = { ...list, profiles: on };
    try {
      await api.setDeviceProfiles(on);
    } catch (e) {
      error = String(e);
    }
  }
</script>

{#if list?.supported}
  <div class="title-row">
    <div class="text">
      <h2 class="section-title">Output device</h2>
      <p class="desc">Where Cadence plays. A device you choose is kept, even when another one becomes the system default (a monitor, a Bluetooth headset).</p>
    </div>
    <Select label="Output device" options={options} value={list.chosen?.id ?? ""} onchange={choose} width="300px" />
  </div>
  {#if !list.chosen && systemDefault}
    <p class="hint">Now: {deviceName(systemDefault.name)}.</p>
  {/if}
  {#if list.chosen}
    <p class="hint">If it isn’t connected, playback pauses rather than moving to another device, and carries on when it’s back within two minutes.</p>
  {/if}
  {#if !windows}
    <p class="hint"><button class="link" onclick={() => void load()}>Refresh the list</button></p>
  {/if}
  {#if error}<p class="error">{error}</p>{/if}

  {#if missing && dismissed !== missing.name}
    <div class="missing" role="status">
      <p>
        <strong>{deviceName(missing.name)}</strong> isn’t connected, so playback is paused.
        {#if missing.resume}It carries on by itself if it’s back within two minutes.{/if}
      </p>
      <div class="buttons">
        {#if missing.fallback}
          <button class="pill" onclick={() => api.outputUseDefault()}>Play on {deviceName(missing.fallback)}</button>
        {/if}
        <button class="pill ghost" onclick={() => (dismissed = missing.name)}>Keep waiting</button>
      </div>
    </div>
  {/if}

  <div class="title-row profiles">
    <div class="text">
      <h3>Remember sound settings per device</h3>
      <p class="hint">
        The equalizer, crossfeed, “Keep it full when quieter”, Direct output and volume are kept for each device, and come back when you play on it again.
        A device used for the first time starts from the settings you have now (with Direct output, at no more than 30% volume).
      </p>
    </div>
    <Toggle checked={list.profiles} label="Remember sound settings per device" onchange={setProfiles} />
  </div>
  {#if list.profiles && device}
    <p class="profile-line">Editing the sound settings for {deviceName(device.name)}.</p>
  {/if}
{/if}

<style>
  .title-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 28px;
  }
  .text {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .desc {
    margin: 6px 0 0;
    color: var(--text-2);
    max-width: 66ch;
  }
  .hint {
    font-size: 12.5px;
    color: var(--text-3);
    margin: 8px 0 0;
    line-height: 1.5;
  }
  .error {
    color: var(--warn);
    font-size: 13px;
  }
  .missing {
    margin-top: 14px;
    padding: 12px 14px;
    border-radius: var(--r-md);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--warn) 45%, transparent);
    overflow-wrap: anywhere;
  }
  .missing p {
    margin: 0;
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 10px;
  }
  .profiles {
    margin-top: 20px;
  }
  .profiles h3 {
    margin: 0;
    font-size: 14.5px;
    font-weight: 600;
  }
  .profile-line {
    margin: 10px 0 0;
    font-size: 13px;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
</style>
