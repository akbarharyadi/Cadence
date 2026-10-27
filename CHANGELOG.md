# Changes

Each release's section here becomes the "What's new" of its GitHub release.

## 0.2.20 (2026-10-27)

- **Opens much faster, especially on Android.** The home page now shows in well under a second instead of up to 7
  seconds. Cadence no longer waits while it starts watching your music folders (slow to list on a phone's shared
  storage), and it remembers each album's cover colours, and which albums have no cover, instead of working them out
  again on every start. The library scan still runs in the background, and new songs show once it's done.

## 0.2.19 (2026-10-26)

- **Android, USB exclusive.** With **Direct output** on, Cadence now drives a USB DAC with its own USB Audio driver on
  phones that don't offer Android's bit-perfect USB (most of them), the way USB Audio Player Pro and HiBy's exclusive
  mode do. Each song reaches the DAC untouched at its own sample rate, including CD-quality 44.1 kHz songs, which many
  phones always play at 48 kHz. Android asks once per plug-in whether Cadence may use the DAC. While Cadence drives it,
  other apps' sound comes from the phone's speaker, the volume keys set the DAC's volume (starting where Android's
  was), and unplugging the DAC pauses. It works with USB Audio 2.0 DACs (most current ones); older USB Audio 1.0 DACs
  play through Android as before.
- **Android, songs at their own rate.** Cadence hands Android each song at its own sample rate instead of converting
  everything to 48 kHz first, so phones that switch a USB DAC to the music's rate (many do for hi-res) play it without
  resampling.
- If the output keeps failing right after it opens, Cadence plays through the system mixer at its usual rate instead of
  retrying over and over, and says so.

## 0.2.18 (2026-10-26)

- **Android, Direct output to a USB DAC.** With **Direct output** on (Sound page), Cadence plays bit-perfect to a USB
  DAC: each song at its own sample rate, in the DAC's most precise format, with nothing mixed, resampled or
  volume-scaled by Android. It needs Android 14 or later and a phone whose maker enabled bit-perfect USB playback;
  otherwise Cadence plays through Android's mixer as before and says why. The phone's volume may not change the level
  while it's on: use Cadence's volume or the DAC's.
- The Sound page and Settings name the right system mixer on each platform (they said "Windows mixer" everywhere).

## 0.2.17 (2026-10-26)

- **Android, SD cards.** "Add a music folder" lists SD cards and USB drives again (newer Android versions hid them from
  Cadence), by the name Android gives them.
- **Android, Open with Cadence.** Songs in a file manager can be opened in Cadence: it plays them at once, in Now Playing.
  A song outside your music folders is added to the library on its own (not its whole folder); it shows in Settings
  → Music folders, where it can be removed.

## 0.2.16 (2026-10-25)

- **Karaoke stays with the singer on slow lines.** A held chorus (like "Maju 'tak gentar…" in Tetap Semangat) no longer
  fills in a few seconds while it's still being sung: the sweep runs until just before the next line, and only a much
  longer gap is taken as an instrumental break.

## 0.2.15 (2026-10-25)

- **Carries on where you left off.** Cadence remembers the queue, the song and the position; when you open it again
  (even after a crash or a restart) the song is ready where it was, paused. Press play to continue.

- **Karaoke sweep at the singer's pace.** Lines without word timing used to fill far too fast (about 0.16 s per
  character, so a slow Japanese line was done in 2 seconds); the sweep now carries the line until shortly before the next
  one, as singers do.
- **Lyrics timing nudge:** if a song's lyrics run early or late (made for another version), tap − or + next to
  **Timing** at the bottom of the lyrics; it's remembered for that song.
- **New music for you** no longer suggests songs you own under a wrong artist (the AI once credited milet's
  "Anytime Anywhere" to Aimer), and drops suggestions that neither Deezer nor iTunes can find.

## 0.2.14 (2026-10-24)

- **Smoother on phones.** Long song lists only build the rows on screen (Songs went from about 5,000 elements to 500 on a
  300-song phone, and far fewer freezes with thousands of songs), Search shows nothing until you type, and Android
  pages slide in with a lighter animation, so tab switches no longer freeze for 40 to 80 ms. The blurred cover behind
  Now Playing stays still on phones.
- The headphone profile buttons on the Sound page no longer cover their text on narrow screens; AutoEq is a link.

## 0.2.13 (2026-10-24)

- **Android updates work again.** Since the GitHub repository was renamed to "Cadence", the app refused its own download
  links; it now accepts them. Phones on 0.2.8 to 0.2.12 need to install this version once from the release page.
- The "new version" card now sits at the bottom right, above the player, instead of covering the sidebar's Sound and
  Settings buttons.

## 0.2.12 (2026-10-24)

- **Themes** in Settings → **Appearance**: **Album colours** (as before: colours from the album that's playing),
  **Dark** (Cadence's dark grey and violet, whatever plays) or **Black** (true black with neutral greys, kind to OLED
  phone screens). On Android, Black also blackens the strips behind the status and navigation bars.

## 0.2.11 (2026-10-24)

- **Karaoke for the pronunciation too:** the romaji, pinyin or romanization under the line being sung now fills in word by
  word along with it.

## 0.2.10 (2026-10-24)

- **Open-source licenses** in Settings → About: the license and notice of every library Cadence is built from, shipped
  with the app. About also credits the main projects and services Cadence uses.

## 0.2.9 (2026-10-24)

- **Lyrics in Latin letters.** Japanese, Chinese and Korean lyrics can show their pronunciation (romaji, pinyin or
  romanization) under each line: tap **Romaji** / **Pinyin** at the bottom of the lyrics.
- **Android, Bluetooth.** Cadence pauses when your car, headphones or speaker disconnects, instead of carrying on through
  the phone's speaker, and starts playing when they connect (Settings → **Play when Bluetooth connects**).
- **Android, music access.** If Cadence isn't allowed to read your music (for example after reinstalling), it asks for
  access instead of showing an empty library.
- **Library.** A folder that can't be read right now (no permission, a drive that isn't plugged in) keeps its songs,
  instead of the library forgetting them and their loudness measurements.

## 0.2.8 (2026-10-23)

- **Change Cover** for albums and songs: pick a cover from iTunes or Deezer, or an image file. It shows at once and is
  written into the songs' files. Find it in the album and song menus, by clicking an album's cover, and on the Now
  Playing cover.
- Now Playing has a **More** menu.
- The frosted top bar now reaches the window's corner.

## 0.2.7 (2026-10-22)

- **Skipped songs** in Settings is now a short summary; **Manage** opens a searchable list with covers, **Stop
  skipping** and **Stop skipping all**.

## 0.2.6 (2026-10-22)

- Windows: installing or updating puts Cadence back in the Start menu if its shortcut is missing.

## 0.2.5 (2026-10-22)

- **Artist pictures:** choose an artist's photo from the online catalogue, one of their album covers, or an image file.

## 0.2.4 (2026-10-22)

- Sound and Settings open even before you have any music.
- When a chosen folder has no music, Cadence says so and shows the folder, with **Choose another folder** and **Look
  again**.

## 0.2.3 (2026-10-21)

- **Skip songs:** leave songs (or every instrumental, karaoke and off-vocal version) out of albums, playlists, mixes and
  shuffles. From a song's menu or the ⊘ button in the player.
- **Fix Song Info:** for downloads named "Artist - Title", pick the right match from the iTunes catalogue; the title,
  artist, album, year and cover are written into the file.
- Right-clicking the player opens the song menu.
- The scrolled top bar is frosted again.

## 0.2.2 (2026-10-21)

- **Updates itself:** new versions download in the background and install when you restart (on Android: download, then
  Android's installer).
