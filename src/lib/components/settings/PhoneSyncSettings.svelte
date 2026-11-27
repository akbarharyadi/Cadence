<script lang="ts">
  // Settings › Sync with phone (the computer's side): turn it on, pair a phone with a code, and see
  // what goes on the phone. What to keep is chosen in the playlist, album and artist menus
  // ("Keep on Phone"), plus the most played songs here. Design: docs/research-phone-sync.md.
  import { api } from "#lib/api.js";
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { phoneSync } from "#lib/phoneSync.svelte.js";
  import { TOP_CHOICES, type PhoneService, type PhoneSummary } from "#lib/phone.js";
  import Toggle from "../Toggle.svelte";
  import Segmented from "../Segmented.svelte";

  let service = $state<PhoneService | null>(null);
  let summary = $state<PhoneSummary | null>(null);
  let busy = $state(false);
  let error = $state("");

  const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;
  const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);
  const time = (secs: number) => new Date(secs * 1000).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const day = (secs: number) => (secs ? new Date(secs * 1000).toLocaleString(undefined, { day: "numeric", month: "short", hour: "numeric", minute: "2-digit" }) : "never");
  const topOptions = TOP_CHOICES.map((n) => ({ value: String(n), label: n ? `Top ${n}` : "None" }));

  async function load() {
    try {
      [service, summary] = await Promise.all([api.phoneService(), api.phoneSummary()]);
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    void load();
    // Changes from the menus, a phone pairing or syncing: shown when they happen.
    const stops = ["phone-changed", "phone-paired", "phone-synced"].map((name) => listen(name, () => void load()));
    return () => stops.forEach((s) => s.then((stop) => stop()).catch(() => {}));
  });

  async function run(step: () => Promise<PhoneService>) {
    busy = true;
    error = "";
    try {
      service = await step();
      if (service.error) error = service.error;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function setTop(v: string) {
    await api.phoneSetTop(Number(v));
    summary = await api.phoneSummary();
  }

  const choice = $derived(summary?.choice);
  const chosenLine = $derived(
    choice
      ? [plural(choice.playlists.length, "playlist"), plural(choice.albums.length, "album"), plural(choice.artists.length, "artist")].join(", ")
      : "",
  );
</script>

<div class="title-row">
  <div>
    <h2 class="section-title">Sync with phone</h2>
    <p class="desc">
      Keep music from this computer on your phone. Your phone's Cadence copies it over your home Wi-Fi, or over the cable with
      USB tethering turned on. Songs are copied as they are, at full quality.
    </p>
  </div>
  <Toggle checked={!!service?.enabled} label="Let my phone sync from this computer" onchange={(v) => run(() => api.setPhoneSync(v))} />
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if service?.on}
  <div class="box">
    <div class="row">
      <span>This computer: <b>{service.computer}</b>{#if service.address} · <span class="mono">{service.address}</span>{/if}</span>
      <button class="pill ghost" onclick={() => run(api.phonePair)} disabled={busy}>Pair a phone</button>
    </div>
    {#if service.code}
      <p class="code" role="status">
        On your phone, enter <span class="mono big">{service.code}</span> to pair. It works once, until {time(service.codeUntil)}.
      </p>
    {/if}
    {#each service.phones as p (p.id)}
      <div class="row phone">
        <span>{p.name || "Phone"} <span class="secondary small">· last synced {day(p.lastSeen)}</span></span>
        <button class="link small" onclick={() => run(() => api.phoneUnpair(p.id))} disabled={busy} aria-label={`Forget ${p.name || "this phone"}`}>Forget</button>
      </div>
    {/each}
  </div>
{/if}

<div class="what">
  <span class="label">Also keep your most played songs</span>
  <Segmented label="Most played songs to keep on the phone" options={topOptions} value={String(choice?.top ?? 0)} onchange={setTop} />
</div>
<p class="hint">
  To choose playlists, albums and artists, right-click one and pick “Keep on Phone”.
  {#if summary}Chosen: {chosenLine}.{/if}
</p>
{#if summary}
  <p class="total" data-testid="phone-total">
    {#if summary.songs}{plural(summary.songs, "song")}, {size(summary.bytes)}.{:else}Nothing chosen yet.{/if}
    {#if summary.pending}<span class="secondary">{plural(summary.pending, "song")} still being prepared; they go on a later sync.</span>{/if}
    {#if summary.leftOut}<span class="secondary">{plural(summary.leftOut, "song")} from albums stored as one file can't be sent yet.</span>{/if}
    {#if summary.notSentPlaylists}<span class="secondary">{plural(summary.notSentPlaylists, "smart playlist")} can't be sent yet.</span>{/if}
  </p>
{/if}
{#if phoneSync.keysJob}
  <button class="link small" onclick={() => api.phoneKeysPause(!phoneSync.keysJob?.paused)}>{phoneSync.keysJob.paused ? "Resume preparing" : "Pause preparing"}</button>
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
  .error {
    margin: 8px 0 0;
    font-size: 13px;
    color: var(--warn);
  }
  .box {
    margin-top: 14px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--hover);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    min-height: 34px;
  }
  .phone {
    border-top: 1px solid var(--line);
  }
  .mono {
    font-variant-numeric: tabular-nums;
    user-select: text;
  }
  .big {
    font-size: 18px;
    font-weight: 600;
    letter-spacing: 0.08em;
    color: var(--text);
  }
  .code {
    margin: 6px 0 8px;
    color: var(--text-2);
  }
  .what {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-top: 18px;
  }
  .label {
    color: var(--text-2);
  }
  .hint,
  .total {
    margin: 8px 0 0;
    font-size: 13px;
    color: var(--text-3);
  }
  .total {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 8px;
    color: var(--text);
  }
  .small {
    font-size: 13px;
  }
  @media (max-width: 720px) {
    .title-row,
    .what {
      flex-direction: column;
      align-items: flex-start;
      gap: 12px;
    }
  }
</style>
