<script lang="ts">
  // Small inline icon set (24×24, stroke or fill), so the app ships without an icon font.
  let { name, size = 18 }: { name: string; size?: number } = $props();

  const paths: Record<string, string> = {
    play: '<path d="M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5Z" fill="currentColor"/>',
    pause: '<rect x="6" y="4" width="4" height="16" rx="1.2" fill="currentColor"/><rect x="14" y="4" width="4" height="16" rx="1.2" fill="currentColor"/>',
    next: '<path d="M4 5.2v13.6a.8.8 0 0 0 1.25.66L14 13v5.8a.8.8 0 0 0 1.25.66l8.5-6.8a.8.8 0 0 0 0-1.32l-8.5-6.8A.8.8 0 0 0 14 5.2V11L5.25 4.54A.8.8 0 0 0 4 5.2Z" fill="currentColor" transform="translate(-1.5 0)"/>',
    prev: '<path d="M20 5.2v13.6a.8.8 0 0 1-1.25.66L10 13v5.8a.8.8 0 0 1-1.25.66l-8.5-6.8a.8.8 0 0 1 0-1.32l8.5-6.8A.8.8 0 0 1 10 5.2V11l8.75-6.46A.8.8 0 0 1 20 5.2Z" fill="currentColor" transform="translate(1.5 0)"/>',
    shuffle: '<path d="M16 4h4v4M4 20 20 4M20 16v4h-4M15 15l5 5M4 4l5 5" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    repeat: '<path d="M17 2l3 3-3 3M4 11V9a4 4 0 0 1 4-4h12M7 22l-3-3 3-3M20 13v2a4 4 0 0 1-4 4H4" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    repeatOne: '<path d="M17 2l3 3-3 3M4 11V9a4 4 0 0 1 4-4h12M7 22l-3-3 3-3M20 13v2a4 4 0 0 1-4 4H4" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/><path d="M11 10.5 12.5 9.5V15" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    volume: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="currentColor"/><path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>',
    volumeLow: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="currentColor"/><path d="M15.5 8.5a5 5 0 0 1 0 7" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>',
    mute: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" fill="currentColor"/><path d="m16 9.5 5 5m0-5-5 5" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/>',
    queue: '<path d="M4 6h12M4 11h12M4 16h7" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="M15 14.5v5l4.2-2.5z" fill="currentColor"/>',
    eq: '<path d="M6 4v6M6 14v6M12 4v2M12 10v10M18 4v10M18 18v2" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="6" cy="12" r="2" stroke="currentColor" stroke-width="1.8" fill="none"/><circle cx="12" cy="8" r="2" stroke="currentColor" stroke-width="1.8" fill="none"/><circle cx="18" cy="16" r="2" stroke="currentColor" stroke-width="1.8" fill="none"/>',
    album: '<rect x="3.5" y="3.5" width="17" height="17" rx="3" stroke="currentColor" stroke-width="1.7" fill="none"/><circle cx="12" cy="12" r="3.5" stroke="currentColor" stroke-width="1.7" fill="none"/><circle cx="12" cy="12" r="0.8" fill="currentColor"/>',
    artist: '<circle cx="12" cy="8.5" r="4" stroke="currentColor" stroke-width="1.7" fill="none"/><path d="M4.5 20a7.5 7.5 0 0 1 15 0" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linecap="round"/>',
    song: '<path d="M9 18V5.5l11-2V16" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/><circle cx="6.5" cy="18" r="2.5" stroke="currentColor" stroke-width="1.7" fill="none"/><circle cx="17.5" cy="16" r="2.5" stroke="currentColor" stroke-width="1.7" fill="none"/>',
    clock: '<circle cx="12" cy="12" r="8.5" stroke="currentColor" stroke-width="1.7" fill="none"/><path d="M12 7.5V12l3 2" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linecap="round"/>',
    settings: '<circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.7" fill="none"/><path d="M12 2.8v2.4M12 18.8v2.4M21.2 12h-2.4M5.2 12H2.8M18.5 5.5l-1.7 1.7M7.2 16.8l-1.7 1.7M18.5 18.5l-1.7-1.7M7.2 7.2 5.5 5.5" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>',
    search: '<circle cx="10.5" cy="10.5" r="6.5" stroke="currentColor" stroke-width="1.8" fill="none"/><path d="m15.5 15.5 5 5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
    back: '<path d="M15 5l-7 7 7 7" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    down: '<path d="M5 9l7 7 7-7" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    plus: '<path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"/>',
    folder: '<path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2.2h7a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/>',
    trash: '<path d="M5 7h14M10 4h4M7 7l1 13h8l1-13M10 11v5M14 11v5" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    refresh: '<path d="M20 11a8 8 0 0 0-14.5-4.5M4 4v3.5h3.5M4 13a8 8 0 0 0 14.5 4.5M20 20v-3.5h-3.5" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    close: '<path d="M6 6l12 12M18 6 6 18" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
    minimize: '<path d="M6 12h12" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>',
    maximize: '<rect x="6.5" y="6.5" width="11" height="11" rx="1.5" stroke="currentColor" stroke-width="1.4" fill="none"/>',
    more: '<circle cx="5.5" cy="12" r="1.6" fill="currentColor"/><circle cx="12" cy="12" r="1.6" fill="currentColor"/><circle cx="18.5" cy="12" r="1.6" fill="currentColor"/>',
    lossless: '<path d="M2 12c1.5-4 3-4 4.5 0s3 4 4.5 0 3-4 4.5 0 3 4 4.5 0" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round"/><path d="M2 17h20" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" opacity=".55"/>',
    playNext: '<path d="M4 6h10M4 11h10M4 16h6" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="M17 9v8M13 13h8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
    expand: '<path d="M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    home: '<path d="M4 10.5 12 4l8 6.5V19a1 1 0 0 1-1 1h-4.5v-5.5h-5V20H5a1 1 0 0 1-1-1z" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/>',
    sparkle: '<path d="M12 3c.7 4.6 2.4 6.3 7 7-4.6.7-6.3 2.4-7 7-.7-4.6-2.4-6.3-7-7 4.6-.7 6.3-2.4 7-7Z" fill="currentColor"/>',
    headphones: '<path d="M4 15v-3a8 8 0 0 1 16 0v3" stroke="currentColor" stroke-width="1.7" fill="none"/><rect x="3" y="14" width="5" height="7" rx="1.5" stroke="currentColor" stroke-width="1.7" fill="none"/><rect x="16" y="14" width="5" height="7" rx="1.5" stroke="currentColor" stroke-width="1.7" fill="none"/>',
    speaker: '<rect x="6" y="3" width="12" height="18" rx="2" stroke="currentColor" stroke-width="1.7" fill="none"/><circle cx="12" cy="14.5" r="3" stroke="currentColor" stroke-width="1.7" fill="none"/><circle cx="12" cy="7.5" r="1.2" fill="currentColor"/>',
    file: '<path d="M7 3h7l4 4v14H7z" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/><path d="M14 3v4h4" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/>',
    gauge: '<path d="M4 17a8 8 0 1 1 16 0" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linecap="round"/><path d="m12 17 4-6" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>',
    radio: '<circle cx="12" cy="12" r="2" fill="currentColor"/><path d="M8.5 15.5a5 5 0 0 1 0-7M15.5 8.5a5 5 0 0 1 0 7M5.6 18.4a9 9 0 0 1 0-12.8M18.4 5.6a9 9 0 0 1 0 12.8" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linecap="round"/>',
    wave: '<path d="M3 12h2l2-5 3 10 3-14 3 12 2-3h3" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    download: '<path d="M12 4v11m0 0-4.5-4.5M12 15l4.5-4.5M5 19h14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    chevron: '<path d="M9 5l7 7-7 7" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/>',
    lyrics: '<path d="M5 5.5h14a1.5 1.5 0 0 1 1.5 1.5v8a1.5 1.5 0 0 1-1.5 1.5H10l-4.5 3.5V16.5H5A1.5 1.5 0 0 1 3.5 15V7A1.5 1.5 0 0 1 5 5.5Z" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/><path d="M8 9.5h8M8 12.5h5" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/>',
    playlist: '<path d="M4 6h11M4 10.5h11M4 15h6" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><path d="M17 17.5V8l3.5 1" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linecap="round" stroke-linejoin="round"/><circle cx="15.2" cy="17.6" r="1.9" stroke="currentColor" stroke-width="1.7" fill="none"/>',
    smart: '<path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/><circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.7" fill="none"/>',
    edit: '<path d="M4 20h4L19 9l-4-4L4 16z" stroke="currentColor" stroke-width="1.7" fill="none" stroke-linejoin="round"/>',
    mini: '<rect x="3" y="5" width="18" height="14" rx="2.5" stroke="currentColor" stroke-width="1.7" fill="none"/><rect x="11" y="11" width="7" height="5" rx="1" fill="currentColor"/>',
    note: '<path d="M9 18V6l10-2v12" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linejoin="round"/><circle cx="6.5" cy="18" r="2.5" fill="currentColor"/><circle cx="16.5" cy="16" r="2.5" fill="currentColor"/>',
  };
</script>

<svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">{@html paths[name] ?? ""}</svg>

<style>
  svg {
    display: block;
    flex: none;
  }
</style>
