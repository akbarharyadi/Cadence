# Changes

Each release's section here becomes the "What's new" of its GitHub release.

## 0.3.2 (2026-11-24)

- **Up Next on the phone.** Now Playing has an Up Next button next to Lyrics. The queue opens full screen: tap a
  song to play it, tap × to take it out, and Back closes it.
- **Playlists are easier to find on the phone.** Listen Now has a Playlists button, and the Library tab now shows
  your playlists first.

## 0.3.1 (2026-11-18)

- **Louder with Sound Check.** Songs now play at the level of most streaming apps (−14 LUFS) instead of the
  quieter ReplayGain level, so Cadence no longer sounds much quieter than other apps at the same Windows volume.
  Songs you've already measured don't need measuring again.
- **Quiet songs keep up.** A quiet song with sharp peaks used to be held back to stay clear of clipping. Now it's
  raised like the others, and a gentle limiter turns down only the few peaks that would go too loud. It works only
  while Sound Check is on.
- **See the gain on this song.** The Sound Check options show how much the song playing now is turned up or down.
- **Lighter rescans.** When the music folders change, songs split by a CUE sheet and damaged files are no longer
  read again each time.
- **Mini player keeps moving** while the main window is minimized.
- **A cover added right after a scan is found** on Linux and on FAT drives.

## 0.3.0 (2026-11-15)

This is the biggest update so far: lighter on your computer, much more in the lyrics, new ways to find and pick
songs, backups, and a long list of fixes.

### What you'll notice first

- **Much lighter on your computer.** Cadence now uses far less processor while playing, and almost none while
  paused or hidden: the moving backgrounds stop when the music is paused or the window is out of sight. On one Windows PC,
  playing with Now Playing open went from about 41 % of one processor core to about 7 %, and paused from about
  34 % to about 0.6 %; memory use is about the same.
- **Lyrics in your language, furigana, and find a song by a line.** Show a translation under each line (English,
  Indonesian, Japanese, Korean or Chinese), put furigana over kanji and pinyin over Chinese, and search your
  library for a song by a line you remember.
- **Command palette and selecting several songs.** Press Ctrl+K (⌘K on a Mac) to go anywhere, run an action or find
  music. Select several songs in any list (Shift or Ctrl and click, or long-press on a phone) and play, queue,
  add them to a playlist or use the song menu on all of them together.
- **One artist, however it's spelled.** 米津玄師 and Kenshi Yonezu are one artist: one page, one count in Your
  Stats, and search finds them by either name, and by their reading in Latin letters ("tokyo" finds とうきょう).
- **Backup and restore.** One file with your playlists, favorites, ratings, history, queue, settings and picked
  lyrics, and Cadence keeps its own snapshot, at most once a day, when it opens and something has changed.
- **Choose your output device** (Windows, macOS and Linux), with its own equalizer, crossfeed, Direct output and
  volume for each one.
- **A shuffle that feels random.** Balanced shuffle spreads each artist out and plays songs you heard recently
  later. New too: shuffle albums, keep playing similar songs when the queue ends, and stop after any song.
- **A waveform seek bar** in Now Playing.
- **Playlists in and out.** Import M3U8, XSPF and CSV (including Spotify exports from Exportify) and export M3U8
  or CSV.
- **ListenBrainz does more.** Restore your listening history from it, send hearts as loved songs, and see new
  releases by your artists in Discover.

### Things that work differently now

- **Volume safety.** Cadence never opens a device louder than it last played there. The first time Direct output
  opens on a device it hasn't played on before, it starts at no more than 30 % on the volume slider, and tells
  you when it lowered the volume.
- **A device you chose stays chosen.** If it's unplugged, playback pauses instead of moving to the speakers, and
  carries on by itself if it comes back within two minutes (or tap "Play on" the default device).
- **Learning artist names online is on by default.** Cadence asks MusicBrainz, in the background, what else each
  artist in your library is called. Only artist names are sent. Turn it off in Settings › Learn artist names
  online.
- **New releases are on by default too.** When Discover opens, at most once a day, Cadence asks ListenBrainz for
  the week's new releases (nothing about you is sent) and shows those by your artists, with covers from the Cover
  Art Archive. Turn it off in Settings › Look for new releases online. It needs no account.
- **If you use ListenBrainz,** hearting or un-hearting a song now also tells it. Turn that off in Settings ›
  Scrobbling.
- **Shuffle is balanced** unless you choose "Fully random" (Sound › Shuffle and the end of the queue).
  **Keep playing similar songs** is off until you turn it on there.
- **The waveform seek bar is on**; turn it off in Settings › Appearance. The waveforms are drawn in the background
  and kept; you can prepare them for all songs there too.
- **Daily snapshots** are kept in the `backups` folder inside Cadence's data folder (the last 7 days and 4 weeks), and
  Settings can copy them to a second folder, such as one on another drive. Your music files, keys and passwords are
  not in them. Restoring first makes a snapshot of what you have now, so you can go back.

### Playing music

- **Balanced shuffle, album shuffle, autoplay, stop after any song.** "Shuffle albums" plays albums whole and in
  order, one after another, on Albums, an artist and a playlist. "Keep playing similar songs" adds ten songs that
  sound like the last ones, never the same artist twice in a row, and not what you played in the last hours. Put
  "Stop after" on any song in Up Next. Shuffle order and the songs autoplay added are remembered when you
  restart.
- **Songs from a one-file album (CUE sheet) start exactly where they should**, not a moment early.
- **No clicks** when you change the volume or the equalizer while a song plays.
- **Unmute returns to the volume you had**, not full volume.
- **Leveling keeps loud songs from clipping**: Sound Check now measures the true peak between samples and keeps it
  below -1 dB. Songs measured before get it when they are measured again.
- **Dither at 16 and 24 bit** when the sound has been changed (volume, equalizer, Sound Check, resampling) and the
  device takes whole steps. Music nothing changed still reaches the device bit for bit.
- **Resampling keeps the top treble** instead of rolling it off when a device runs at another rate.
- **Direct output on Windows asks Windows for pro-audio priority**, as audio software does, so other programs are
  less likely to disturb it.
- **Restoring a backup puts the queue back** as it was, paused where you left off.

### Your library

- **Multi-select in every song list** with the usual keys (Shift and Ctrl with click or arrows, Ctrl+A, Escape to
  clear, type a title's first letters to jump to it), and a bar with what you can do with the selection.
- **Import and export playlists.** Songs are matched by their file, then by title, artist and length, across
  spellings and scripts, and never guessed: you see what was found, what wasn't and what was ambiguous.
- **Playlists and the queue follow a song** when its file is moved, renamed, or replaced (an m4a swapped for a FLAC,
  a duplicate removed): they point at the copy you have now, and favorites and ratings come along.
- **Back and Forward** (Alt + arrows, or the mouse buttons) return you to where you were scrolled, and Forward
  works.
- **Phones get a Library tab** with Songs, Albums, Artists, Needs Attention, Library Health and your playlists.
- **Your Stats covers your whole listening history**, with more to see: streaks, "On this day", forgotten favorites,
  new artists, albums heard through, and your listening by decade and language.- **Settings has a search box and a list of keyboard shortcuts**; press ? anywhere for the same list.
- **Make the interface larger or smaller** on computers (Settings › Appearance, or Ctrl + = and Ctrl + -).
- **Artists under two names** in Needs Attention can be shown as one artist without changing any file, and Undo
  takes that back.

### Lyrics

- **Translations** under each line, in English, Indonesian, Japanese, Korean or Chinese. Translations that come with
  the lyrics are used as they are. Otherwise Cadence asks the AI you set up, once per song and language, and keeps
  the answer. Without the AI set up, you're told so.
- **Furigana** above kanji (Japanese) and **pinyin above** the characters (Chinese), kept in step with the karaoke
  sweep. Both are off until you turn them on in the lyrics view.
- **Find a song by a line.** Type a line you remember in Search: it finds songs by what's sung, as written or in Latin
  letters ("ame ga" finds a line written 雨が). Cadence reads your saved lyrics once, on your computer, in the
  background.
- **Word-by-word lyrics from LRCLIB** are used when it has them, so the highlight follows each word without
  following the singer first.
- **Better lyric matches online.** The search uses the artist and album too, and tries looser searches when the
  first finds little.
- **Lyric lines and the mini player keep your timing nudge.**

### Finding things

- **Command palette (Ctrl+K)** for pages, actions and your music, with the shortcut shown beside each action.
- **Search works across scripts**: an artist's other spellings and aliases, names and titles read in Latin letters
  (kana as romaji, Hangul romanized, Chinese as pinyin where the music is Chinese), and the lyrics line search above.
- **Press `/` or Ctrl+F to search**, from anywhere you're not typing.

### Sound and devices

- **Output device** on the Sound page: pick one, or follow the system default. A chosen device is kept even when
  another becomes the default (a monitor, a Bluetooth headset). Windows notices at once when one is plugged in or
  removed; on macOS and Linux use "Refresh the list". Android routes its own devices.
- **Remember sound settings per device** (on by default): each device keeps its equalizer, crossfeed, "Keep it full
  when quieter", Direct output and volume. A device used for the first time starts from your current settings.
- **Ask the DJ picks from your library** instead of inventing songs: the AI turns your request into a plan, Cadence
  finds the songs, and the AI (if you leave "DJ ordering" on) only orders and names them. A language your library
  has few songs in is said, not padded. It needs the AI set up.

### Safety of your data

- **Saves can't half-finish.** Settings, playlists, favorites, history and the rest are written to a temporary file
  and put in place in one step, with a copy of the last good version kept. A file Cadence can't read is left alone,
  not replaced by defaults.
- **History is never thrown away.** Beyond the last 20,000 plays, older ones now move to a yearly archive instead of
  being dropped, and Your Stats counts them.
- **Backups before risky changes.** A snapshot is made before every restore and every ListenBrainz import, and a
  restore that was cut short is undone the next time Cadence starts.
- **Updates tell you when they fail**, with what happened and a Try again button, instead of doing nothing.
- **Saving song info to several songs tries every file**, saves the ones it can, and lists the ones it couldn't.
- **Undo brings favorites and ratings back** with a duplicate you removed, and keeps your year and date.
- **ListenBrainz listens can't pile up forever:** ones it refuses as bad are let go, a sign-in it rejects is
  checked again before you're signed out, and the newest are kept for when you connect again.

### Smaller fixes

- Playlist edits (removing, dragging) hit the song you meant, even when another song in it is missing from your library.
- Escape closes only the top dialog, and Escape inside a dropdown closes just the dropdown.
- After you edit an album's title or artist, its page stays open instead of saying it's no longer in your library.
- Dialogs give the keyboard back to where it was when they close.
- On Android, "Delete copies" in Needs Attention asks first and ticks nothing for you, because phones delete for
  good. On a Mac, removed copies go to the Trash.
- Adding songs a playlist already has asks first.
- A dropdown inside a dialog is no longer cut off by the dialog's edge.
- The seek slider in Now Playing no longer freezes after a click.
- On Windows, a window hidden in the tray or minimized stops drawing and getting updates until you show it again.

## 0.2.40 (2026-11-10)

- **More varied "Made for you today".** The four daily mixes now each come from a different part of your
  library: **Right now** (what you usually play at this time of day), **On repeat** (this month's most
  played), **Rediscover** (old favourites you haven't played in two months), **Deep cuts** (songs you've barely
  played by artists you love) and **Somewhere new** (the part of your library you haven't visited lately).
  Four of the five appear each day, and the label is shown on each mix.
- **No more look-alike mixes.** A song is in one mix only, an artist leads one mix at most, a mix has at most
  3 songs per artist, and two mixes don't wear the same cover.
- **Fresh every day.** Songs from the last week's mixes wait their turn, and **New mixes** gives you different
  songs and ideas from the ones you just had.

## 0.2.39 (2026-11-09)

- **Better romaji.** Japanese lyrics are now read word by word in their sentence, with a Japanese dictionary
  built into Cadence: 君と座ってバス停見上げた is "kimi to suwatte basutei miageta" (it was "kun to suwatsu te
  basu tei miage ta"), and 君 after a name is still "-kun". Particles are written as they're said (wa, o, e),
  verb endings stay with their verb ("ikunda", "shiteru"), and English inside a line is left as it is.
- **Follow the singer reads kanji the same way**, so lyrics are placed on the singing more accurately in
  Japanese songs.
- The app is bigger because of the dictionary: about 7 MB more to download, about 45 MB more on disk.

## 0.2.38 (2026-11-09)

- **Needs Attention, rebuilt.** One card per kind of problem, worst first: damaged files, extra copies, albums
  split in two, artists under two names, songs without info, then the optional ones (lossless files that aren't,
  albums without a cover, a year or a genre, and albums with songs missing). Click a card to see its list. The
  sidebar number only counts what's wrong, not what's missing, and the optional kinds can be hidden.
- **Sure or Check.** Every suggestion says how sure Cadence is. Sure ones start ticked; Check ones don't, so you
  look first (a copy of another length, or a song found by its title only).
- **Downloads you already have.** A file without tags, like "りりあ。 - Kiminotonaride" from YouTube, is now
  recognised as a copy of the song you have tagged (Riria.'s FLAC), across Japanese and Latin letters.
- **Removed copies go to the Recycle Bin**, and Undo brings them back. The favourite and rating move to the copy
  that stays. (Library Health used to delete them for good.)
- **Covers for all your albums without one**: Cadence looks them up in the background, then shows them in a grid
  for you to check before anything is saved. Undo puts the old files back.
- **Ignore** anything that's the way you want it; it stays out of the list (and can be shown again).
- Artist names in Japanese or Chinese letters are checked by themselves after each scan, and "Compilations - Song"
  file names are no longer read as an artist called "Compilations".
- Keys on Needs Attention: ↑ ↓ to move, Enter to fix, Space to tick, I to ignore.
- **Discover no longer suggests songs you have under their Japanese names** ("Kizuna no Kiseki" when you have
  絆ノ奇跡).

## 0.2.37 (2026-11-08)

- **Song info in Latin letters.** When Fix song info finds a song named in Japanese, Chinese or Korean letters,
  the new **Aa Latin** switch shows and saves its names in Latin letters instead: Apple's own where it has them
  (「あんたなんて。」 by りりあ。 is "Antanante." by "Riria."), otherwise written out by Cadence. The other songs of
  the album follow, the original title is kept as the subtitle so searching finds the song by either name, and
  your choice is remembered.
- **"Both" no longer marks lines one person sings.** In duets whose two voices are close in pitch (a man and a
  woman singing in a similar range), nearly every line could be marked as sung together. Now that only happens
  when the voices are clearly apart; otherwise each line goes to the voice it sounds closer to. Songs listened to
  before are fixed the next time their lyrics open.

## 0.2.36 (2026-11-08)

- **Much lighter while music downloads.** If a downloader (aria2, Soulseek, a browser) saves into your music
  folder, Cadence used to read the whole library again every time a file was written, which could be every minute
  or every few seconds, keeping a processor core busy and making pages slow. Now songs still downloading are left
  out until they're complete, Cadence looks again at most every 15 seconds, and pages don't reload when nothing
  changed. On a 7,000-song library: from more than a full processor core to about a sixth, and a library check
  from 7 seconds to a third of a second.
- **Damaged files don't slow everything down.** A FLAC file whose download never finished could take 8 seconds
  to read on every check; Cadence now spots it in a fraction of a millisecond and skips its cover.
- **Albums and Recently Added open faster**: the first albums appear at once (about 5× quicker) and the rest are
  added as you look.
- **Clearer message when lyrics can't follow the singer** because they belong to another version (an acoustic or
  live one sings other lines): "Lyrics don't match this version" opens the lyrics search, instead of offering to
  try again.

## 0.2.35 (2026-11-07)

- **Windows: if Microsoft Defender removed Cadence after the 0.2.34 update, install this version.** Defender
  wrongly flagged 0.2.34's program as a threat (an automatic guess, "Bearfoos.A!ml", about a new app that isn't
  signed yet). Nothing harmful was in it: it was checked against the source, and the same code built again is
  clean. It has been reported to Microsoft. From this version, every Windows build is scanned with Microsoft
  Defender before it's published. Your music and Cadence's library were never touched.
- **The Android app is back.** 0.2.34 came out for Windows, macOS and Linux only; this brings its changes to
  Android.
- **Needs Attention reacts at once.** "Use …" on an album split in two, and renaming an artist, show the change
  right away (in a big library it used to wait for the whole library to be read again). Long names no longer
  squeeze the album's name: the buttons go onto their own line.

## 0.2.34 (2026-11-07)

- **Fix song info finds much more.** It now searches Deezer as well as iTunes, so songs Apple doesn't list (many
  game soundtracks and Asian singles) are found. A song tagged in Chinese, Japanese or Korean is found even when
  the catalogues only know its English name (悠忽舞于梦中 is "Dancing Through Fantasies"): by its artists and its
  length, with no typing. Each result says where it's from.
  - **Keep my title**: take the album, year and cover but keep your title; the catalogue's is kept as the song's
    subtitle, so searching your library finds it by either name.
  - **The whole album at once**: the other songs of the album (in its folder) get the same album, year, genre and
    cover, each matched to its own song (伴奏, カラオケ and 반주 are known as instrumentals), so the album never
    splits in two.
  - The cover you pick now replaces the file's own.
- **Fix Album Info** (album menu): find the album, check which song each file is (any can be changed), and save the
  album's info, cover and every song's title, artist and number in one go. The album whose songs are as long as
  yours comes first.
- **Needs Attention** (sidebar, with a count of what to fix): albums split in two with a one-click fix, songs
  without info (**Find info for all** looks them up in the background, then shows what it found for you to check
  before anything is saved), albums without a cover, and **the same artist under two names** (米津玄師 and Kenshi
  Yonezu): Cadence asks the catalogue which names belong together, and you pick the one to keep.
- **Undo.** Every change Cadence makes to your songs' files (Fix song info, Fix Album Info, renaming an artist,
  merging an album, editing info by hand, and info it filled in by itself) can be undone: from the message that
  appears, or later from **Recent changes** in Needs Attention. Undo puts the files back as they were, cover included.
- **Now Playing, easier to read and use.** The background darkens as much as each cover needs, so the words stay
  readable on white and pastel covers too, and upcoming lyrics are brighter. Everything fits the window, short or
  narrow: nothing is cut off at the bottom any more. The lyrics have fewer buttons (Sing along, Latin letters, and ⋯
  for the rest), the sleep timer moved into ⋯, and with "reduce motion" on in your system, nothing drifts.
- **Favorites and ratings.** Heart a song and give it 1 to 5 stars; they follow the song if its file moves. Smart
  playlists can use them, and the format, sample rate, bit depth and bitrate.
- **Sleep timer**: stop after some minutes (with a fade-out), at the end of the song or of the album.
- **Up Next tools**: clear it, save it as a playlist, drag songs to reorder, undo.
- **Hotkeys that work in any app** on computers: Ctrl+Alt+Space plays or pauses, Ctrl+Alt+← / → skip, Ctrl+Alt+↑ / ↓
  change the volume and Ctrl+Alt+H hearts the song. Turn them off in Settings.
- **Library Health** (sidebar): songs you have twice, lossless files made from lossy or CD-quality sources, and
  damaged files (FLAC checksum). The file checks run in the background, each file once.
- **CUE sheets**: albums ripped as one big file with a .cue sheet show up as their songs.

## 0.2.33 (2026-11-07)

- **No more "500 Internal Error".** On Android, turning the screen off and on again could leave Cadence on a blank
  "500 / Internal Error" page. Now Cadence loads itself again right away (the music keeps playing), and an error in
  one page, Now Playing or Up Next stays in that part, with a "Try again" button, while the rest keeps working.
- **Report a problem in one tap.** After an error, "Report" opens a new GitHub issue already filled in with the
  error, your Cadence version and device, and the last steps before it (pages opened, buttons pressed, the screen
  turning off and on). Read it, add what you were doing, and press Submit; nothing is sent without you.

## 0.2.32 (2026-11-07)

- **Singers: no more made-up second voice.** Since 0.2.30, songs with one singer could show their lines as a
  "Lower voice" and a "Higher voice": a singer moving between low verses and a high chorus sounded like two people.
  Now Cadence only tells voices apart in songs known to have several singers: the AI DJ found two or more credited
  vocalists, or the title or artist says so ("feat.", "ft.", "with", "×"). Singers named in the lyrics are shown as
  before. Songs followed before are only listened to again when they have several singers.
- **Easier to read.** Lines not being sung are white again, with only a hint of their singer's colour; the full
  colour is on the singer's name and the line being sung, so lyrics stay readable on every cover's colours.

## 0.2.31 (2026-11-07)

- **The Android app is back.** 0.2.30 came out for Windows, macOS and Linux only; this release brings its changes to
  Android too. On a phone, singers named in the lyrics are shown in their colours (telling voices apart needs
  Follow singer, which runs on computers).

## 0.2.30 (2026-11-07)

- **Who sings each line.** In songs with two singers, each singer's lines now have their own colour, lines sung
  together have a third, and a small name shows where the singer changes. Turn it off with **⋯ → Hide singers**.
  - Names written in the lyrics are used first, like "Name: …", "[Name]", or karaoke's M / F / D.
  - Otherwise, while **Follow singer** listens to a song, Cadence also tells the voices apart by their pitch and
    tone. It only splits songs with two clearly different voices, and lines it isn't sure about stay unmarked. Songs
    it has followed before are listened to once more, without the slow part (about 40 seconds with a graphics card),
    the next time you open their lyrics.
  - **⋯ → Who sings? (AI)** asks the AI DJ to find the singers' names. It only uses names it can show a credit for,
    so a band member who only plays an instrument isn't named as a singer; without a credit the voices are shown as
    "Lower voice" and "Higher voice". When a lyrics site marks each singer's part, as K-pop color-coded lyrics do,
    those parts are used line by line.

## 0.2.29 (2026-11-07)

- **Fix song info finds the right songs.** When the catalogue doesn't have the song, the list used to fill up with
  unrelated songs that just happened to be about as long. It now only shows songs that share most of what you searched
  for, the closest first, and says so when none of them is clearly your song (demos and unreleased songs often aren't
  in the iTunes catalogue), pointing you to **Edit by hand**. Typing "Artist - Title" in the search box works too.

## 0.2.28 (2026-11-06)

- **An easier player.** The song's time and time left now always show, either side of a thicker seek line that's
  easier to grab. Click anywhere on the song display to open Now Playing; the title, artist and album are still links.
  **+** adds the song to a playlist, and **⋯** opens its menu. "Skip this song from now on" has moved into that menu,
  away from Next, and its message has an **Undo** button. When nothing is playing, the display offers **Shuffle your
  library**.
- **Shuffle.** A new Shuffle button sits beside the play buttons, with Repeat on the other side; both light up when
  on. Shuffle plays the songs coming up in a random order, and keeps doing that for anything you play next, always
  starting with the song you picked. Turn it off and the songs go back in their order. It's in Now Playing too.
- **Player at the top or the bottom.** Settings → Appearance → **Player position**: keep the player as the window's
  top bar, or put it in a bar along the bottom of the window, with the song on the left and the buttons in the
  middle. You can also switch it from the player's **⋯** menu, which now holds the mini player and sound settings.
- **Keyboard shortcuts:** Ctrl+S shuffle, Ctrl+R repeat, Ctrl+U Up Next, Ctrl+L lyrics. The buttons' tooltips show
  their shortcuts.
- **The queue follows songs that move.** When a song's file is moved, renamed or replaced by another copy (for
  example, when duplicates are cleaned up), the songs waiting in Up Next now play from the new file instead of
  failing. The player also keeps showing the song that's playing instead of going blank.

## 0.2.27 (2026-11-04)

- **The player is the window's top bar.** On computers, the player no longer floats over the bottom of the page: it
  sits at the top of the window, with the controls on the left and the song in the middle like a small display, its
  seek line along the bottom edge and the album's colours behind it. Lists and pages now run to the bottom edge.
  Phones keep the player at the bottom, above the tab bar.
- **Discover knows the songs you have under other names.** Japanese songs are often tagged in Latin letters
  ("Kenshi Yonezu – KICK BACK") while the AI DJ names them in Japanese (米津玄師), so Discover could suggest a song you
  already had. It now checks both names, for the song, the artist and the album.
- **Songs you get drop out of Discover at once.** When a song from Discover or **New music for you** arrives in your
  library, it disappears from the picks right away, instead of staying until the next day's picks.

## 0.2.26 (2026-11-03)

- **Discover new music.** A new **Discover** page finds songs you don't have yet, searched for on the web by the AI
  DJ. Base it on what you've played in the last 30 days, an artist, a genre, or anything you describe in words. Each
  song shows how close it is to what you asked for, how well listeners and critics rate it, which of your songs it's
  like, why it was picked, and where to buy it (Qobuz, Bandcamp, OTOTOY, mora, iTunes or Amazon). Songs you already
  have, or that are on an album you have, are never suggested, and a song comes back only if it's very well rated.
- **New picks every day.** Discover's picks for this month's listening are made once a day and kept, so the page (and
  **New music for you** on Listen Now) opens at once. **New picks** asks for fresh ones.
- **Claude as the AI DJ.** Settings → AI DJ can now use Claude (Anthropic), which also searches the web for new
  releases. Paste a Claude API key and Cadence switches to it; your other service's key is kept.
- **Your Stats.** A new page with what you play most: songs, artists and albums, counting only songs heard to the
  end, for the last week, month, year or all time, with your listening time and a chart of when you listened.
- **Following the singer is faster on Windows.** It runs on the graphics card (any DirectX 12 GPU): well under a
  minute per song instead of 2–3 minutes. Moving between pages stays quick while it works.
- **Lighter while playing on phones.** The seek line and the little "now playing" bars no longer redraw the screen
  60 times a second; Cadence uses a fraction of the processor it did while a song plays.
- **Android: the music doesn't stop if the screen's drawing process ends.** When Android stops it (low memory, a
  graphics driver problem), Cadence rebuilds the screen and keeps playing, instead of closing.
- **Fixes.** On very wide screens, Listen Now's quick picks fill the width. Light-grey text is easier to read. Counts
  say "1 song", not "1 songs". On Android, the status bar icons stay readable when the phone is in light mode. On
  Linux, Cadence no longer closes at startup on a system without security certificates.

## 0.2.25 (2026-11-02)

- **Lyrics follow the singer.** Press **Follow singer** under the lyrics. Cadence listens to the song once,
  separates the singing from the music, and places every word of the lyrics on the singing. The karaoke highlight
  then moves with the singer, syllable by syllable for Japanese, Chinese and Korean and word by word for other
  languages, instead of being spread evenly over the line. It also notices when a song's lyrics run early or late
  (made for another version) and sets that right. It works for Japanese, Chinese, Korean, English, Indonesian and
  most other languages. Everything runs on your computer, in the background, at low priority. The first time, it
  needs a one-time download of about 440 MB (ONNX Runtime, a vocal separator and Meta's MMS aligner). Each song is
  listened to only once: the result is kept, and corrected lyrics are placed again in under a second. Click
  **Follow singer** again to see the lyrics' own timing. Desktop only (Windows, Linux, and Macs with Apple silicon).
- **For your whole library.** Settings → **Lyrics follow the singer** → **Follow the singer in all songs with
  lyrics** does every song that has lyrics, most-played first. The sidebar and Settings show how far it has got and
  roughly how long is left. You can stop it any time, and if you close Cadence it carries on the next time it starts.
  With **Follow the singer automatically** on, songs whose lyrics you open (and the next song in the queue) go first.
- **Tidier lyrics toolbar.** Everything under the lyrics sits on one row again. **Search for other lyrics**,
  **Paste lyrics** and **Hide these lyrics** are in the **⋯** menu.

## 0.2.24 (2026-11-01)

- **Edit the info of the song that's playing.** **Edit Info…** used to refuse the playing song ("This song is
  playing. Play something else…"). Now **Save** works: the new title, artist, album and so on show at once in your
  library and the player, and Cadence writes them into the file when the song finishes (a file can't be rewritten
  while it's being played). If you close Cadence before the song ends, they're written the next time it starts.
- **Up to date and patched.** Everything Cadence is built from was updated, including fixes for known security issues
  in two parts used for downloading and installing updates.

## 0.2.23 (2026-10-31)

- **Much lighter window while playing.** Cadence's window could grow past 1 GB of memory within minutes of playing,
  and kept part of a processor core busy. The little "playing" bars and the seek line were redrawn on every frame of
  the screen (up to 160 times a second), and the whole queue was sent to the window four times a second. They now
  move on the graphics card, the queue is only sent when it changes, and the karaoke sweep updates 30 times a
  second while the lyrics are open. The window now stays around 100 MB and barely uses the processor.

## 0.2.22 (2026-10-28)

- **Much lighter in the background.** Cadence listened to every song in your library again each time it started
  (one silent song made its saved results unreadable), using most of the processor, around 1 GB of memory and a lot of
  disk for several minutes. Results are now kept, and listening to songs and measuring loudness run on at most two
  low-priority threads that give way to everything else (on Windows 11 they also use the power-saving cores), with
  little memory. It takes longer the first time, but you won't notice it.
- **You can see it.** The sidebar shows **Listening to songs** with its progress, as it does for importing and
  measuring loudness.
- If one of Cadence's saved files (history, playlists, settings…) can't be read, it's kept as `*.broken.json` instead
  of being replaced by an empty one.
- Smart EQ stays flat for songs with no sound in some bands (very low sample rates), instead of going wrong.

## 0.2.21 (2026-10-27)

- **Windows, updates.** Pressing **Restart** to update could stop with "Failed to kill Cadence. Please close it first
  then try again" when Cadence took a moment to close (for example while it was playing and analysing songs). The
  installer now waits for Cadence to close before it carries on.

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
