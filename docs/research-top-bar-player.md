# Top-bar player: why it feels hard to use, and how to fix it

Date: 2026-11-06. This review covers `src/lib/components/PlayerBar.svelte` (the `placement === "top"` branch, added in 3be2a9f).

## What it is now

```
[repeat] [prev] (play) [next] | [art][ title / artist - album      ✦ ⊘ Lossless ] | ● Enhanced  💬 ☰ ▣ ⫶ 🔊━━━ | – □ ×
                              |  0:00 (hover)  ───────3 px line───────  -3:24 (hover) |
```

The layout copies the Apple Music "LCD" on macOS and Windows: buttons on the left, a small display in the middle, and tools on the right. Apple's LCD is also the part of Apple Music people complain about most, for the same reasons listed below.

## Problems, worst first

| # | Problem | Why it hurts |
|---|---------|--------------|
| 1 | **The times only show on hover** (`.lcd .time { opacity: 0 }`) | At a glance you can't tell how far into the song you are or how long it is. Spotify, YouTube Music, Tidal and the Windows Media Player all show both times all the time. |
| 2 | **The seek line is a 3 px line on the bottom edge of the display** | It's hard to see and hard to hit. You have to aim for a thin target on a box edge, and nothing shows it can be dragged until you hover it. |
| 3 | **There's no shuffle button in the player** | Shuffle and repeat are a standard pair. Here repeat sits alone where shuffle usually goes, so it reads like a mystery icon (it's also dim when off). |
| 4 | **"Skip this song from now on" sits right next to the title** | Its icon looks like "next". Clicking it bans the song from future plays, which is a lasting effect from a button that looks harmless. People will mix it up with Next. |
| 5 | **The right side is a row of 5–6 bare icons** (lyrics, queue, mini player, sound, volume) plus an "Enhanced" chip | Icons without labels are only clear when they're universal (play, volume). "Mini player" and "Sound" (the sliders icon) aren't. "Enhanced" is jargon, and it isn't obvious you can click it. |
| 6 | **When nothing is playing, the display is empty apart from the logo** | It doesn't say what to do. It looks broken, which is what happened in the bug fixed today. |
| 7 | **The cover opens Now Playing, but the title opens the album** | There are two different click targets in one small box, and the only hint is a small expand icon that appears on hover. |
| 8 | **The previous/next song lists appear on hover** | Lists that pop up while the pointer passes over the transport buttons feel jumpy, and they cover the page. |

Background principles:
- Fitts's law: small targets on edges are slow to hit.
- NN/g, "Icon Usability": icons need text labels unless they're universally known.
- Apple HIG and Fluent guidance: show the playback position all the time, and keep destructive actions away from the main controls.

## What the well-liked players do

- **Spotify desktop (bottom bar):** the song (cover, title, artist, ♥) on the left. In the centre, shuffle · prev · **play** · next · repeat, with a wide progress bar under them and **both times always visible**. Tools (lyrics, queue, devices, volume) on the right. Its controls are the ones people recognise most.
- **YouTube Music / Tidal:** the same three parts. The progress bar runs the full width along the bar's edge, but it's thick and gets thicker on hover, and the times are always shown.
- **Apple Music:** the LCD, as in Cadence. People like how it looks and complain about the hidden times and the hard-to-grab seek line.

The common pattern is: **the controls you use all the time are big and always visible, the progress and times are always visible, and the rarer tools are tucked away.**

## Recommendations

### Phase 1: small changes, biggest gain (keeps the top-bar layout)
1. **Always show the times** on both sides of the seek line, dimmed, and brighter on hover.
2. **Make the seek line thicker and move it inside the display.** Make it 4 px at rest and 6 px on hover, with the knob always visible while playing. Give it a hit area at least 16 px tall.
3. **Add a shuffle button** to the left of prev, giving `shuffle · prev · play · next · repeat`. When shuffle or repeat is on, show it in the accent colour with a dot under it, not as a dim/bright difference.
4. **Move "Skip this song from now on" out of the bar** and into the song's right-click menu and the ⋯ menu, with a toast that has an Undo button. Keep "Fix song info" in the menu too.
5. **Make the empty display useful.** Show "Nothing playing · Shuffle your library ▶" as a button.
6. **Add tooltips with keyboard shortcuts** to every icon (e.g. "Up Next (Ctrl+U)"), and use a clearer name for the "Enhanced" chip ("Sound: Enhanced").

### Phase 2: clean up the right side
7. Keep **lyrics, queue and volume** in the bar, and move **mini player** and **sound** into one ⋯ menu (or show them only on hover over the bar).
8. Add a **♥ / add to playlist** button next to the title. It's the most common song action in other players and it's missing here.
9. Make the whole display open Now Playing, with the title and artist as clear underlined links on hover. That way it's one big target plus explicit links.

### Phase 3 (optional): try a Spotify-style bottom bar
If testers still find the top bar hard after Phase 1, add a setting **Player position: Top / Bottom**. The bottom version would be song on the left, controls and a full-width progress bar in the centre, tools on the right, which is the layout most people already know. Phones already have it at the bottom.

## Status (2026-11-06): all three phases implemented
- Shuffle is a real engine mode (`Cmd::SetShuffle`, saved in settings). Turning it on shuffles the songs coming up and every list played later, with the chosen song first. Turning it off puts the remaining songs back in their original order.
- Cadence has no favourites yet, so the ♥ is a **+ Add to a playlist** button.
- The new shortcuts are Ctrl+S (shuffle), Ctrl+R (repeat), Ctrl+U (Up Next) and Ctrl+L (lyrics).
- The player position is in Settings > Appearance > Player position, and also in the player's ⋯ menu. It's stored in localStorage (`cadence-player-position`).

## Proposed Phase 1 top bar

```
[⤨][⏮](▶)[⏭][↻] | [art] Title                         ♥  Lossless | 💬 ☰ ⋯  🔊━━━ | – □ ×
                 |       Artist — Album                         |
                 | 1:12 ━━━━━━━━━━━●──────────────────── -2:12  |
```

## How to check it worked
Give 2–3 people (including your brother) five tasks without explaining anything:
1. "How long is this song?"
2. "Jump to the middle."
3. "Turn on shuffle."
4. "See what's playing next."
5. "Open the lyrics."

Time each task and note any mis-clicks. Do it before and after the changes.
