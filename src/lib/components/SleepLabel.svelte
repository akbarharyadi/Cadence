<script lang="ts">
  import { app } from "#lib/store.svelte.js";

  // Counts down once a second, only while this is on screen (it's only shown with a timer set).
  let now = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    if (app.sleepEndsAt == null) return;
    now = Math.floor(Date.now() / 1000);
    const timer = setInterval(() => (now = Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(timer);
  });

  /** "23 min", "45 s", or what the song / album rule is. */
  const text = $derived.by(() => {
    if (app.sleepEndsAt != null) {
      const left = Math.max(0, app.sleepEndsAt - now);
      return left >= 60 ? `${Math.ceil(left / 60)} min` : `${left} s`;
    }
    const stop = app.status?.stopAfter;
    if (!stop) return "";
    if (stop.kind === "album") return "End of album";
    return stop.left > 1 ? `${stop.left} songs` : "End of song";
  });
</script>

{text}
