//! The playback engine.
//!
//! One worker thread owns the decoder, DSP and the play queue, and keeps a lock-free ring buffer
//! topped up. The device side ([`output::Renderer`]) only copies samples out of that ring, so decoding
//! never blocks audio. Songs follow each other in the same stream: gapless.
//!
//! Signal chain (worker): decode → channel mapping → loudness gain → per-song tone → crossfade blend
//! → vocal cut → EQ → crossfeed → resample → ring. The renderer then applies volume and the sleep
//! timer's fade (both gliding), and dither where the device takes integers.

pub mod crossfeed;
pub mod devices;
pub mod eq;
#[cfg(windows)]
mod exclusive;
#[cfg(target_os = "android")]
mod android_direct;
#[cfg(target_os = "android")]
mod usb_dac;
#[cfg(any(target_os = "android", test))]
mod usb_audio;
mod pcm;
pub mod loudness;
pub mod envelope;
mod output;
mod resample;
mod source;
mod flacfix;
pub mod tone;
pub mod karaoke;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use ringbuf::traits::{Observer, Producer};
use serde::{Deserialize, Serialize};

use crossfeed::{Crossfeed, CrossfeedSettings};
pub use devices::DeviceRef;
use eq::{EqSettings, Equalizer};
use loudness::{Leveling, SongGain};
use output::{Output, Shared};
pub(crate) use resample::Resampler;
pub use source::Source;

/// Opens a file for decoding (used by examples and diagnostics).
pub fn open_source(path: &std::path::Path) -> Result<Source> {
    Source::open(path)
}

pub use loudness::measure_album;

/// Output level (linear) above which the Windows mixer's limiter starts working.
const CLIP_THRESHOLD: f32 = 0.985;

#[derive(Clone, Debug, Default)]
pub struct QueueItem {
    pub id: String,
    pub path: PathBuf,
    pub duration: f64,
    pub album_id: String,
    pub gain: SongGain,
    /// Smart EQ curve (dB per band) from the sound analysis, if measured.
    pub tone: Option<[f32; 10]>,
    /// A song inside a longer file (CUE sheet): seconds into the file where it starts, and where it
    /// ends (None: the end of the file). Positions and `duration` are the song's own.
    pub start: f64,
    pub end: Option<f64>,
    /// Who the song is by, as one name per artist (`shuffle::artist_key`): the balanced shuffle
    /// spreads each artist's songs out.
    pub artist: String,
    /// When the song last played (unix seconds, 0: not recently): shuffle plays fresher songs first.
    pub last_played: u64,
    /// Added by Autoplay when the queue ran out (listed under "Autoplay" in Up Next).
    pub auto: bool,
}

impl QueueItem {
    fn is_cue(&self) -> bool {
        self.start > 0.0 || self.end.is_some()
    }

    /// The frame of the file (at `rate`) where this song starts.
    fn first_frame(&self, rate: u32) -> u64 {
        cue_frame(self.start, rate)
    }

    /// How many frames long the song is, when it ends before the end of its file (CUE sheet).
    fn cue_frames(&self, rate: u32) -> Option<u64> {
        self.end.map(|e| cue_frame(e, rate).saturating_sub(self.first_frame(rate)))
    }

    /// This song carries straight on from `prev` in the same file (the next track of a CUE sheet).
    fn continues(&self, prev: &QueueItem, rate: u32) -> bool {
        self.path == prev.path
            && prev.end.is_some_and(|e| cue_frame(e, rate) == self.first_frame(rate))
    }
}

/// A position in seconds as a frame at `rate`. CUE sheets count in CD frames (1/75 s), which are a
/// whole number of samples at the usual rates (588 at 44.1 kHz), so a position that is a whole
/// number of CD frames lands exactly on its sample (truncating float seconds could land one early).
fn cue_frame(secs: f64, rate: u32) -> u64 {
    let secs = secs.max(0.0);
    let cd = (secs * 75.0).round();
    if rate.is_multiple_of(75) && (secs * 75.0 - cd).abs() < 1e-6 {
        cd as u64 * u64::from(rate / 75)
    } else {
        (secs * f64::from(rate)).round() as u64
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

/// Everything about how audio is processed, persisted in settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DspSettings {
    pub eq: EqSettings,
    pub leveling: Leveling,
    pub crossfeed: CrossfeedSettings,
    /// WASAPI exclusive mode: bypass the Windows mixer and play at each song's own rate.
    pub exclusive: bool,
    /// Exclusive mode at one fixed rate (Hz) instead of each song's own; 0 = match each song.
    /// Some devices click when they change rate; a fixed rate avoids that.
    pub exclusive_rate: u32,
    pub crossfade: CrossfadeSettings,
    /// Loudness compensation and Smart EQ (per song).
    pub tone: tone::ToneSettings,
    /// Sing along: how far the lead vocal is turned down (0 = not at all, 1 = fully).
    pub vocal_cut: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrossfadeSettings {
    pub enabled: bool,
    /// Length of the blend, in seconds.
    pub seconds: f32,
}

impl Default for CrossfadeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            seconds: 6.0,
        }
    }
}

/// The incoming song during a crossfade: decoded alongside the outgoing one and blended in.
struct Crossfade {
    src: Source,
    index: usize,
    /// Linear loudness gain for the incoming song.
    gain: f32,
    /// The incoming song's own per-song tone, applied to it before the blend; it becomes the
    /// worker's tone EQ (filter memory and all) when the song takes over.
    tone: Equalizer,
    rate: u32,
    /// Incoming audio already mapped to the output channels and gain-adjusted, not yet mixed.
    buf: Vec<f32>,
    /// Frames of the incoming song decoded so far.
    frames: u64,
    /// Progress through the blend, in frames.
    pos: u64,
    len: u64,
    scratch: Vec<f32>,
}

impl Crossfade {
    /// Mixes the incoming song into `out` (the outgoing song's chunk) with equal-power curves.
    fn mix_into(&mut self, out: &mut [f32], ch: usize) {
        while self.buf.len() < out.len() {
            match self.src.next_chunk() {
                Ok(Some((samples, _rate, src_ch))) => {
                    self.frames += (samples.len() / src_ch.max(1)) as u64;
                    map_channels(samples, src_ch, ch, &mut self.scratch);
                    if self.gain != 1.0 {
                        let g = self.gain;
                        self.scratch.iter_mut().for_each(|x| *x *= g);
                    }
                    self.tone.set_format(ch, self.rate);
                    self.tone.process(&mut self.scratch);
                    self.buf.extend_from_slice(&self.scratch);
                }
                _ => break,
            }
        }
        let frames = out.len() / ch;
        for f in 0..frames {
            let t = ((self.pos + f as u64) as f32 / self.len.max(1) as f32).min(1.0);
            let (fade_out, fade_in) = (
                (t * std::f32::consts::FRAC_PI_2).cos(),
                (t * std::f32::consts::FRAC_PI_2).sin(),
            );
            for c in 0..ch {
                let i = f * ch + c;
                out[i] = out[i] * fade_out + self.buf.get(i).copied().unwrap_or(0.0) * fade_in;
            }
        }
        let used = out.len().min(self.buf.len());
        self.buf.drain(..used);
        self.pos += frames as u64;
    }
}

pub enum Cmd {
    Load {
        items: Vec<QueueItem>,
        index: usize,
    },
    /// The queue from the last session, paused at the song and position it was left at.
    /// `unshuffled`: the order shuffle had put aside (see `Worker::unshuffled`), to put back when
    /// it's turned off; `autoplay_from`: where the songs Autoplay added begin.
    Restore {
        items: Vec<QueueItem>,
        index: usize,
        position: f64,
        unshuffled: Vec<String>,
        autoplay_from: Option<usize>,
    },
    /// Plays a whole list in a shuffled order (the balanced or fully random one, as set), from its
    /// first song. With shuffle on, turning it off puts the list's own order back.
    LoadShuffled(Vec<QueueItem>),
    /// Plays `items` (one album, in order, whether shuffle is on or not). `more`: album shuffle,
    /// asking for the next album when the last one queued starts (`Status::more`).
    PlayAlbums {
        items: Vec<QueueItem>,
        more: bool,
    },
    /// Songs for the end of the queue, asked for by `Status::more` (the next album of an album
    /// shuffle, or Autoplay's songs: `auto`).
    Extend {
        items: Vec<QueueItem>,
        auto: bool,
    },
    /// "Stop after this song": pauses once the song at this queue place has played (None: no
    /// longer). One at a time; it follows the song when the queue changes around it.
    StopAt(Option<usize>),
    SetShuffleMode(crate::shuffle::ShuffleMode),
    /// When the queue ends: keep playing songs that fit (`Status::more`), or stop.
    SetAutoplay(bool),
    /// Takes the songs Autoplay added out of the queue.
    ClearAutoplay,
    Play,
    Pause,
    Toggle,
    Next,
    Prev,
    Seek(f64),
    /// Seek relative to the current position, in seconds.
    SeekBy(f64),
    Jump(usize),
    Enqueue {
        items: Vec<QueueItem>,
        next: bool,
    },
    Remove(usize),
    /// Puts songs into the queue at this place (never before the song that's playing): undoing a
    /// removal.
    Insert {
        at: usize,
        items: Vec<QueueItem>,
    },
    /// Takes everything after the playing song out of the queue.
    ClearUpNext,
    /// Moves a song that's waiting in the queue to another place among the waiting ones.
    Move {
        from: usize,
        to: usize,
    },
    /// Pauses by itself at a song boundary (the sleep timer's "end of this song / album").
    StopAfter(Option<StopAfter>),
    /// New gain measurements for songs (from the background loudness scan).
    UpdateGains(Vec<(String, SongGain)>),
    SetVolume(f32),
    SetDsp(DspSettings),
    SetRepeat(Repeat),
    /// Shuffle the songs after the current one (and every list played from now on); off puts
    /// them back in their order.
    SetShuffle(bool),
    /// Songs whose files moved (found again by a rescan): the queue entry with the old id gets
    /// the new one, in place.
    Replace(Vec<(String, QueueItem)>),
    /// The output device to play on (None: the system default, followed when it changes). Playing
    /// moves there at once, from the same place.
    SetDevice(Option<DeviceRef>),
    /// The chosen device isn't connected: play on the system default until it's back.
    UseDefaultForNow,
    /// The device list may have changed (the Sound page listed it; where the system doesn't tell
    /// Cadence, this is how a device that's back is found).
    CheckDevices,
    /// Sound settings per output device (None: one set for all, as before).
    SetProfiles(Option<ProfileSource>),
}

/// Asked when playback moves to another device: the sound settings and volume to use there (None:
/// keep the ones in use). Runs on the engine's worker thread, never in the audio callback.
pub type ProfileSource = Box<dyn FnMut(&DeviceRef) -> Option<(DspSettings, f32)> + Send>;

/// When playback should pause by itself.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum StopAfter {
    /// After this many more songs have finished (1 = the end of the song that's playing).
    Songs { left: u32 },
    /// When the next song is from another album than the one playing.
    Album,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub bits: Option<u32>,
    pub channels: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    pub device: String,
    pub sample_rate: u32,
    pub channels: usize,
    /// "shared" (through the Windows mixer) or "exclusive".
    pub mode: String,
    /// The song's rate differs from the device's, so it's being converted.
    pub resampling: bool,
    pub eq: bool,
    /// Automatic EQ level reduction in dB (≤ 0).
    pub eq_headroom: f64,
    pub crossfeed: bool,
    /// Loudness gain applied to the current song, in dB (None = leveling off).
    pub leveling: Option<f32>,
    /// Per-song tone on the current song.
    pub tone: Option<ToneInfo>,
    /// Channels are being down- or up-mixed.
    pub remixing: bool,
    /// Cadence passes the samples through unchanged (the Windows mixer may still process them in shared mode).
    pub unaltered: bool,
    /// Unaltered by Cadence *and* sent straight to the device (exclusive mode).
    pub bit_perfect: bool,
    /// Integer bits sent to the device (exclusive mode).
    pub bits: Option<u32>,
    /// The DAC's own volume Cadence applies, in dB, when it's below full (Android's USB exclusive:
    /// Android's volume doesn't reach the DAC, so the volume keys move this).
    pub device_volume_db: Option<f32>,
    /// Something worth telling the user about the output (e.g. exclusive mode fell back to shared).
    pub note: Option<String>,
    /// Peak output level since the last update, in dBFS.
    pub peak_db: f32,
    /// The output reached full scale in the last few seconds.
    pub clipping: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub queue: Vec<String>,
    pub index: Option<usize>,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f32,
    pub repeat: Repeat,
    pub shuffle: bool,
    pub source: Option<SourceInfo>,
    pub output: OutputInfo,
    pub error: Option<String>,
    /// The sleep timer's song or album rule, while one is set.
    pub stop_after: Option<StopAfter>,
    /// "Stop after this song": the queue place it's on.
    pub stop_mark: Option<usize>,
    /// Where the songs Autoplay added begin (after the playing song), if there are any.
    pub autoplay_from: Option<usize>,
    /// An album shuffle is playing (albums are added one at a time).
    pub album_shuffle: bool,
    /// The queue is about to run out and should be extended (see `More`).
    pub more: Option<More>,
    /// The output device in use (or last used, while it's let go): its sound settings are in use.
    pub device: Option<DeviceStatus>,
    /// The chosen output device isn't connected: playback is paused until it's back.
    pub device_missing: Option<DeviceMissing>,
    /// Goes up when the system's output devices changed (the device list is out of date).
    pub devices: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub id: String,
    pub name: String,
    /// The device chosen in Cadence (false: the system default).
    pub chosen: bool,
    /// Goes up each time a device's own sound settings were put in use (the window loads them).
    pub profile: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceMissing {
    pub name: String,
    /// The system default's name, to offer playing there instead.
    pub fallback: Option<String>,
    /// Playback carries on by itself if it's back soon (`devices::RESUME_WINDOW`).
    pub resume: bool,
}

/// What the end of the queue is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum More {
    /// The next album of an album shuffle (the playing song's album is the last one queued).
    Albums,
    /// Autoplay: songs like the last ones (the queue's last song, or the one before it, plays).
    Similar,
}

pub struct Engine {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
    shared: Arc<Shared>,
    /// The order shuffle put aside (`Worker::unshuffled`), for the saved session.
    unshuffled: Arc<Mutex<Arc<Vec<String>>>>,
}

impl Engine {
    pub fn start(
        volume: f32,
        dsp: DspSettings,
        repeat: Repeat,
        shuffle: bool,
        on_status: impl Fn(&Status) + Send + 'static,
    ) -> Self {
        Self::start_on(None, volume, dsp, repeat, shuffle, on_status)
    }

    /// `start`, playing on `device` (None: the system default) from the first moment.
    pub fn start_on(
        device: Option<DeviceRef>,
        volume: f32,
        dsp: DspSettings,
        repeat: Repeat,
        shuffle: bool,
        on_status: impl Fn(&Status) + Send + 'static,
    ) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let status = Arc::new(Mutex::new(Status {
            volume,
            repeat,
            shuffle,
            ..Default::default()
        }));
        let status2 = status.clone();
        let shared = Arc::new(Shared::new(volume));
        let shared2 = shared.clone();
        let unshuffled: Arc<Mutex<Arc<Vec<String>>>> = Default::default();
        let unshuffled2 = unshuffled.clone();
        std::thread::Builder::new()
            .name("cadence-audio".into())
            .spawn(move || {
                let mut w = Worker::new(rx, status2, Box::new(on_status), shared2, volume, dsp, repeat, shuffle);
                w.memory = unshuffled2;
                w.choice = device;
                w.run()
            })
            .expect("audio thread");
        Self { tx, status, shared, unshuffled }
    }

    /// The order shuffle put aside, to put back when it's turned off (empty: none).
    pub fn unshuffled(&self) -> Arc<Vec<String>> {
        self.unshuffled.lock().clone()
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// The sleep timer's fade (0 = silent, 1 = full), on top of the volume and without moving it:
    /// the renderer glides to each new value, so steps a tenth of a second apart are one smooth fade.
    pub fn set_fade(&self, gain: f32) {
        self.shared.fade_gain.store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Diagnostics: start or stop recording the device output to `%TEMP%\cadence-tap.f32`.
    pub fn tap(&self, on: bool) {
        self.shared.tap.store(on, Ordering::Relaxed);
    }

    pub fn status(&self) -> Status {
        self.status.lock().clone()
    }
}

#[derive(Clone)]
struct Boundary {
    /// Device frame (counted from the last flush) where this song starts.
    start: u64,
    index: usize,
    /// Song position at `start`, in seconds (non-zero after a seek).
    offset: f64,
    info: SourceInfo,
    resampling: bool,
    /// Loudness gain used for this song (None = leveling off).
    gain_db: Option<f32>,
    /// Per-song tone applied (None = none).
    tone: Option<ToneInfo>,
}

/// What the per-song tone does to the current song (shown in the signal path).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToneInfo {
    /// Loudness compensation shelves (dB).
    pub bass: f32,
    pub treble: f32,
    /// Smart EQ curve in use (dB per band), if any.
    pub smart: Option<[f32; 10]>,
}

enum Step {
    Continue,
    Full,
    EndOfSong,
}

struct Worker {
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
    status: Arc<Mutex<Status>>,
    on_status: Box<dyn Fn(&Status) + Send>,
    out: Option<Output>,
    queue: Vec<QueueItem>,
    play_index: Option<usize>,
    position: f64,
    dec: Option<Source>,
    dec_index: usize,
    decoding: bool,
    /// Linear gain for the song being decoded.
    gain: f32,
    /// Frames of the current song decoded so far (to know when a crossfade should begin).
    dec_frames: u64,
    crossfade_settings: CrossfadeSettings,
    crossfade: Option<Crossfade>,
    /// Song index a crossfade was already considered for (so it isn't retried every packet).
    crossfade_checked: Option<usize>,
    /// Incoming-song audio left over when a crossfade completes (mapped, gain-adjusted and toned).
    carry: Vec<f32>,
    carry_rate: u32,
    /// The outgoing song of a crossfade ended before the blend did: the rest of the incoming
    /// song's fade-in (frames done, total) carries on by itself instead of jumping to full level.
    fade_in: Option<(u64, u64)>,
    leveling: Leveling,
    tone_settings: tone::ToneSettings,
    /// Per-song tone (before the listener's EQ).
    tone_eq: Equalizer,
    vocals: karaoke::VocalCut,
    eq: Equalizer,
    crossfeed: Crossfeed,
    resampler: Option<Resampler>,
    written: u64,
    boundaries: Vec<Boundary>,
    /// Device-format samples waiting for room in the ring.
    pending: Vec<f32>,
    mapped: Vec<f32>,
    repeat: Repeat,
    shuffle: bool,
    shuffle_mode: crate::shuffle::ShuffleMode,
    /// While shuffled: the order the songs after the current one had, to put back.
    unshuffled: Arc<Vec<String>>,
    /// Where `unshuffled` is shown to the saved session (`Engine::unshuffled`).
    memory: Arc<Mutex<Arc<Vec<String>>>>,
    /// Pause at the next song boundary that matches (sleep timer).
    stop: Option<StopAfter>,
    /// "Stop after this song": pause once the song at this queue place has played.
    stop_mark: Option<usize>,
    /// When the queue ends, keep playing songs that fit (Autoplay).
    autoplay: bool,
    /// The listener cleared Autoplay's songs: none again until they add songs or play others.
    autoplay_held: bool,
    /// An album shuffle is playing: the next album is asked for as the last one starts.
    albums: bool,
    /// The queue ran out while waiting for more songs (Autoplay or album shuffle): the songs that
    /// arrive start playing.
    ran_dry: bool,
    volume: f32,
    error: Option<String>,
    /// A song that couldn't be played and was skipped: told once (as the status error), even
    /// when the next song opens fine straight away (which clears `error`).
    skipped: Option<String>,
    /// The queue played to its end (paused at the start of its last song): Play goes on with
    /// songs added since rather than playing the last one again.
    finished: bool,
    last_emit: Instant,
    last_clip: Option<Instant>,
    exclusive: bool,
    /// See [`DspSettings::exclusive_rate`].
    exclusive_rate: u32,
    /// Exclusive mode: the next song needs the device at another rate; switch once the buffer drains.
    switch_pending: Option<usize>,
    /// Exclusive mode: when playback paused (the device is released after a while).
    idle_since: Option<Instant>,
    /// When the output was last opened, and how many times in a row it failed right after.
    opened_at: Option<Instant>,
    quick_failures: u32,
    /// The output kept failing as soon as it opened: plain shared mode until the mode changes.
    plain: bool,
    note: Option<String>,
    peak: f32,
    dirty: bool,
    /// The output device the listener chose (None: the system default, followed when it changes).
    choice: Option<DeviceRef>,
    /// The device the output was last opened on (kept while it's let go): its sound settings are
    /// the ones in use.
    device: Option<DeviceRef>,
    /// The chosen device isn't connected: paused, waiting for it.
    missing: Option<devices::Waiting>,
    /// "Play on the default" while the chosen device is missing: the default until it's back.
    stand_in: bool,
    /// Sound settings per device (`Cmd::SetProfiles`).
    profiles: Option<ProfileSource>,
    /// How many times a device's own settings were put in use (`DeviceStatus::profile`).
    profile_seq: u32,
    /// Lists the connected devices (a stand-in list in tests).
    list_devices: fn() -> Result<devices::Present>,
    /// Windows: what WASAPI says about devices coming, going, and the default changing.
    #[cfg(windows)]
    watcher: Option<devices::watch::Watcher>,
    /// The watcher's count when last looked at.
    devices_seen: u64,
    /// `Status::devices`.
    devices_seq: u64,
    /// Tests: "open" a silent output instead of a device, and count the opens.
    #[cfg(test)]
    null_output: bool,
    #[cfg(test)]
    opens: u32,
}

/// The connected output devices, asked on the worker's own thread (Android plays through its own
/// paths and has no list).
fn system_devices() -> Result<devices::Present> {
    #[cfg(not(target_os = "android"))]
    return devices::present();
    #[cfg(target_os = "android")]
    Ok(devices::Present::default())
}

/// The connected output devices, for the window (any thread). Can take a moment.
pub fn list_output_devices() -> Result<devices::Present> {
    #[cfg(not(target_os = "android"))]
    return devices::present_elsewhere();
    #[cfg(target_os = "android")]
    Ok(devices::Present::default())
}

impl Worker {
    fn new(
        rx: Receiver<Cmd>,
        status: Arc<Mutex<Status>>,
        on_status: Box<dyn Fn(&Status) + Send>,
        shared: Arc<Shared>,
        volume: f32,
        dsp: DspSettings,
        repeat: Repeat,
        shuffle: bool,
    ) -> Self {
        Self {
            rx,
            shared,
            status,
            on_status,
            out: None,
            queue: Vec::new(),
            play_index: None,
            position: 0.0,
            dec: None,
            dec_index: 0,
            decoding: false,
            gain: 1.0,
            dec_frames: 0,
            crossfade_settings: dsp.crossfade,
            crossfade: None,
            crossfade_checked: None,
            carry: Vec::new(),
            carry_rate: 0,
            fade_in: None,
            leveling: dsp.leveling,
            tone_settings: dsp.tone,
            tone_eq: Equalizer::new(EqSettings::default(), 2, 44_100),
            vocals: karaoke::VocalCut::new(dsp.vocal_cut),
            eq: Equalizer::new(dsp.eq, 2, 44_100),
            crossfeed: Crossfeed::new(dsp.crossfeed, 44_100),
            resampler: None,
            written: 0,
            boundaries: Vec::new(),
            pending: Vec::new(),
            mapped: Vec::new(),
            repeat,
            shuffle,
            shuffle_mode: Default::default(),
            unshuffled: Default::default(),
            memory: Default::default(),
            stop: None,
            stop_mark: None,
            autoplay: false,
            autoplay_held: false,
            albums: false,
            ran_dry: false,
            volume,
            error: None,
            skipped: None,
            finished: false,
            last_emit: Instant::now(),
            last_clip: None,
            exclusive: dsp.exclusive,
            exclusive_rate: dsp.exclusive_rate,
            switch_pending: None,
            idle_since: None,
            opened_at: None,
            quick_failures: 0,
            plain: false,
            note: None,
            peak: 0.0,
            dirty: true,
            choice: None,
            device: None,
            missing: None,
            stand_in: false,
            profiles: None,
            profile_seq: 0,
            list_devices: system_devices,
            #[cfg(windows)]
            watcher: None,
            devices_seen: 0,
            devices_seq: 0,
            #[cfg(test)]
            null_output: false,
            #[cfg(test)]
            opens: 0,
        }
    }

    fn run(mut self) {
        #[cfg(windows)]
        {
            // cpal's device enumerator is made by the first thread that lists devices and lives
            // with it: this one, which lives as long as Cadence (as before devices could be chosen).
            let _ = (self.list_devices)();
            self.watcher = devices::watch::start();
        }
        // Exclusive mode takes the device only once something plays.
        if !self.exclusive {
            self.open_output(None);
        }
        loop {
            let busy = self.decoding && self.has_room();
            match self.rx.recv_timeout(if busy {
                Duration::ZERO
            } else {
                Duration::from_millis(20)
            }) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Ok(cmd) = self.rx.try_recv() {
                self.handle(cmd);
            }
            if self.shared.failed.swap(false, Ordering::AcqRel) {
                self.recover_output();
            }
            // Devices came or went, or the default changed (two atomic reads when nothing did).
            #[cfg(windows)]
            if let Some(seen) = self.watcher.as_ref().map(|w| w.events.changed.load(Ordering::Acquire))
                && seen != self.devices_seen
            {
                self.devices_seen = seen;
                self.devices_seq += 1;
                let default = self.watcher.as_ref().is_some_and(|w| w.events.default_changed.swap(false, Ordering::AcqRel));
                self.devices_changed(default);
            }
            #[cfg(target_os = "android")]
            {
                // The user just allowed Cadence to use the USB DAC: switch to it.
                if self.exclusive && usb_dac::allowed() {
                    log::info!("allowed to use the USB DAC");
                    self.note = None;
                    self.reopen(false);
                }
                if self.out.as_ref().is_some_and(|o| matches!(o.backend, output::Backend::Usb(_))) {
                    usb_dac::sync_volume();
                }
            }
            self.fill();
            self.update_position();
            self.release_if_idle();
            let interval = if self.shared.playing.load(Ordering::Relaxed) {
                250
            } else {
                1000
            };
            if self.dirty || self.last_emit.elapsed() >= Duration::from_millis(interval) {
                self.emit();
            }
        }
    }

    // ---------- output device ----------

    /// Opens the output: exclusive at `rate` when that's enabled (falling back to shared mode), else shared.
    fn open_output(&mut self, rate: Option<u32>) {
        // Release the current device first: exclusive mode needs it free.
        self.out = None;
        // The last output held for a while: earlier failures right after opening don't count any more.
        if !self.opened_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            self.quick_failures = 0;
        }
        // Which device (None: the system default, the platform's own way).
        let Ok(device) = self.device_to_open() else {
            self.dirty = true;
            return;
        };
        // Another device: its own sound settings first (Direct output is one of them).
        if let Some(d) = &device {
            self.switched_to(d);
        }
        #[cfg(test)]
        if self.null_output {
            let (prod, _) = ringbuf::traits::Split::split(ringbuf::HeapRb::<f32>::new(1 << 16));
            let exclusive = self.exclusive;
            self.out = Some(Output { backend: output::Backend::Null, prod, rate: rate.unwrap_or(44_100), channels: 2, device: format!("test exclusive={exclusive}"), bits: None });
            self.opened_at = Some(Instant::now());
            self.opens += 1;
            self.dirty = true;
            return;
        }
        // Windows always names the device (also the default: see `devices`); elsewhere the system
        // default is opened as before, so it keeps following the system's own changes.
        let id: Option<&str> = match &device {
            Some(d) if cfg!(windows) || (self.choice.is_some() && !self.stand_in) => Some(d.id.as_str()),
            _ => None,
        };
        let opened = match (self.exclusive, rate) {
            _ if self.plain => output::open_shared(self.shared.clone(), id),
            #[cfg(windows)]
            (true, Some(rate)) => match exclusive::open(self.shared.clone(), self.device_rate(rate), id) {
                Ok(o) => {
                    self.note = None;
                    Ok(o)
                }
                Err(e) => {
                    log::warn!("exclusive mode failed: {e:#}");
                    self.note = Some(format!(
                        "Exclusive mode isn’t available right now ({e}), so Cadence is playing through the Windows mixer."
                    ));
                    output::open_shared(self.shared.clone(), id)
                }
            },
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            (true, Some(rate)) => match output::open_direct(self.shared.clone(), self.device_rate(rate), id) {
                Ok(o) => {
                    self.note = None;
                    Ok(o)
                }
                Err(e) => {
                    log::warn!("direct output failed: {e:#}");
                    self.note = Some(format!(
                        "Direct output isn’t available right now ({e}), so Cadence is playing through the system mixer."
                    ));
                    output::open_shared(self.shared.clone(), id)
                }
            },
            // Android's own bit-perfect USB where the phone offers it, else Cadence's USB driver.
            #[cfg(target_os = "android")]
            (true, Some(rate)) => match android_direct::open(self.shared.clone(), self.device_rate(rate)).or_else(|e| {
                log::info!("no bit-perfect USB from Android ({e:#}); trying Cadence's own driver");
                usb_dac::open(self.shared.clone(), self.device_rate(rate))
            }) {
                Ok(o) => {
                    self.note = None;
                    Ok(o)
                }
                Err(e) => {
                    log::warn!("direct output failed: {e:#}");
                    self.note = Some(format!(
                        "Direct output isn’t available right now ({e}), so Cadence is playing through Android’s mixer, at the song’s own rate."
                    ));
                    android_direct::open_native(self.shared.clone(), self.device_rate(rate))
                }
            },
            // Android's mixer takes each song at its own rate (see `android_direct::open_native`).
            #[cfg(target_os = "android")]
            (false, Some(rate)) => android_direct::open_native(self.shared.clone(), rate),
            #[cfg(not(any(windows, target_os = "macos", target_os = "linux", target_os = "android")))]
            (true, Some(_)) => {
                self.note = Some("Direct output isn’t available here; Cadence is playing through the system mixer.".into());
                output::open_shared(self.shared.clone(), id)
            }
            _ => output::open_shared(self.shared.clone(), id),
        };
        match opened {
            Ok(out) => {
                log::info!(
                    "output: {} @ {} Hz, {} ch, exclusive={}",
                    out.device,
                    out.rate,
                    out.channels,
                    out.exclusive()
                );
                self.out = Some(out);
                self.opened_at = Some(Instant::now());
                self.error = None;
            }
            Err(e) => {
                log::error!("no output device: {e:#}");
                self.out = None;
                self.error = Some(format!("No audio output: {e}"));
            }
        }
        self.dirty = true;
    }

    /// The device changed (unplugged, new default...): reopen and continue where we were.
    fn recover_output(&mut self) {
        // A device that fails as soon as it's opened would otherwise be reopened forever (e.g. a
        // system that keeps interrupting a stream at the song's rate): fall back to plain shared mode.
        let quick = self.opened_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        self.quick_failures = if quick { self.quick_failures + 1 } else { 0 };
        #[cfg(target_os = "android")]
        if let Some(o) = self.out.as_ref().filter(|_| quick) {
            android_direct::interrupted(o);
        }
        if self.quick_failures >= 3 && !self.plain {
            log::warn!("the output keeps failing right after opening; using plain shared mode");
            self.plain = true;
            self.note = Some(format!(
                "Playback kept being interrupted, so Cadence is playing through the {} at its usual rate.",
                if cfg!(target_os = "android") { "Android mixer" } else { "system mixer" }
            ));
        }
        self.reopen(true);
    }

    /// Opens the output again and carries on where playback was (`settle`: after a failure, give the
    /// system a moment first).
    fn reopen(&mut self, settle: bool) {
        let playing = self.shared.playing.load(Ordering::Relaxed);
        self.out = None;
        if settle {
            std::thread::sleep(Duration::from_millis(300));
        }
        self.reopen_with(playing);
    }

    /// Opens the output again (on the device it should be on now) and carries on from the same
    /// place, playing or paused.
    fn reopen_with(&mut self, play: bool) {
        self.out = None;
        match self.play_index {
            // start_at reopens the device at the song's rate.
            Some(i) => self.start_at(i, self.position, play),
            None if !self.exclusive => self.open_output(None),
            None => {}
        }
    }

    /// The device to open now: the chosen one, or the system default (also while standing in for
    /// a chosen one that's missing). `Ok(None)`: the system default, opened the platform's own way
    /// (Android; or the list couldn't be read, off Windows). `Err`: nothing to open (the chosen
    /// device is missing: paused, waiting for it; or there's no device at all).
    fn device_to_open(&mut self) -> Result<Option<DeviceRef>, ()> {
        if cfg!(target_os = "android") {
            return Ok(None);
        }
        let chosen = if self.stand_in { None } else { self.choice.clone() };
        let present = match (self.list_devices)() {
            Ok(p) => p,
            Err(e) => {
                log::warn!("can't list the output devices: {e:#}");
                if chosen.is_none() && !cfg!(windows) {
                    return Ok(None);
                }
                devices::Present::default()
            }
        };
        match devices::choose(chosen.as_ref(), &present) {
            devices::Target::Open(d) => {
                self.missing = None;
                Ok(Some(d))
            }
            devices::Target::Missing(d) => {
                self.wait_for(d, present.default_name());
                Err(())
            }
            devices::Target::Nothing => {
                self.error = Some("No audio output: no output device is connected".into());
                Err(())
            }
        }
    }

    /// The chosen device can't be opened: pause and wait for it. Playback carries on by itself if
    /// it was playing and the device is back soon (`devices::Waiting::resumes`).
    fn wait_for(&mut self, device: DeviceRef, fallback: Option<String>) {
        let playing = self.shared.playing.load(Ordering::Relaxed);
        match &mut self.missing {
            Some(w) if w.device.id == device.id => {
                w.fallback = fallback;
                w.resume |= playing;
            }
            _ => {
                log::warn!("{} isn't connected: paused until it's back", device.name);
                self.missing = Some(devices::Waiting { device, since: Instant::now(), resume: playing, fallback });
            }
        }
        self.out = None;
        self.shared.playing.store(false, Ordering::Relaxed);
        self.dirty = true;
    }

    /// Playback is about to open `d`. Another device than before: its own sound settings and
    /// volume come into use (before it opens, so the first sample already has them).
    fn switched_to(&mut self, d: &DeviceRef) {
        if let Some(cur) = self.device.as_mut().filter(|c| c.id == d.id) {
            if !d.name.trim().is_empty() {
                cur.name = d.name.clone();
            }
            return;
        }
        log::info!("output device: {}", d.name);
        self.device = Some(d.clone());
        self.dirty = true;
        let profile = match self.profiles.as_mut() {
            Some(f) => f(d),
            None => None,
        };
        if let Some((dsp, volume)) = profile {
            self.profile_seq += 1;
            self.set_volume(volume);
            // Live where it can be; whatever needs a restart has it as this device opens.
            self.take_dsp(dsp);
        }
    }

    /// Devices came or went, or the default changed (Windows tells; elsewhere `Cmd::CheckDevices`).
    fn devices_changed(&mut self, default_changed: bool) {
        self.dirty = true;
        let following = self.choice.is_none() || self.stand_in;
        if self.missing.is_none() && !self.stand_in && !(default_changed && following) {
            return;
        }
        let Ok(present) = (self.list_devices)() else { return };
        if let Some(w) = self.missing.clone() {
            if present.has(&w.device.id) {
                // Back: carry on if it was playing and that wasn't long ago; else stay paused
                // (the next Play opens it).
                let resume = w.resumes(Instant::now());
                log::info!("{} is back{}", w.device.name, if resume { "; playing on" } else { "" });
                self.missing = None;
                if resume {
                    self.reopen_with(true);
                }
            } else if let Some(m) = self.missing.as_mut() {
                m.fallback = present.default_name();
            }
            return;
        }
        if self.stand_in && self.choice.as_ref().is_some_and(|c| present.has(&c.id)) {
            log::info!("the chosen output device is back; playing there again");
            self.stand_in = false;
            let playing = self.shared.playing.load(Ordering::Relaxed);
            self.reopen_with(playing);
            return;
        }
        // Following the system default: a new default (another device picked in the system's sound
        // settings, or plugged in) is played on, as the system mixer would.
        let new = present.default.as_ref().map(|d| d.id.as_str());
        if default_changed && following && self.out.is_some() && new.is_some() && new != self.device.as_ref().map(|d| d.id.as_str()) {
            log::info!("the default output changed; playing there");
            let playing = self.shared.playing.load(Ordering::Relaxed);
            self.reopen_with(playing);
        }
    }

    fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.shared.volume.store(self.volume.to_bits(), Ordering::Relaxed);
    }

    /// Whether the open device is wrong for a song at `rate` (mode changed, or exclusive at another
    /// rate). `gapless`: the song follows on from the previous one.
    fn needs_reopen(&self, rate: u32, gapless: bool) -> bool {
        if self.plain {
            return self.out.is_none();
        }
        !self.out.as_ref().is_some_and(|o| {
            // Direct output that fell back to the mixer is tried again when a song starts afresh,
            // not between songs (that would break gapless playback for nothing).
            let mode_fits = o.exclusive() == self.exclusive || (gapless && self.exclusive);
            // Exclusive mode runs the device at each song's rate, and so does Android's mixer (see
            // `android_direct::open_native`); other mixers resample.
            let follows = o.exclusive() || cfg!(target_os = "android");
            let wanted = if self.exclusive { self.device_rate(rate) } else { rate };
            mode_fits && (!follows || o.rate == wanted)
        })
    }

    /// The rate to open the device at in exclusive mode for a song at `rate`.
    fn device_rate(&self, rate: u32) -> u32 {
        if self.exclusive_rate > 0 {
            self.exclusive_rate
        } else {
            rate
        }
    }

    /// In exclusive mode, gives the device back to Windows after 20 s paused, so other apps can play.
    fn release_if_idle(&mut self) {
        if !self.out.as_ref().is_some_and(|o| o.exclusive()) {
            self.idle_since = None;
            return;
        }
        if self.shared.playing.load(Ordering::Relaxed) {
            self.idle_since = None;
        } else if self.idle_since.get_or_insert_with(Instant::now).elapsed()
            > Duration::from_secs(20)
        {
            log::info!("released the device while paused");
            self.out = None;
            self.idle_since = None;
            self.dirty = true;
        }
    }

    fn has_room(&self) -> bool {
        self.out
            .as_ref()
            .is_some_and(|o| o.prod.vacant_len() >= 4096 * o.channels)
    }

    /// Drops everything buffered (seek, new song) and waits for the renderer to confirm (it fades out first).
    fn flush_output(&mut self) {
        self.shared.flush.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.shared.flush.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        // Renderer not running (no device / stopped): nothing can be consuming, so just reset.
        self.shared.flush.store(false, Ordering::Release);
        self.shared.consumed.store(0, Ordering::Release);
        self.pending.clear();
        self.written = 0;
        self.boundaries.clear();
    }

    // ---------- commands ----------

    fn handle(&mut self, cmd: Cmd) {
        self.dirty = true;
        match cmd {
            Cmd::Load { mut items, mut index } => {
                self.new_queue();
                if self.shuffle && index < items.len() && items.len() > 1 {
                    // The chosen song first, the rest in a random order (put back in the list's
                    // order, from that song on, if shuffle is turned off).
                    let first = items.remove(index);
                    items.rotate_left(index);
                    self.set_unshuffled(items.iter().map(|q| q.id.clone()).collect());
                    shuffle_items(self.shuffle_mode, &mut items, Some(&first));
                    items.insert(0, first);
                    index = 0;
                }
                self.queue = items;
                if index < self.queue.len() {
                    self.start_at(index, 0.0, true);
                }
            }
            Cmd::LoadShuffled(mut items) => {
                self.new_queue();
                if items.is_empty() {
                    return;
                }
                if self.shuffle {
                    self.set_unshuffled(items.iter().map(|q| q.id.clone()).collect());
                }
                shuffle_items(self.shuffle_mode, &mut items, None);
                self.queue = items;
                self.start_at(0, 0.0, true);
            }
            Cmd::PlayAlbums { items, more } => {
                self.new_queue();
                if items.is_empty() {
                    return;
                }
                self.albums = more;
                self.queue = items;
                self.start_at(0, 0.0, true);
            }
            Cmd::Extend { items, auto } => self.extend(items, auto),
            Cmd::StopAt(at) => {
                // On the playing song or one still to come.
                self.stop_mark = at.filter(|&i| i < self.queue.len() && self.play_index.is_some_and(|pi| i >= pi));
            }
            Cmd::SetShuffleMode(mode) => self.shuffle_mode = mode,
            Cmd::SetAutoplay(on) => {
                self.autoplay = on;
                if !on {
                    self.clear_autoplay();
                }
            }
            Cmd::ClearAutoplay => {
                self.clear_autoplay();
                self.autoplay_held = true;
            }
            Cmd::Restore { items, index, position, unshuffled, autoplay_from } => {
                self.new_queue();
                self.queue = items;
                if self.shuffle {
                    self.set_unshuffled(unshuffled);
                }
                if let Some(from) = autoplay_from.filter(|&f| f > index) {
                    for q in self.queue.iter_mut().skip(from) {
                        q.auto = true;
                    }
                }
                if let Some(item) = self.queue.get(index) {
                    // Not right at the very end, or it would simply move on to the next song.
                    let at = position.clamp(0.0, (item.duration - 2.0).max(0.0));
                    self.start_at(index, at, false);
                }
            }
            Cmd::Play => self.play(),
            Cmd::Pause => {
                self.shared.playing.store(false, Ordering::Relaxed);
                // Waiting for the chosen device: paused for good, not carrying on when it's back.
                if let Some(w) = self.missing.as_mut() {
                    w.resume = false;
                }
            }
            Cmd::Toggle => {
                if self.shared.playing.load(Ordering::Relaxed) {
                    self.shared.playing.store(false, Ordering::Relaxed);
                } else if let Some(w) = self.missing.as_mut().filter(|w| w.resume) {
                    // Waiting to carry on when the chosen device is back: not any more.
                    w.resume = false;
                } else {
                    self.play();
                }
            }
            Cmd::Next => {
                if let Some(i) = self.play_index {
                    match self.manual_next(i) {
                        Some(n) => self.start_at(n, 0.0, true),
                        // End of the queue: stop at the start of the last song.
                        None => self.start_at(i, 0.0, false),
                    }
                }
            }
            Cmd::Prev => {
                // Like Apple Music: past the first 3 seconds, "previous" restarts the song.
                if let Some(i) = self.play_index {
                    let target = if self.position > 3.0 {
                        i
                    } else if i > 0 {
                        i - 1
                    } else if self.repeat == Repeat::All {
                        // Repeat All goes round: before the first song comes the last.
                        self.queue.len().saturating_sub(1)
                    } else {
                        i
                    };
                    self.start_at(target, 0.0, true);
                }
            }
            Cmd::Seek(secs) => self.restart_here(Some(secs)),
            Cmd::SeekBy(delta) => {
                let target = (self.position + delta).max(0.0);
                self.restart_here(Some(target));
            }
            Cmd::Jump(i) => {
                if i < self.queue.len() {
                    self.start_at(i, 0.0, true);
                }
            }
            Cmd::Enqueue { items, next } => {
                if self.queue.is_empty() || self.play_index.is_none() {
                    self.new_queue();
                    self.queue = items;
                    self.start_at(0, 0.0, true);
                    return;
                }
                let at = if next {
                    self.play_index.unwrap() + 1
                } else {
                    // Before the songs Autoplay added: the listener's own songs come first.
                    self.auto_start().unwrap_or(self.queue.len())
                };
                self.insert_items(at, items);
            }
            Cmd::Insert { at, items } => {
                if self.play_index.is_none() {
                    return;
                }
                self.insert_items(at, items);
            }
            Cmd::ClearUpNext => {
                if let Some(pi) = self.play_index
                    && self.queue.len() > pi + 1 {
                        let prepared = self.next_is_prepared(pi);
                        self.queue.truncate(pi + 1);
                        self.set_unshuffled(Vec::new());
                        // Nothing waiting any more: an album shuffle ends here too.
                        self.albums = false;
                        self.stop_mark = self.stop_mark.filter(|&m| m <= pi);
                        if prepared {
                            // The next song was already on its way to the device.
                            self.restart_here(None);
                        }
                    }
            }
            Cmd::Move { from, to } => {
                let Some(pi) = self.play_index else { return };
                let n = self.queue.len();
                if from == to || from <= pi || to <= pi || from >= n || to >= n {
                    return;
                }
                let prepared = self.next_is_prepared(pi);
                let marked = self.stop_mark == Some(from);
                let item = self.queue.remove(from);
                self.shift_indices(from + 1, -1);
                self.queue.insert(to, item);
                self.shift_indices(to, 1);
                if marked {
                    self.stop_mark = Some(to);
                }
                // Moved in among Autoplay's songs, it's one of them; out of them, the listener's.
                let after = self.queue.get(to + 1).map_or(self.queue[to - 1].auto, |q| q.auto);
                self.queue[to].auto = after;
                if prepared && (from == pi + 1 || to == pi + 1) {
                    self.restart_here(None);
                }
            }
            Cmd::StopAfter(rule) => self.stop = rule,
            Cmd::Remove(i) => {
                if i >= self.queue.len() || Some(i) == self.play_index {
                    return;
                }
                if self.stop_mark == Some(i) {
                    self.stop_mark = None;
                }
                self.queue.remove(i);
                // Already on its way: being decoded, blended in, waiting for the device to switch
                // rate, or decoded into the buffer already (a short song).
                let was_prepared = (self.decoding && self.dec_index == i)
                    || self.crossfade.as_ref().is_some_and(|x| x.index == i)
                    || self.switch_pending == Some(i)
                    || self.boundaries.iter().any(|b| b.index == i);
                self.shift_indices(i + 1, -1);
                if was_prepared {
                    // The removed song was already being prepared as "next": redo that part.
                    self.restart_here(None);
                }
            }
            Cmd::UpdateGains(gains) => {
                for (id, gain) in gains {
                    for q in self.queue.iter_mut().filter(|q| q.id == id) {
                        q.gain = gain;
                    }
                }
            }
            Cmd::SetVolume(v) => self.set_volume(v),
            Cmd::SetDsp(dsp) => {
                // A new loudness gain has to start at the current position, not half a second later.
                if self.set_dsp(dsp) {
                    self.restart_here(None);
                }
            }
            Cmd::SetDevice(choice) => {
                let same = choice.as_ref().map(|d| &d.id) == self.choice.as_ref().map(|d| &d.id);
                if same && !self.stand_in && self.missing.is_none() && self.out.is_some() {
                    return;
                }
                self.choice = choice;
                self.stand_in = false;
                // Playing (or waiting to play on) the old one: play on the new one, from here.
                let playing = self.shared.playing.load(Ordering::Relaxed) || self.missing.take().is_some_and(|w| w.resume);
                self.missing = None;
                self.reopen_with(playing);
            }
            Cmd::UseDefaultForNow => {
                if self.missing.take().is_some() {
                    log::info!("playing on the system default until the chosen device is back");
                    self.stand_in = true;
                    self.reopen_with(true);
                }
            }
            Cmd::CheckDevices => self.devices_changed(false),
            Cmd::SetProfiles(source) => {
                self.profiles = source;
                // The device already open (Cadence just started): its own settings.
                let profile = match (self.profiles.as_mut(), self.device.as_ref()) {
                    (Some(f), Some(d)) => f(d),
                    _ => None,
                };
                if let Some((dsp, volume)) = profile {
                    self.profile_seq += 1;
                    self.set_volume(volume);
                    if self.set_dsp(dsp) {
                        self.restart_here(None);
                    }
                }
            }
            Cmd::SetRepeat(r) => {
                let before = self.play_index.map(|i| self.auto_next(i));
                self.repeat = r;
                // The next song may already be prepared (the last moments of a song, or a
                // crossfade): if the new mode picks another one, prepare that instead.
                if let Some(pi) = self.play_index
                    && before != Some(self.auto_next(pi))
                    && self.moved_on(pi)
                {
                    self.restart_here(None);
                }
            }
            Cmd::SetShuffle(on) => self.set_shuffle(on),
            Cmd::Replace(pairs) => {
                for q in self.queue.iter_mut() {
                    if let Some((_, new)) = pairs.iter().find(|(old, _)| *old == q.id) {
                        *q = new.clone();
                    }
                }
                if !self.unshuffled.is_empty() {
                    let mut ids = (*self.unshuffled).clone();
                    for id in ids.iter_mut() {
                        if let Some((_, new)) = pairs.iter().find(|(old, _)| old == id) {
                            *id = new.id.clone();
                        }
                    }
                    self.set_unshuffled(ids);
                }
                self.dirty = true;
            }
        }
    }

    /// New sound settings. Returns whether the song must restart for them (Sound Check, the tone or
    /// the output mode changed); the output is reopened in the new mode by that restart.
    fn set_dsp(&mut self, dsp: DspSettings) -> bool {
        let (restart, mode_changed) = self.take_dsp(dsp);
        if mode_changed {
            // Restarting the song opens the device in the new mode.
            self.out = None;
            // Direct output off: Android gets the USB DAC back now (not a moment later), so
            // playing through it finds the DAC rather than the speaker.
            #[cfg(target_os = "android")]
            if !self.exclusive {
                usb_dac::give_back(true);
            }
            if self.play_index.is_none() && !self.exclusive {
                self.open_output(None);
            }
        }
        restart
    }

    /// Takes new sound settings (filters change live); doesn't touch the output. Returns (the song
    /// must restart, the output mode changed).
    fn take_dsp(&mut self, dsp: DspSettings) -> (bool, bool) {
        let leveling_changed = dsp.leveling != self.leveling || dsp.tone != self.tone_settings;
        self.tone_settings = dsp.tone;
        let mode_changed = dsp.exclusive != self.exclusive
            || (dsp.exclusive && dsp.exclusive_rate != self.exclusive_rate);
        self.exclusive = dsp.exclusive;
        self.exclusive_rate = dsp.exclusive_rate;
        if mode_changed {
            self.note = None;
            // Changing the mode tries it again.
            self.plain = false;
            self.quick_failures = 0;
        }
        self.vocals.set_amount(dsp.vocal_cut);
        self.eq.set_settings(dsp.eq);
        self.crossfeed.set_settings(dsp.crossfeed);
        self.crossfade_settings = dsp.crossfade;
        self.leveling = dsp.leveling;
        (leveling_changed || mode_changed, mode_changed)
    }

    fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        self.dirty = true;
        // Shuffle turned on or off ends an album shuffle (its albums stay as they are queued).
        self.albums = false;
        let Some(pi) = self.play_index else {
            self.set_unshuffled(Vec::new());
            return;
        };
        let marked = self.stop_mark.filter(|&m| m > pi).and_then(|m| self.queue.get(m)).map(|q| q.id.clone());
        // Autoplay's songs stay where they are, after the listener's own.
        let tail = self.queue.split_off(self.auto_start().unwrap_or(self.queue.len()));
        let mut rest = self.queue.split_off(pi + 1);
        if on {
            self.set_unshuffled(rest.iter().map(|q| q.id.clone()).collect());
            shuffle_items(self.shuffle_mode, &mut rest, self.queue.get(pi));
        } else {
            // Back in the order they had; songs added since keep their place at the end.
            let ids: Vec<&str> = rest.iter().map(|q| q.id.as_str()).collect();
            let order = crate::shuffle::unshuffle_order(&ids, &self.unshuffled);
            crate::shuffle::apply(&mut rest, &order);
            self.set_unshuffled(Vec::new());
        }
        self.queue.append(&mut rest);
        self.queue.extend(tail);
        if let Some(id) = marked {
            self.stop_mark = self.queue.iter().skip(pi + 1).position(|q| q.id == id).map(|k| k + pi + 1);
        }
        // The next song may already be on its way to the device: prepare the new one instead.
        if self.next_is_prepared(pi) {
            self.restart_here(None);
        }
    }

    /// A new queue replaces the old one: what belonged to the old one goes.
    fn new_queue(&mut self) {
        self.set_unshuffled(Vec::new());
        self.stop_mark = None;
        self.albums = false;
        self.ran_dry = false;
        self.autoplay_held = false;
    }

    /// Keeps the order shuffle put aside, and shows it to the saved session.
    fn set_unshuffled(&mut self, ids: Vec<String>) {
        if ids.is_empty() && self.unshuffled.is_empty() {
            return;
        }
        self.unshuffled = Arc::new(ids);
        *self.memory.lock() = self.unshuffled.clone();
    }

    /// Where the songs Autoplay added begin: the run of them at the end of the queue, after the
    /// playing song.
    fn auto_start(&self) -> Option<usize> {
        let pi = self.play_index?;
        let mut start = self.queue.len();
        while start > pi + 1 && self.queue[start - 1].auto {
            start -= 1;
        }
        (start < self.queue.len()).then_some(start)
    }

    fn clear_autoplay(&mut self) {
        let (Some(pi), Some(start)) = (self.play_index, self.auto_start()) else { return };
        let prepared = self.next_is_prepared(pi);
        self.queue.truncate(start);
        self.stop_mark = self.stop_mark.filter(|&m| m < start);
        if prepared && start == pi + 1 {
            self.restart_here(None);
        }
    }

    /// What the end of the queue is waiting for, if anything: the next album of an album shuffle
    /// (the playing song's album is the last one queued), or Autoplay's songs (the last or the
    /// next-to-last song plays). Never while paused, with a stop set (sleep timer, "stop after
    /// this song") or repeating one song; Autoplay only without repeat.
    fn wants_more(&self) -> Option<More> {
        let pi = self.play_index?;
        if !self.shared.playing.load(Ordering::Relaxed) || self.stop.is_some() || self.stop_mark.is_some() || self.repeat == Repeat::One {
            return None;
        }
        if self.albums {
            let album = &self.queue.get(pi)?.album_id;
            return self.queue.iter().skip(pi + 1).all(|q| &q.album_id == album).then_some(More::Albums);
        }
        (self.autoplay && !self.autoplay_held && self.repeat == Repeat::Off && self.queue.len() <= pi + 2).then_some(More::Similar)
    }

    /// Adds the songs `wants_more` asked for at the end of the queue.
    fn extend(&mut self, items: Vec<QueueItem>, auto: bool) {
        if items.is_empty() || self.queue.is_empty() {
            return;
        }
        let at = self.queue.len();
        self.queue.extend(items.into_iter().map(|mut q| {
            q.auto = auto;
            q
        }));
        let last = Some(at - 1);
        if self.finished && self.ran_dry && self.play_index == last {
            // They came after the queue ran out: play on with them.
            self.ran_dry = false;
            self.start_at(at, 0.0, true);
        } else if !self.finished && !self.decoding && self.switch_pending.is_none() && self.play_index.is_some() && self.dec_index + 1 == at && self.crossfade.is_none() {
            // The decoder had already reached the end of the queue: it carries on into them.
            self.finish_track();
        }
    }

    /// Whether the song after `pi` is already being decoded or buffered for the device, or waiting
    /// for it to switch rate (another index: Repeat All goes round to the first song).
    fn next_is_prepared(&self, pi: usize) -> bool {
        self.dec_index != pi
            || self.boundaries.iter().any(|b| b.index != pi)
            || self.switch_pending.is_some()
    }

    /// Whether what follows song `pi` was already decided: the decoder finished it (and found the
    /// next song, or none), or a next song is decoding, blending in or waiting for a rate switch.
    fn moved_on(&self, pi: usize) -> bool {
        !self.decoding
            || self.dec_index != pi
            || self.boundaries.len() > 1
            || self.crossfade.is_some()
            || self.switch_pending.is_some()
    }

    /// Puts `items` into the queue at `at`, never before the song that's playing.
    fn insert_items(&mut self, at: usize, items: Vec<QueueItem>) {
        // Songs the listener added: Autoplay may carry on after them again.
        self.autoplay_held = false;
        let pi = self.play_index.unwrap_or(0);
        let at = at.clamp(pi + 1, self.queue.len());
        let n = items.len();
        let prepared = self.next_is_prepared(pi);
        self.queue.splice(at..at, items);
        self.shift_indices(at, n as isize);
        if prepared && at == pi + 1 {
            // What was about to play next is now second: prepare the new next song instead.
            self.restart_here(None);
        }
    }

    /// Crossing from song `old` into `new`: whether "stop after this song" (on `old`) or the
    /// sleep timer's rule says to stop here (counting the song down when it doesn't). A rule or
    /// mark that stops is used up.
    fn stop_here(&mut self, old: Option<usize>, new: usize) -> bool {
        if old.is_some() && self.stop_mark == old {
            self.stop_mark = None;
            return true;
        }
        let stop = self.sleep_rule_stops(old, new);
        if stop {
            self.stop = None;
        }
        stop
    }

    fn sleep_rule_stops(&mut self, old: Option<usize>, new: usize) -> bool {
        match self.stop.clone() {
            None => false,
            Some(StopAfter::Songs { left }) => {
                if left <= 1 {
                    true
                } else {
                    self.stop = Some(StopAfter::Songs { left: left - 1 });
                    false
                }
            }
            Some(StopAfter::Album) => {
                let album = |i: usize| self.queue.get(i).map(|q| q.album_id.as_str());
                old.and_then(album) != album(new)
            }
        }
    }

    /// Restarts decoding at the current (or given) position of the current song, keeping play/pause.
    fn restart_here(&mut self, at: Option<f64>) {
        if let Some(i) = self.play_index {
            let playing = self.shared.playing.load(Ordering::Relaxed);
            self.start_at(i, at.unwrap_or(self.position), playing);
        }
    }

    /// Keeps queue positions right after inserting (`delta` > 0) or removing items at `at`.
    fn shift_indices(&mut self, at: usize, delta: isize) {
        let fix = |i: usize| {
            if i >= at {
                (i as isize + delta).max(0) as usize
            } else {
                i
            }
        };
        self.play_index = self.play_index.map(fix);
        self.stop_mark = self.stop_mark.map(fix);
        self.dec_index = fix(self.dec_index);
        self.switch_pending = self.switch_pending.map(fix);
        if let Some(xf) = &mut self.crossfade {
            xf.index = fix(xf.index);
        }
        for b in &mut self.boundaries {
            b.index = fix(b.index);
        }
    }

    fn play(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let ended = !self.decoding
            && self.pending.is_empty()
            && self.shared.consumed.load(Ordering::Acquire) >= self.written;
        match self.play_index {
            None => self.start_at(0, 0.0, true),
            // The queue had played to its end and songs were added since: go on with them.
            Some(i) if self.finished && i + 1 < self.queue.len() => self.start_at(i + 1, 0.0, true),
            Some(i) if ended => self.start_at(i, 0.0, true),
            // The device was released while paused: take it again and carry on from here.
            Some(i) if self.out.is_none() => self.start_at(i, self.position, true),
            Some(_) => self.shared.playing.store(true, Ordering::Relaxed),
        }
    }

    fn manual_next(&self, i: usize) -> Option<usize> {
        if i + 1 < self.queue.len() {
            Some(i + 1)
        } else if self.repeat == Repeat::All {
            Some(0)
        } else {
            None
        }
    }

    fn auto_next(&self, i: usize) -> Option<usize> {
        if self.repeat == Repeat::One {
            Some(i)
        } else {
            self.manual_next(i)
        }
    }

    /// Starts decoding song `index` at `secs`, dropping whatever was buffered.
    fn start_at(&mut self, index: usize, secs: f64, play: bool) {
        self.flush_output();
        self.tone_eq.reset();
        self.vocals.reset();
        self.eq.reset();
        self.crossfeed.reset();
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
        self.play_index = Some(index);
        // Gone past the song marked "stop after this song" (skipped ahead): the mark goes.
        self.stop_mark = self.stop_mark.filter(|&m| m >= index);
        self.position = secs;
        self.decoding = false;
        self.dec = None;
        self.crossfade = None;
        self.crossfade_checked = None;
        self.carry.clear();
        self.fade_in = None;
        self.finished = false;
        self.dirty = true;
        self.switch_pending = None;
        let mut tries = 0;
        let mut index = index;
        loop {
            match self.open_source(index, secs, true) {
                Ok(offset) => {
                    let offset = offset.unwrap_or(secs);
                    self.play_index = Some(index);
                    self.position = offset;
                    // The chosen device isn't connected: stay paused; asked to play, carry on
                    // when it's back (if that's soon).
                    let play = match self.missing.as_mut() {
                        Some(w) => {
                            if play && !w.resume {
                                w.resume = true;
                                w.since = Instant::now();
                            }
                            false
                        }
                        None => play,
                    };
                    self.shared.playing.store(play, Ordering::Relaxed);
                    return;
                }
                Err(e) => {
                    log::warn!("can't play {}: {e:#}", self.queue[index].path.display());
                    // Kept apart from `error`, which opening the next song clears.
                    self.skipped = Some(format!(
                        "Couldn't play “{}”: {e}",
                        file_name(&self.queue[index].path)
                    ));
                    tries += 1;
                    match self.manual_next(index) {
                        Some(n) if tries < self.queue.len() => index = n,
                        _ => {
                            self.shared.playing.store(false, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Whether song `index` is being played as part of its album (a neighbour in the queue is from it).
    fn album_context(&self, index: usize) -> bool {
        let id = &self.queue[index].album_id;
        let same = |i: Option<usize>| {
            i.and_then(|i| self.queue.get(i))
                .is_some_and(|q| &q.album_id == id)
        };
        same(index.checked_sub(1)) || same(Some(index + 1))
    }

    /// The per-song tone for song `index`: the tone EQ's settings, what to show for it, and how far
    /// the song may be lifted back (its peak headroom after Sound Check's gain, less 1 dB).
    /// While an album plays in order its songs share one curve (the album's average), so gapless
    /// transitions never change the filters mid-stream.
    fn tone_plan(&self, index: usize, gain_db: Option<f32>) -> (EqSettings, Option<ToneInfo>, f32) {
        let album = self.album_context(index);
        let item = &self.queue[index];
        let smart = if album {
            let curves: Vec<&[f32; 10]> = self
                .queue
                .iter()
                .filter(|q| q.album_id == item.album_id)
                .filter_map(|q| q.tone.as_ref())
                .collect();
            (!curves.is_empty()).then(|| {
                let mut avg = [0.0f32; 10];
                for c in &curves {
                    avg.iter_mut().zip(c.iter()).for_each(|(a, v)| *a += v / curves.len() as f32);
                }
                avg
            })
        } else {
            item.tone
        };
        let peak = if album && self.leveling.mode != loudness::LevelingMode::Track {
            item.gain.album.or(item.gain.track)
        } else {
            item.gain.track
        }
        .map(|g| g.peak)
        .unwrap_or(1.0);
        let settings = tone::settings_for(&self.tone_settings, gain_db, smart.as_ref());
        let gain = gain_db.unwrap_or(0.0);
        let room = (-gain - 20.0 * peak.max(1e-6).log10() - 1.0).max(0.0);
        let info = settings.enabled.then(|| {
            let (bass, treble) = if self.tone_settings.loudness {
                tone::loudness_shelves(-gain)
            } else {
                (0.0, 0.0)
            };
            ToneInfo {
                bass,
                treble,
                smart: smart.filter(|_| self.tone_settings.smart),
            }
        });
        (settings, info, room)
    }

    /// A song's linear gain: Sound Check's gain plus back whatever level its tone EQ's own headroom
    /// took, as far as the song's peak allows (`room`, from [`Self::tone_plan`]).
    fn song_gain(gain_db: Option<f32>, tone: &Equalizer, room: f32) -> f32 {
        let gain = gain_db.unwrap_or(0.0);
        let lift = if tone.settings().enabled { -tone.auto_headroom_db() as f32 } else { 0.0 };
        10f32.powf((gain + lift.min(room)) / 20.0)
    }

    /// Sets the per-song tone for song `index` and its linear gain (`self.gain`).
    fn apply_tone(&mut self, index: usize, gain_db: Option<f32>) -> Option<ToneInfo> {
        let (settings, info, room) = self.tone_plan(index, gain_db);
        // A new song: its tone's level changes at the same sample as its gain, not gliding.
        self.tone_eq.set_settings_now(settings);
        self.gain = Self::song_gain(gain_db, &self.tone_eq, room);
        info
    }

    /// Sound Check's gain for song `index` in dB (None = leveling off).
    fn leveling_db(&self, index: usize) -> Option<f32> {
        match self.leveling.mode {
            loudness::LevelingMode::Off => None,
            _ => Some(
                self.leveling
                    .gain_db(&self.queue[index].gain, self.album_context(index)),
            ),
        }
    }

    /// Opens a song for decoding and records where it starts in the output. Returns the start position,
    /// or None when the device must be reopened first but `may_reopen` is false (gapless hand-over in
    /// exclusive mode to a song at another sample rate).
    fn open_source(&mut self, index: usize, secs: f64, may_reopen: bool) -> Result<Option<f64>> {
        let mut src = Source::open(&self.queue[index].path)?;
        if self.needs_reopen(src.rate, !may_reopen) {
            if !may_reopen {
                return Ok(None);
            }
            self.open_output(Some(src.rate));
        }
        // Where the song starts in the file (a song of a CUE sheet starts partway in), in frames,
        // and how far into the song itself decoding starts. The seek is sample-exact: a CUE song
        // starts on its first sample, not on the start of the FLAC frame holding it.
        let rate = src.rate;
        let first = self.queue[index].first_frame(rate);
        let frames = if secs > 0.5 || first > 0 {
            let want = first + (secs.max(0.0) * f64::from(rate)).round() as u64;
            match src.seek_frame(want) {
                Ok(reached) => reached.saturating_sub(first),
                // A song of a CUE sheet can't start at the beginning of its file instead.
                Err(e) if first > 0 => return Err(e),
                Err(e) => {
                    log::warn!("seek failed, playing from the start: {e:#}");
                    0
                }
            }
        } else {
            0
        };
        let offset = frames as f64 / f64::from(rate);
        self.begin_song(index, src, frames, !may_reopen);
        Ok(Some(offset))
    }

    /// Makes `src` (already at `frames` into song `index`) the song being decoded: its gain, tone
    /// and the place it starts in the output.
    fn begin_song(&mut self, index: usize, src: Source, frames: u64, gapless: bool) {
        let out_rate = self.out.as_ref().map(|o| o.rate).unwrap_or(src.rate);
        let gain_db = self.leveling_db(index);
        let tone = self.apply_tone(index, gain_db);
        log::info!(
            "song {index}: {} Hz from {:.1}s, device {} Hz{}, gain {:.1} dB",
            src.rate,
            frames as f64 / f64::from(src.rate),
            out_rate,
            if gapless { " (gapless)" } else { "" },
            20.0 * self.gain.log10()
        );
        self.boundaries.push(Boundary {
            // `written` already counts samples still waiting in `pending`.
            start: self.written,
            index,
            offset: frames as f64 / f64::from(src.rate),
            info: SourceInfo {
                codec: src.codec.clone(),
                sample_rate: src.rate,
                bits: src.bits,
                channels: src.channels,
            },
            resampling: src.rate != out_rate,
            gain_db,
            tone,
        });
        self.dec_frames = frames;
        self.dec = Some(src);
        self.dec_index = index;
        self.decoding = true;
        self.error = None;
    }

    // ---------- decoding ----------

    /// Keeps the ring buffer topped up, a few packets at a time.
    fn fill(&mut self) {
        for _ in 0..32 {
            match self.step() {
                Step::Continue => {}
                Step::Full => return,
                Step::EndOfSong => self.finish_track(),
            }
        }
    }

    fn step(&mut self) -> Step {
        let Some(out) = self.out.as_mut() else {
            return Step::Full;
        };
        let ch = out.channels;
        // First, whatever is already converted.
        if !self.pending.is_empty() {
            let room = out.prod.vacant_len() / ch * ch;
            let n = out
                .prod
                .push_slice(&self.pending[..room.min(self.pending.len())]);
            self.pending.drain(..n);
            if !self.pending.is_empty() {
                return Step::Full;
            }
        }
        if !self.decoding || out.prod.vacant_len() < 4096 * ch {
            return Step::Full;
        }
        let out_rate = out.rate;
        let rate = if !self.carry.is_empty() {
            // The rest of a song that faded in, already mapped, gain-adjusted and toned.
            self.mapped.clear();
            self.mapped.append(&mut self.carry);
            self.carry_rate
        } else {
            let Some(dec) = self.dec.as_mut() else {
                return Step::Full;
            };
            // A song of a CUE sheet ends where the next one starts, partway into the file.
            let room = self.queue.get(self.dec_index).and_then(|q| q.cue_frames(dec.rate));
            if room.is_some_and(|r| self.dec_frames >= r) {
                return Step::EndOfSong;
            }
            match dec.next_chunk() {
                Ok(Some((samples, rate, src_ch))) => {
                    let src_ch = src_ch.max(1);
                    let frames = (samples.len() / src_ch) as u64;
                    let used = match room {
                        Some(r) if self.dec_frames + frames > r => r - self.dec_frames,
                        _ => frames,
                    };
                    map_channels(&samples[..used as usize * src_ch], src_ch, ch, &mut self.mapped);
                    if used < frames {
                        // The rest belongs to the next song of the sheet.
                        dec.give_back(used as usize);
                    }
                    self.dec_frames += used;
                    if self.gain != 1.0 {
                        let g = self.gain;
                        self.mapped.iter_mut().for_each(|s| *s *= g);
                    }
                    // The song's own tone, before it's blended with anything.
                    self.tone_eq.set_format(ch, rate);
                    self.tone_eq.process(&mut self.mapped);
                    rate
                }
                Ok(None) => return Step::EndOfSong,
                Err(e) => {
                    log::warn!("decode error: {e:#}");
                    return Step::EndOfSong;
                }
            }
        };
        if let Some((done, len)) = self.fade_in {
            self.fade_in = fade_in_rest(&mut self.mapped, ch, done, len);
        }
        self.maybe_start_crossfade(rate, out_rate);
        if let Some(xf) = self.crossfade.as_mut() {
            xf.mix_into(&mut self.mapped, ch);
        }
        self.vocals.process(&mut self.mapped, ch, rate);
        self.eq.set_format(ch, rate);
        self.eq.process(&mut self.mapped);
        self.crossfeed.set_rate(rate);
        self.crossfeed.process(&mut self.mapped, ch);
        let before = self.pending.len();
        if rate != out_rate {
            let fresh = !self
                .resampler
                .as_ref()
                .is_some_and(|r| r.matches(rate, out_rate, ch));
            if fresh {
                if let Some(old) = &mut self.resampler {
                    let _ = old.flush(&mut self.pending);
                }
                self.resampler = Resampler::new(rate, out_rate, ch).ok();
            }
            match &mut self.resampler {
                Some(r) => {
                    if let Err(e) = r.push(&self.mapped, &mut self.pending) {
                        log::warn!("resample failed: {e:#}");
                    }
                }
                None => self.pending.extend_from_slice(&self.mapped),
            }
        } else {
            if let Some(mut old) = self.resampler.take() {
                let _ = old.flush(&mut self.pending);
            }
            self.pending.extend_from_slice(&self.mapped);
        }
        self.written += ((self.pending.len() - before) / ch) as u64;
        Step::Continue
    }

    /// Frames left of the song being decoded: from the CUE sheet, else the file's header, else the
    /// library's duration.
    fn frames_left(&self, rate: u32) -> Option<u64> {
        let current = self.queue.get(self.dec_index)?;
        let total = current
            .cue_frames(rate)
            .or_else(|| self.dec.as_ref().filter(|d| !current.is_cue() && d.rate == rate).and_then(|d| d.total_frames))
            .or_else(|| (current.duration > 0.0).then(|| (current.duration * f64::from(rate)) as u64))?;
        Some(total.saturating_sub(self.dec_frames))
    }

    /// Starts blending in the next song when the current one is about to end (crossfade on).
    /// Songs from the same album stay gapless, and so do songs at a different sample rate.
    fn maybe_start_crossfade(&mut self, rate: u32, out_rate: u32) {
        let cf = self.crossfade_settings;
        if !cf.enabled
            || self.crossfade.is_some()
            || self.crossfade_checked == Some(self.dec_index)
            || self.repeat == Repeat::One
            // The sleep timer ends playback at a boundary: don't blend the next song in first.
            || self.stop.is_some()
            || self.stop_mark == Some(self.dec_index)
        {
            return;
        }
        let Some(current) = self.queue.get(self.dec_index) else {
            return;
        };
        let Some(left) = self.frames_left(rate) else {
            return;
        };
        let remaining = left as f64 / f64::from(rate);
        if remaining > cf.seconds as f64 {
            return;
        }
        self.crossfade_checked = Some(self.dec_index);
        let Some(next) = self.auto_next(self.dec_index) else {
            return;
        };
        if next == self.dec_index
            || self.queue[next].album_id == current.album_id
            || self.queue[next].is_cue()
            || remaining < 1.0
        {
            return;
        }
        let Ok(src) = Source::open(&self.queue[next].path) else {
            return;
        };
        if src.rate != rate {
            return;
        }
        let gain_db = self.leveling_db(next);
        // The incoming song's own tone and gain, as it will have once it takes over.
        let (settings, tone_info, room) = self.tone_plan(next, gain_db);
        let ch = self.out.as_ref().map(|o| o.channels).unwrap_or(2);
        let tone = Equalizer::new(settings, ch, rate);
        let gain = Self::song_gain(gain_db, &tone, room);
        // The incoming song is "now playing" from the start of the blend.
        self.boundaries.push(Boundary {
            start: self.written,
            index: next,
            offset: 0.0,
            info: SourceInfo {
                codec: src.codec.clone(),
                sample_rate: src.rate,
                bits: src.bits,
                channels: src.channels,
            },
            resampling: src.rate != out_rate,
            gain_db,
            tone: tone_info,
        });
        log::info!("crossfading into song {next} over {remaining:.1}s");
        self.crossfade = Some(Crossfade {
            src,
            index: next,
            gain,
            tone,
            rate,
            buf: Vec::new(),
            frames: 0,
            pos: 0,
            len: left,
            scratch: Vec::new(),
        });
    }

    /// The decoder reached the end of a song: carry straight on with the next one (gapless).
    fn finish_track(&mut self) {
        // A crossfade was running: the incoming song simply takes over (its start is already recorded).
        if let Some(xf) = self.crossfade.take() {
            self.dec = Some(xf.src);
            self.dec_index = xf.index;
            self.gain = xf.gain;
            self.dec_frames = xf.frames;
            self.carry = xf.buf;
            self.carry_rate = xf.rate;
            // Its tone was already on it during the blend; the same filters carry on.
            self.tone_eq = xf.tone;
            // The outgoing song ended early (shorter than it said): finish the fade-in smoothly.
            self.fade_in = (xf.pos < xf.len).then_some((xf.pos, xf.len));
            return;
        }
        let mut next = self.auto_next(self.dec_index);
        // The next track of the same CUE sheet: carry on with the same decoder, which is already
        // exactly at its first sample (no reopening, no seek: truly gapless).
        if let (Some(n), Some(dec)) = (next, self.dec.as_ref()) {
            let (cur, rate) = (&self.queue[self.dec_index], dec.rate);
            if n != self.dec_index
                && self.queue[n].continues(cur, rate)
                && cur.cue_frames(rate).is_some_and(|r| self.dec_frames >= r)
            {
                let src = self.dec.take().expect("checked above");
                self.begin_song(n, src, 0, true);
                return;
            }
        }
        self.dec = None;
        let mut tries = 0;
        while let Some(n) = next {
            match self.open_source(n, 0.0, false) {
                Ok(Some(_)) => return,
                Ok(None) => {
                    // Exclusive mode, different sample rate: finish this song, then switch the device.
                    self.switch_pending = Some(n);
                    break;
                }
                Err(e) => {
                    log::warn!("skipping {}: {e:#}", self.queue[n].path.display());
                    self.skipped = Some(format!(
                        "Couldn't play “{}”: {e}",
                        file_name(&self.queue[n].path)
                    ));
                    tries += 1;
                    next = if tries < self.queue.len() {
                        self.manual_next(n)
                    } else {
                        None
                    };
                }
            }
        }
        self.decoding = false;
        if let Some(r) = &mut self.resampler {
            let ch = self.out.as_ref().map(|o| o.channels).unwrap_or(2);
            let before = self.pending.len();
            let _ = r.flush(&mut self.pending);
            self.written += ((self.pending.len() - before) / ch) as u64;
        }
    }

    // ---------- position / status ----------

    fn update_position(&mut self) {
        let Some(out) = self.out.as_ref() else { return };
        let consumed = self.shared.consumed.load(Ordering::Acquire);
        if let Some(pos) = self.boundaries.iter().rposition(|b| b.start <= consumed) {
            let (index, offset, start) = {
                let b = &self.boundaries[pos];
                (b.index, b.offset, b.start)
            };
            let old = self.play_index;
            if self.play_index != Some(index) {
                self.play_index = Some(index);
                self.dirty = true;
            }
            self.position = offset + (consumed - start) as f64 / out.rate as f64;
            if pos > 0 {
                self.boundaries.drain(..pos);
                // Playback just crossed into the next song (same index when it repeats).
                if self.stop_here(old, index) {
                    self.shared.playing.store(false, Ordering::Relaxed);
                    self.start_at(index, 0.0, false);
                    return;
                }
                self.dirty = true;
            }
        }
        let ended = !self.decoding && self.pending.is_empty() && consumed >= self.written;
        if ended && self.shared.playing.load(Ordering::Relaxed) && self.play_index.is_some() {
            self.drained();
        }
    }

    /// Everything decoded has been played and nothing more was queued up for the device: switch
    /// the device's rate for the next song (exclusive mode), or stop at the end of the queue.
    fn drained(&mut self) {
        if let Some(n) = self.switch_pending.take() {
            // The sleep timer's "end of this song / album" and "stop after this song" count here too.
            let play = !self.stop_here(self.play_index, n);
            if !play {
                self.shared.playing.store(false, Ordering::Relaxed);
            }
            self.start_at(n, 0.0, play);
            return;
        }
        // Queue finished: stop, ready to play again from the start of the last song. Songs that
        // were asked for (Autoplay, the next album) and arrive now play on; not after a stop the
        // listener or the sleep timer set.
        self.ran_dry = self.wants_more().is_some();
        self.stop = None;
        self.stop_mark = None;
        self.shared.playing.store(false, Ordering::Relaxed);
        if let Some(i) = self.play_index {
            self.start_at(i, 0.0, false);
            self.finished = true;
        }
    }

    fn emit(&mut self) {
        self.dirty = false;
        self.last_emit = Instant::now();
        // Peak since the last update; "clipping" stays lit for 3 s so it's noticeable.
        self.peak = self.shared.take_peak();
        let underrun = self.shared.underrun.swap(0, Ordering::Relaxed);
        let clipped = self.shared.clipped.swap(0, Ordering::Relaxed);
        if underrun > 0 || clipped > 0 {
            log::warn!(
                "output glitch at song {:?} {:.2}s: {underrun} frames missing (buffer ran empty), {clipped} samples clipped, decoding={}",
                self.play_index,
                self.position,
                self.decoding
            );
        }
        if self.peak >= CLIP_THRESHOLD {
            self.last_clip = Some(Instant::now());
        }
        let clipping = self
            .last_clip
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        let current = self.boundaries.first();
        let duration = self
            .play_index
            .and_then(|i| self.queue.get(i))
            .map(|q| q.duration)
            .unwrap_or(0.0);
        let eq_on = !self.eq.settings().is_transparent();
        let resampling = current.is_some_and(|b| b.resampling);
        let leveling = current.and_then(|b| b.gain_db);
        let tone = current.and_then(|b| b.tone.clone());
        let output = self
            .out
            .as_ref()
            .map(|o| {
                let remixing = current.is_some_and(|b| b.info.channels != o.channels);
                #[cfg(target_os = "android")]
                let device_volume_db = matches!(o.backend, output::Backend::Usb(_))
                    .then(usb_dac::volume_db)
                    .flatten();
                #[cfg(not(target_os = "android"))]
                let device_volume_db = None;
                let unaltered = !resampling
                    && !eq_on
                    && tone.is_none()
                    && !self.vocals.active()
                    && !self.crossfeed.enabled()
                    && !remixing
                    && leveling.is_none_or(|g| g == 0.0)
                    && self.volume >= 1.0
                    && f32::from_bits(self.shared.fade_gain.load(Ordering::Relaxed)) >= 1.0
                    && device_volume_db.is_none();
                OutputInfo {
                    device: o.device.clone(),
                    sample_rate: o.rate,
                    channels: o.channels,
                    mode: if o.exclusive() { "exclusive" } else { "shared" }.into(),
                    resampling,
                    eq: eq_on,
                    eq_headroom: self.eq.auto_headroom_db(),
                    crossfeed: self.crossfeed.enabled(),
                    leveling,
                    tone,
                    remixing,
                    unaltered,
                    // Shared mode always goes through the Windows mixer.
                    bit_perfect: unaltered && o.exclusive(),
                    bits: o.bits,
                    device_volume_db,
                    note: self.note.clone(),
                    peak_db: if self.peak > 0.0 {
                        20.0 * self.peak.log10()
                    } else {
                        -120.0
                    },
                    clipping,
                }
            })
            .unwrap_or_default();
        let status = Status {
            queue: self.queue.iter().map(|q| q.id.clone()).collect(),
            index: self.play_index,
            playing: self.shared.playing.load(Ordering::Relaxed),
            position: self
                .position
                .min(if duration > 0.0 { duration } else { f64::MAX }),
            duration,
            volume: self.volume,
            repeat: self.repeat,
            shuffle: self.shuffle,
            source: current.map(|b| b.info.clone()),
            output,
            // A skipped song is told once; a lasting problem (no device) comes first.
            error: self.error.clone().or_else(|| self.skipped.take()),
            stop_after: self.stop.clone(),
            stop_mark: self.stop_mark,
            autoplay_from: self.auto_start(),
            album_shuffle: self.albums,
            more: self.wants_more(),
            device: self.device.as_ref().map(|d| DeviceStatus {
                id: d.id.clone(),
                name: devices::display_name(&d.name),
                chosen: !self.stand_in && self.choice.as_ref().is_some_and(|c| c.id == d.id),
                profile: self.profile_seq,
            }),
            device_missing: self.missing.as_ref().map(|w| DeviceMissing {
                name: devices::display_name(&w.device.name),
                fallback: w.fallback.clone(),
                resume: w.resume,
            }),
            devices: self.devices_seq,
        };
        (self.on_status)(&status);
        *self.status.lock() = status;
    }
}

/// Carries on a crossfade's fade-in (equal-power, as in [`Crossfade::mix_into`]) over `samples`,
/// `done` of `len` frames in. Returns what's left of it (None once at full level).
fn fade_in_rest(samples: &mut [f32], ch: usize, done: u64, len: u64) -> Option<(u64, u64)> {
    let len = len.max(1);
    let mut f = done;
    for frame in samples.chunks_exact_mut(ch.max(1)) {
        if f >= len {
            break;
        }
        let g = ((f as f32 / len as f32) * std::f32::consts::FRAC_PI_2).sin();
        frame.iter_mut().for_each(|s| *s *= g);
        f += 1;
    }
    (f < len).then_some((f, len))
}

fn file_name(p: &std::path::Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Converts interleaved audio between channel counts.
///
/// Multichannel → stereo uses the ITU-R BS.775 downmix (centre and surrounds folded in at −3 dB),
/// scaled so the sum can't exceed full scale. Assumes the usual FL, FR, FC, LFE, BL, BR, (SL, SR) order;
/// the LFE channel is left out, as the standard recommends.
fn map_channels(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    out.clear();
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let frames = input.len() / from.max(1);
    out.reserve(frames * to);
    for f in input.chunks_exact(from) {
        match (from, to) {
            (1, _) => out.extend(std::iter::repeat_n(f[0], to)),
            (_, 1) => out.push(f.iter().sum::<f32>() / from as f32),
            (2, _) => {
                out.extend_from_slice(&f[..2]);
                out.extend(std::iter::repeat_n(0.0, to - 2));
            }
            (_, 2) => {
                let c = f.get(2).copied().unwrap_or(0.0);
                // Surrounds: channels after LFE (index 4..), alternating left/right.
                let (mut ls, mut rs) = (0.0, 0.0);
                for (i, s) in f.iter().enumerate().skip(4) {
                    if i % 2 == 0 { ls += s } else { rs += s }
                }
                let norm = 1.0 / (1.0 + K + K);
                out.push((f[0] + K * c + K * ls) * norm);
                out.push((f[1] + K * c + K * rs) * norm);
            }
            _ => {
                // Multichannel to another multichannel layout: keep what fits.
                out.extend(f.iter().take(to).copied());
                out.extend(std::iter::repeat_n(0.0, to.saturating_sub(from)));
            }
        }
    }
}

/// Puts `items` in a shuffled order: the balanced one (artists spread out, recently played songs
/// later; `lead` is the song playing before them) or a fully random one. Seeded from the clock.
fn shuffle_items(mode: crate::shuffle::ShuffleMode, items: &mut Vec<QueueItem>, lead: Option<&QueueItem>) {
    shuffle_items_with(mode, items, lead, crate::shuffle::clock_seed());
}

fn shuffle_items_with(mode: crate::shuffle::ShuffleMode, items: &mut Vec<QueueItem>, lead: Option<&QueueItem>, seed: u64) {
    use crate::shuffle::{Entry, ShuffleMode};
    let order = match mode {
        ShuffleMode::Random => crate::shuffle::random(items.len(), seed),
        ShuffleMode::Balanced => {
            let entries: Vec<Entry> = items
                .iter()
                .map(|q| Entry { artist: &q.artist, album: &q.album_id, last_played: q.last_played })
                .collect();
            let lead = lead.map(|l| (l.artist.as_str(), l.album_id.as_str()));
            crate::shuffle::balanced(&entries, lead, crate::history::now(), seed)
        }
    };
    crate::shuffle::apply(items, &order);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_keeps_the_centre_channel() {
        // 5.1 frame with only the centre (dialogue/vocals) playing.
        let frame = [0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        let mut out = Vec::new();
        map_channels(&frame, 6, 2, &mut out);
        assert!(out[0] > 0.2 && (out[0] - out[1]).abs() < 1e-6, "{out:?}");
        // Full-scale on every channel stays within full scale.
        map_channels(&[1.0; 6], 6, 2, &mut out);
        assert!(out.iter().all(|s| s.abs() <= 1.0 + 1e-6), "{out:?}");
    }

    #[test]
    fn mono_is_duplicated() {
        let mut out = Vec::new();
        map_channels(&[0.5, -0.5], 1, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
    }

    /// A worker with a queue of songs that don't exist (nothing here opens a file or a device),
    /// playing song `playing`. Songs are "a1", "a2" of album "A" and "b1", "b2" of album "B".
    fn worker(playing: usize) -> Worker {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let status = Arc::new(Mutex::new(Status::default()));
        let mut w = Worker::new(rx, status, Box::new(|_| {}), Arc::new(Shared::new(1.0)), 1.0, DspSettings::default(), Repeat::Off, false);
        w.queue = ["a1", "a2", "b1", "b2"].iter().map(|id| item(id)).collect();
        w.play_index = Some(playing);
        w.dec_index = playing;
        w
    }

    fn item(id: &str) -> QueueItem {
        QueueItem {
            id: id.into(),
            path: PathBuf::from(format!("{id}.flac")),
            duration: 200.0,
            album_id: id[..1].to_uppercase(),
            gain: SongGain::default(),
            tone: None,
            start: 0.0,
            end: None,
            artist: id[..1].into(),
            ..Default::default()
        }
    }

    fn ids(w: &Worker) -> Vec<&str> {
        w.queue.iter().map(|q| q.id.as_str()).collect()
    }

    #[test]
    fn stops_after_the_song_or_the_count() {
        let mut w = worker(0);
        w.stop = Some(StopAfter::Songs { left: 1 });
        assert!(w.stop_here(Some(0), 1), "end of the song");
        w.stop = Some(StopAfter::Songs { left: 3 });
        assert!(!w.stop_here(Some(0), 1));
        assert!(!w.stop_here(Some(1), 2));
        assert_eq!(w.stop, Some(StopAfter::Songs { left: 1 }));
        assert!(w.stop_here(Some(2), 3));
        w.stop = None;
        assert!(!w.stop_here(Some(0), 1));
    }

    #[test]
    fn stops_when_the_album_changes() {
        let mut w = worker(0);
        w.stop = Some(StopAfter::Album);
        assert!(!w.stop_here(Some(0), 1), "a1 to a2: same album");
        assert!(w.stop_here(Some(1), 2), "a2 to b1: album over");
        assert_eq!(w.stop, None, "used up");
        // Repeating a song never ends the album.
        w.stop = Some(StopAfter::Album);
        assert!(!w.stop_here(Some(1), 1));
    }

    #[test]
    fn clearing_keeps_the_playing_song() {
        let mut w = worker(1);
        w.handle(Cmd::ClearUpNext);
        assert_eq!(ids(&w), ["a1", "a2"]);
        assert_eq!(w.play_index, Some(1));
    }

    #[test]
    fn songs_move_among_the_waiting_ones_only() {
        let mut w = worker(0);
        w.handle(Cmd::Move { from: 3, to: 1 });
        assert_eq!(ids(&w), ["a1", "b2", "a2", "b1"]);
        w.handle(Cmd::Move { from: 1, to: 3 });
        assert_eq!(ids(&w), ["a1", "a2", "b1", "b2"]);
        // The playing song and out-of-range places are left alone.
        w.handle(Cmd::Move { from: 0, to: 2 });
        w.handle(Cmd::Move { from: 2, to: 0 });
        w.handle(Cmd::Move { from: 2, to: 9 });
        assert_eq!(ids(&w), ["a1", "a2", "b1", "b2"]);
        assert_eq!(w.play_index, Some(0));
    }

    #[test]
    fn removed_songs_can_be_put_back() {
        let mut w = worker(1);
        w.handle(Cmd::Remove(2));
        assert_eq!(ids(&w), ["a1", "a2", "b2"]);
        w.handle(Cmd::Insert { at: 2, items: vec![item("b1")] });
        assert_eq!(ids(&w), ["a1", "a2", "b1", "b2"]);
        // Never before the playing song.
        w.handle(Cmd::Insert { at: 0, items: vec![item("a3")] });
        assert_eq!(ids(&w), ["a1", "a2", "a3", "b1", "b2"]);
        assert_eq!(w.play_index, Some(1));
    }

    #[test]
    fn cue_positions_are_whole_samples() {
        // 20:34.56 + 56 CD frames: f64 seconds times the rate lands just below the sample.
        let secs = 1234.0 + 56.0 / 75.0;
        assert_eq!(cue_frame(secs, 44_100), (1234 * 75 + 56) * 588);
        assert_eq!(cue_frame(secs, 48_000), (1234 * 75 + 56) * 640);
        assert_eq!(cue_frame(secs, 96_000), (1234 * 75 + 56) * 1280);
        // Not a whole CD frame (or a rate that isn't a multiple of 75): the nearest sample.
        assert_eq!(cue_frame(1.00001, 44_100), 44_100);
        assert_eq!(cue_frame(1.5, 22_050 + 1), 33_077);
        // Track 2 of a sheet carries on from track 1 of the same file.
        let mut t1 = item("a1");
        t1.end = Some(secs);
        let mut t2 = item("a2");
        t2.path = t1.path.clone();
        t2.start = secs;
        assert!(t2.continues(&t1, 44_100));
        assert_eq!(t1.cue_frames(44_100), Some((1234 * 75 + 56) * 588));
        assert!(!item("b1").continues(&t1, 44_100));
    }

    #[test]
    fn sleep_timer_stops_across_a_rate_switch() {
        // Exclusive mode, the next song needs another rate: the device switches once this song
        // has played out. "End of this song" must stop there too.
        let mut w = worker(0);
        w.stop = Some(StopAfter::Songs { left: 1 });
        w.switch_pending = Some(1);
        w.shared.playing.store(true, Ordering::Relaxed);
        w.drained();
        assert_eq!(w.stop, None, "the rule was used up");
        assert!(!w.shared.playing.load(Ordering::Relaxed));
        assert_eq!(w.play_index, Some(1), "paused at the start of the next song");
        // "After 2 songs" counts the boundary and plays on.
        let mut w = worker(0);
        w.stop = Some(StopAfter::Songs { left: 2 });
        w.switch_pending = Some(1);
        w.drained();
        assert_eq!(w.stop, Some(StopAfter::Songs { left: 1 }));
    }

    /// A worker whose songs are real files (short WAVs whose samples count their own frame, see
    /// `source::tests::position_wav`) and whose output is a ring buffer the test reads instead of
    /// a device. Returns the worker, the ring's reading end and the folder to remove afterwards.
    fn real_worker(name: &str, ids: &[&str], frames: u32) -> (Worker, ringbuf::HeapCons<f32>, PathBuf) {
        use ringbuf::traits::Split;
        let dir = std::env::temp_dir().join(format!("cadence-worker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = worker(0);
        w.queue = ids
            .iter()
            .map(|id| {
                let mut q = item(id);
                q.path = dir.join(format!("{id}.wav"));
                if !q.path.exists() {
                    source::tests::position_wav(&q.path, frames);
                }
                q
            })
            .collect();
        // Untouched samples: no loudness gain (the rest of the chain is off by default).
        w.leveling.mode = loudness::LevelingMode::Off;
        let (prod, cons) = ringbuf::HeapRb::<f32>::new(1 << 20).split();
        w.out = Some(Output { backend: output::Backend::Null, prod, rate: 44_100, channels: 2, device: "test".into(), bits: None });
        (w, cons, dir)
    }

    /// Decodes until `done` says so (or the decoder has nothing left), keeping what reached the
    /// "device" in `got`.
    fn decode_until(w: &mut Worker, cons: &mut ringbuf::HeapCons<f32>, got: &mut Vec<f32>, done: impl Fn(&Worker) -> bool) {
        use ringbuf::traits::Consumer;
        // One packet at a time (what `fill` does 32 times over), so `done` sees every step.
        for _ in 0..100_000 {
            if done(w) || (!w.decoding && w.pending.is_empty()) {
                break;
            }
            match w.step() {
                Step::Continue => {}
                Step::Full => got.extend(cons.pop_iter()),
                Step::EndOfSong => w.finish_track(),
            }
        }
        got.extend(cons.pop_iter());
    }

    /// The frame number a left-channel sample carries.
    fn frame_of(s: f32) -> i64 {
        (s * 32_768.0).round() as i64
    }

    #[test]
    fn queue_edits_follow_a_pending_switch() {
        let (mut w, mut cons, dir) = real_worker("switch", &["a1", "a2", "b1", "b2", "a3"], 6_000);
        let a3 = w.queue.pop().unwrap();
        w.start_at(0, 0.0, false);
        assert_eq!((w.dec_index, w.decoding, w.skipped.as_deref()), (0, true, None));
        // a1 has played out; b1 (index 2) waits for the device to switch rate.
        w.switch_pending = Some(2);
        w.handle(Cmd::Remove(1));
        assert_eq!(w.switch_pending, Some(1), "still b1");
        // A song put in front of it is now the next one: prepare that instead.
        w.handle(Cmd::Insert { at: 1, items: vec![a3] });
        assert_eq!(w.switch_pending, None);
        assert_eq!(ids(&w), ["a1", "a3", "b1", "b2"]);
        // Restarted on the playing song, from where it was, with nothing else prepared.
        assert_eq!((w.play_index, w.dec_index, w.decoding), (Some(0), 0, true));
        assert_eq!(w.boundaries.iter().map(|b| b.index).collect::<Vec<_>>(), [0]);
        assert_eq!(w.skipped, None);
        // And what the decoder takes up after a1 is a3, not b1.
        let mut got = Vec::new();
        decode_until(&mut w, &mut cons, &mut got, |w| w.boundaries.len() > 1);
        assert_eq!(w.dec_index, 1);
        assert_eq!(w.queue[w.dec_index].id, "a3");
        assert_eq!(w.boundaries.iter().map(|b| (b.index, b.start)).collect::<Vec<_>>(), [(0, 0), (1, 6_000)]);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previous_goes_round_under_repeat_all() {
        let mut w = worker(0);
        w.repeat = Repeat::All;
        w.handle(Cmd::Prev);
        assert_eq!(w.play_index, Some(3));
        let mut w = worker(0);
        w.handle(Cmd::Prev);
        assert_eq!(w.play_index, Some(0), "without repeat: the first song again");
    }

    #[test]
    fn play_after_the_end_goes_on_with_added_songs() {
        let mut w = worker(3);
        w.finished = true;
        w.handle(Cmd::Enqueue { items: vec![item("c1")], next: false });
        w.handle(Cmd::Play);
        assert_eq!(w.play_index, Some(4), "the added song, not b2 again");
        // Without anything added, Play starts the last song again.
        let mut w = worker(3);
        w.finished = true;
        w.handle(Cmd::Play);
        assert_eq!(w.play_index, Some(3));
    }

    #[test]
    fn repeat_change_redoes_a_prepared_next_song() {
        // Song a2 is already being decoded after a1 (a1 is all in the buffer, still playing):
        // Repeat One means a1 again instead.
        let (mut w, mut cons, dir) = real_worker("repeat", &["a1", "a2"], 6_000);
        w.start_at(0, 0.0, false);
        let mut got = Vec::new();
        decode_until(&mut w, &mut cons, &mut got, |w| w.dec_index == 1);
        assert_eq!((w.play_index, w.dec_index, w.decoding), (Some(0), 1, true), "a2 prepared");
        w.handle(Cmd::SetRepeat(Repeat::One));
        assert_eq!((w.play_index, w.dec_index, w.decoding), (Some(0), 0, true), "a1 again");
        assert_eq!(w.boundaries.iter().map(|b| b.index).collect::<Vec<_>>(), [0]);
        // ...and after it, a1 once more (not a2).
        decode_until(&mut w, &mut cons, &mut got, |w| w.boundaries.len() > 1);
        assert_eq!(w.boundaries.iter().map(|b| (b.index, b.start)).collect::<Vec<_>>(), [(0, 0), (0, 6_000)]);
        assert_eq!(w.skipped, None);

        // Nothing prepared yet (a1 still being decoded): nothing to redo, decoding goes on.
        let (mut w2, mut cons2, dir2) = real_worker("repeat-2", &["a1", "a2"], 6_000);
        w2.start_at(0, 0.0, false);
        assert!(matches!(w2.step(), Step::Continue));
        let written = w2.written;
        assert!(written > 0 && w2.dec_index == 0);
        w2.handle(Cmd::SetRepeat(Repeat::One));
        assert_eq!((w2.written, w2.dec_index, w2.boundaries.len()), (written, 0, 1), "not restarted");
        let mut got2 = Vec::new();
        decode_until(&mut w2, &mut cons2, &mut got2, |w| w.boundaries.len() > 1);
        assert_eq!(w2.boundaries.last().map(|b| b.index), Some(0), "the new rule picks what follows");
        drop((w, w2));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn seeking_a_flac_starts_on_the_exact_sample() {
        let (mut w, mut cons, dir) = real_worker("flac-seek", &[], 0);
        let path = dir.join("song.flac");
        source::tests::position_flac(&path, 20 * 4096);
        let mut q = item("f1");
        q.path = path;
        w.queue = vec![q];
        // 1.0 s is frame 44 100, partway into the 11th FLAC frame (4096 frames each).
        w.start_at(0, 1.0, false);
        assert_eq!((w.position, w.dec_frames), (1.0, 44_100));
        let mut got = Vec::new();
        decode_until(&mut w, &mut cons, &mut got, |_| false);
        assert_eq!(got.len() / 2, 20 * 4096 - 44_100, "every frame from there to the end, once");
        assert_eq!(frame_of(got[0]), 44_100 % 30_000);
        assert_eq!(frame_of(got[1]), -(44_100 % 30_000));
        assert!(got.chunks(2).enumerate().all(|(i, f)| frame_of(f[0]) == ((44_100 + i) % 30_000) as i64));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cue_sheet_plays_on_across_its_track_boundary() {
        // One 20 000-frame file, two songs of its sheet: track 2 starts at 0.2 s (15 CD frames =
        // frame 8 820), partway into a decoded chunk (WAV packets are 1 152 frames).
        let (mut w, mut cons, dir) = real_worker("cue", &["c"], 20_000);
        let file = w.queue[0].path.clone();
        let mut t1 = item("t1");
        t1.path = file.clone();
        t1.end = Some(0.2);
        let mut t2 = item("t2");
        t2.path = file;
        t2.start = 0.2;
        w.queue = vec![t1, t2];
        w.start_at(0, 0.0, false);
        let mut got = Vec::new();
        decode_until(&mut w, &mut cons, &mut got, |_| false);
        // Track 2 begins on the very next sample: nothing missing, nothing twice.
        assert_eq!(w.boundaries.iter().map(|b| (b.index, b.start, b.offset)).collect::<Vec<_>>(), [(0, 0, 0.0), (1, 8_820, 0.0)]);
        assert_eq!(got.len() / 2, 20_000);
        assert!(got.chunks(2).enumerate().all(|(i, f)| frame_of(f[0]) == i as i64 && frame_of(f[1]) == -(i as i64)));
        assert_eq!(w.skipped, None);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_skipped_song_is_reported_once() {
        let mut w = worker(0);
        w.skipped = Some("Couldn't play “x”".into());
        w.error = None;
        w.emit();
        assert_eq!(w.status.lock().error.as_deref(), Some("Couldn't play “x”"));
        w.emit();
        assert_eq!(w.status.lock().error, None);
    }

    /// A queue of `n` songs by a handful of artists ("x0".."x{n-1}", artists by number mod 5).
    fn many(n: usize) -> Vec<QueueItem> {
        (0..n)
            .map(|i| {
                let mut q = item(&format!("x{i}"));
                q.artist = format!("artist{}", i % 5);
                q.album_id = format!("album{}", i % 7);
                q
            })
            .collect()
    }

    #[test]
    fn shuffle_off_puts_the_exact_order_back_also_after_a_restart() {
        for mode in [crate::shuffle::ShuffleMode::Balanced, crate::shuffle::ShuffleMode::Random] {
            let mut w = worker(0);
            w.shuffle_mode = mode;
            w.queue = many(40);
            let original: Vec<String> = w.queue.iter().map(|q| q.id.clone()).collect();
            w.handle(Cmd::SetShuffle(true));
            assert_eq!(w.queue[0].id, "x0", "the playing song stays first");
            assert_ne!(ids(&w), original.iter().map(String::as_str).collect::<Vec<_>>(), "shuffled");
            w.handle(Cmd::SetShuffle(false));
            assert_eq!(ids(&w), original.iter().map(String::as_str).collect::<Vec<_>>(), "{mode:?}");

            // Shuffled, saved with the session, Cadence closed and opened again: still undone.
            w.handle(Cmd::SetShuffle(true));
            let queue: Vec<String> = w.queue.iter().map(|q| q.id.clone()).collect();
            let saved = crate::shuffle::positions(&queue, Some(0), &w.memory.lock());
            assert_eq!(saved.len(), 39);
            let json = serde_json::to_string(&crate::state::Session { queue: queue.clone(), index: Some(0), position: 12.0, unshuffled: saved, autoplay_from: None }).unwrap();
            let back: crate::state::Session = serde_json::from_str(&json).unwrap();
            let mut w2 = worker(0);
            w2.shuffle = true;
            let items: Vec<QueueItem> = back.queue.iter().map(|id| w.queue.iter().find(|q| &q.id == id).unwrap().clone()).collect();
            w2.handle(Cmd::Restore { items, index: 0, position: 12.0, unshuffled: crate::shuffle::from_positions(&back.queue, &back.unshuffled), autoplay_from: None });
            w2.handle(Cmd::SetShuffle(false));
            assert_eq!(ids(&w2), original.iter().map(String::as_str).collect::<Vec<_>>(), "after a restart, {mode:?}");
        }
        // A session file from before has no saved order: it still loads.
        let old: crate::state::Session = serde_json::from_str(r#"{"queue":["a"],"index":0,"position":1.5}"#).unwrap();
        assert!(old.unshuffled.is_empty() && old.autoplay_from.is_none());
    }

    #[test]
    fn a_shuffled_list_starts_with_the_chosen_song_and_spreads_its_artists() {
        let mut w = worker(0);
        w.shuffle = true;
        let items = many(50);
        w.handle(Cmd::Load { items, index: 7 });
        assert_eq!(w.queue[0].id, "x7");
        assert_eq!(w.queue.len(), 50);
        // Five artists, ten songs each: never one artist twice in a row.
        assert!(w.queue.windows(2).all(|p| p[0].artist != p[1].artist), "{:?}", ids(&w));
        // A shuffled list (the Shuffle buttons) is shuffled whatever the setting, and with shuffle on
        // its own order comes back.
        w.handle(Cmd::LoadShuffled(many(50)));
        assert!(w.queue.windows(2).all(|p| p[0].artist != p[1].artist));
        w.handle(Cmd::SetShuffle(false));
        assert_eq!(w.queue.iter().map(|q| q.id.clone()).collect::<Vec<_>>()[1..], many(50).iter().map(|q| q.id.clone()).filter(|id| *id != w.queue[0].id).collect::<Vec<_>>()[..]);
    }

    #[test]
    fn stop_after_a_song_fires_there_and_is_used_up() {
        let mut w = worker(0);
        w.handle(Cmd::StopAt(Some(1)));
        assert!(!w.stop_here(Some(0), 1), "not after a1");
        assert!(w.stop_here(Some(1), 2), "after a2");
        assert_eq!(w.stop_mark, None);
        assert!(!w.stop_here(Some(2), 3));
        // A sleep rule waiting for later is left alone when the mark stops playback.
        w.stop_mark = Some(2);
        w.stop = Some(StopAfter::Songs { left: 3 });
        assert!(w.stop_here(Some(2), 3));
        assert_eq!(w.stop, Some(StopAfter::Songs { left: 3 }));
        // Only on the playing song or later ones.
        let mut w = worker(2);
        w.handle(Cmd::StopAt(Some(1)));
        assert_eq!(w.stop_mark, None);
        w.handle(Cmd::StopAt(Some(9)));
        assert_eq!(w.stop_mark, None);
    }

    #[test]
    fn stop_after_a_song_follows_it_through_queue_edits() {
        let mut w = worker(0);
        w.queue = many(10);
        w.handle(Cmd::StopAt(Some(4)));
        let marked = |w: &Worker| w.stop_mark.map(|m| w.queue[m].id.clone());
        w.handle(Cmd::Remove(2));
        assert_eq!((w.stop_mark, marked(&w).as_deref()), (Some(3), Some("x4")));
        w.handle(Cmd::Insert { at: 1, items: vec![item("n1"), item("n2")] });
        assert_eq!((w.stop_mark, marked(&w).as_deref()), (Some(5), Some("x4")));
        w.handle(Cmd::Move { from: 5, to: 8 });
        assert_eq!((w.stop_mark, marked(&w).as_deref()), (Some(8), Some("x4")));
        w.handle(Cmd::Move { from: 2, to: 9 });
        assert_eq!(marked(&w).as_deref(), Some("x4"));
        w.handle(Cmd::SetShuffle(true));
        assert_eq!(marked(&w).as_deref(), Some("x4"), "shuffled, the mark stays on its song");
        w.handle(Cmd::SetShuffle(false));
        assert_eq!(marked(&w).as_deref(), Some("x4"));
        w.handle(Cmd::Enqueue { items: vec![item("n3")], next: true });
        assert_eq!(marked(&w).as_deref(), Some("x4"));
        // Removing the marked song removes the mark.
        let m = w.stop_mark.unwrap();
        w.handle(Cmd::Remove(m));
        assert_eq!(w.stop_mark, None);
        // So does skipping past it, and clearing Up Next.
        w.handle(Cmd::StopAt(Some(3)));
        w.handle(Cmd::Jump(5));
        assert_eq!(w.stop_mark, None);
        w.handle(Cmd::StopAt(Some(7)));
        w.handle(Cmd::ClearUpNext);
        assert_eq!(w.stop_mark, None);
        // A new list starts without one.
        w.handle(Cmd::StopAt(Some(5)));
        w.handle(Cmd::Load { items: many(3), index: 0 });
        assert_eq!(w.stop_mark, None);
    }

    #[test]
    fn stop_after_a_song_pauses_at_the_next_one_in_the_output() {
        // Gapless: a2 is already in the buffer behind a1; once the device reaches it, playback
        // pauses at its start.
        let (mut w, mut cons, dir) = real_worker("mark", &["a1", "a2", "b1"], 6_000);
        w.start_at(0, 0.0, true);
        w.handle(Cmd::StopAt(Some(0)));
        let mut got = Vec::new();
        decode_until(&mut w, &mut cons, &mut got, |w| w.boundaries.len() > 1);
        assert_eq!(w.boundaries.iter().map(|b| b.index).collect::<Vec<_>>(), [0, 1]);
        w.shared.consumed.store(6_100, Ordering::Release);
        w.update_position();
        assert!(!w.shared.playing.load(Ordering::Relaxed), "paused");
        assert_eq!((w.play_index, w.position, w.stop_mark), (Some(1), 0.0, None), "at the start of a2, the mark used up");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_after_a_song_holds_across_a_rate_switch() {
        // Exclusive mode: b1 needs the device at another rate, so it waits until a1 has played
        // out (what `finish_track` leaves when `open_source` says the device must switch).
        let (mut w, mut cons, dir) = real_worker("mark-switch", &["a1", "b1"], 6_000);
        w.start_at(0, 0.0, true);
        w.handle(Cmd::StopAt(Some(0)));
        let mut got = Vec::new();
        loop {
            match w.step() {
                Step::EndOfSong => break,
                Step::Full => got.extend(ringbuf::traits::Consumer::pop_iter(&mut cons)),
                Step::Continue => {}
            }
        }
        w.dec = None;
        w.decoding = false;
        w.switch_pending = Some(1);
        // Queue edits meanwhile keep it on a1.
        w.handle(Cmd::Enqueue { items: vec![item("c1")], next: false });
        assert_eq!(w.stop_mark, Some(0));
        // The device has played everything: the switch, and the stop.
        w.shared.consumed.store(w.written, Ordering::Release);
        w.pending.clear();
        w.update_position();
        assert!(!w.shared.playing.load(Ordering::Relaxed), "paused, not playing b1");
        assert_eq!((w.play_index, w.stop_mark, w.switch_pending), (Some(1), None, None));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autoplay_asks_near_the_end_and_only_when_it_should() {
        let mut w = worker(2);
        w.autoplay = true;
        w.shared.playing.store(true, Ordering::Relaxed);
        assert_eq!(w.wants_more(), Some(More::Similar), "the next-to-last song plays");
        w.play_index = Some(1);
        assert_eq!(w.wants_more(), None, "two songs still to come");
        w.play_index = Some(3);
        assert_eq!(w.wants_more(), Some(More::Similar));
        // Not while paused (the listener stopped), with a sleep rule or a stop mark set, or repeating.
        w.shared.playing.store(false, Ordering::Relaxed);
        assert_eq!(w.wants_more(), None);
        w.shared.playing.store(true, Ordering::Relaxed);
        w.stop = Some(StopAfter::Songs { left: 1 });
        assert_eq!(w.wants_more(), None);
        w.stop = None;
        w.stop_mark = Some(3);
        assert_eq!(w.wants_more(), None);
        w.stop_mark = None;
        for r in [Repeat::All, Repeat::One] {
            w.repeat = r;
            assert_eq!(w.wants_more(), None, "{r:?}");
        }
        w.repeat = Repeat::Off;
        w.autoplay = false;
        assert_eq!(w.wants_more(), None, "setting off");
    }

    #[test]
    fn autoplay_songs_come_after_the_listeners_and_clear_in_one_go() {
        let mut w = worker(3);
        w.autoplay = true;
        w.shared.playing.store(true, Ordering::Relaxed);
        let batch: Vec<QueueItem> = (0..10).map(|i| item(&format!("c{i}"))).collect();
        w.handle(Cmd::Extend { items: batch, auto: true });
        assert_eq!((w.queue.len(), w.auto_start()), (14, Some(4)));
        assert_eq!(w.wants_more(), None, "a batch is enough for now");
        // The listener's songs go before Autoplay's.
        w.handle(Cmd::Enqueue { items: vec![item("d1")], next: false });
        assert_eq!(w.queue[4].id, "d1");
        assert_eq!(w.auto_start(), Some(5));
        // Near the end of the batch it asks again.
        w.play_index = Some(13);
        assert_eq!(w.wants_more(), Some(More::Similar));
        w.play_index = Some(3);
        w.handle(Cmd::ClearAutoplay);
        assert_eq!(ids(&w), ["a1", "a2", "b1", "b2", "d1"]);
        assert_eq!(w.auto_start(), None);
        w.play_index = Some(4);
        w.dec_index = 4;
        assert_eq!(w.wants_more(), None, "cleared: not again straight away");
        w.handle(Cmd::Enqueue { items: vec![item("e1")], next: false });
        assert_eq!(w.wants_more(), Some(More::Similar), "songs added: it may carry on after them");
        // Turning Autoplay off takes its songs out too.
        w.handle(Cmd::Extend { items: vec![item("c1")], auto: true });
        w.handle(Cmd::SetAutoplay(false));
        assert_eq!(w.queue.len(), 6);
    }

    #[test]
    fn autoplay_songs_after_the_queue_ran_out_play_but_not_after_a_stop() {
        let (mut w, _cons, dir) = real_worker("autoplay-end", &["a1", "a2", "b1"], 3_000);
        let more = w.queue.pop().unwrap();
        w.autoplay = true;
        w.start_at(1, 0.0, true);
        // The queue played out before the songs arrived: they play.
        w.drained();
        assert!(w.finished && w.ran_dry && !w.shared.playing.load(Ordering::Relaxed));
        w.handle(Cmd::Extend { items: vec![more.clone()], auto: true });
        assert_eq!(w.play_index, Some(2));
        assert!(w.shared.playing.load(Ordering::Relaxed), "playing on");
        // The sleep timer's "end of this song" stopped it: they wait in the queue.
        w.queue.truncate(2);
        w.start_at(1, 0.0, true);
        w.stop = Some(StopAfter::Songs { left: 1 });
        w.drained();
        assert!(!w.ran_dry);
        w.handle(Cmd::Extend { items: vec![more], auto: true });
        assert_eq!(w.play_index, Some(1));
        assert!(!w.shared.playing.load(Ordering::Relaxed));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn album_shuffle_asks_for_the_next_album_before_the_queue_runs_dry() {
        let mut w = worker(0);
        w.shuffle = true;
        w.autoplay = true;
        w.handle(Cmd::PlayAlbums { items: vec![item("a1"), item("a2"), item("a3")], more: true });
        assert_eq!(ids(&w), ["a1", "a2", "a3"], "in order, though shuffle is on");
        w.shared.playing.store(true, Ordering::Relaxed);
        w.play_index = Some(0);
        assert_eq!(w.wants_more(), Some(More::Albums), "the next album, right away");
        w.handle(Cmd::Extend { items: vec![item("b1"), item("b2")], auto: false });
        assert_eq!(w.wants_more(), None, "album B waits behind A");
        assert!(w.album_context(3) && w.album_context(4), "B plays as an album (gapless, album leveling)");
        assert!(w.queue.iter().all(|q| !q.auto));
        w.play_index = Some(3);
        assert_eq!(w.wants_more(), Some(More::Albums), "B started: one more");
        w.repeat = Repeat::One;
        assert_eq!(w.wants_more(), None);
        w.repeat = Repeat::All;
        assert_eq!(w.wants_more(), Some(More::Albums), "a new round under Repeat All");
        // Playing something else ends it; "Surprise me" plays one album only.
        w.handle(Cmd::Load { items: vec![item("c1")], index: 0 });
        assert!(!w.albums);
        w.handle(Cmd::PlayAlbums { items: vec![item("d1"), item("d2")], more: false });
        assert!(!w.albums);
        assert_eq!(ids(&w), ["d1", "d2"]);
    }

    #[test]
    fn a_cut_short_fade_in_carries_on() {
        let mut x = vec![1.0f32; 2 * 100];
        let rest = fade_in_rest(&mut x, 2, 50, 120);
        // Starts where the blend was (sin of 50/120 of a quarter turn), rises, reaches full.
        assert!((x[0] - (50.0f32 / 120.0 * std::f32::consts::FRAC_PI_2).sin()).abs() < 1e-6);
        let left: Vec<f32> = x.iter().step_by(2).copied().collect();
        assert!(left.windows(2).all(|p| p[1] >= p[0]));
        assert_eq!(x[2 * 70], 1.0, "full level from frame 120 on");
        assert_eq!(rest, None);
        let mut y = vec![1.0f32; 2 * 10];
        assert_eq!(fade_in_rest(&mut y, 2, 0, 120), Some((10, 120)));
    }

    // ---------- output devices (a stand-in device list; nothing real is listed or opened) ----------

    thread_local! {
        static DEVICES: std::cell::RefCell<devices::Present> = Default::default();
    }

    fn fake_devices() -> Result<devices::Present> {
        Ok(DEVICES.with(|d| d.borrow().clone()))
    }

    /// The devices "connected" now: (id, is the default).
    fn connect(list: &[(&str, bool)]) {
        let devices: Vec<devices::Listed> = list.iter().map(|(id, def)| devices::Listed { id: id.to_string(), name: format!("{id} name"), is_default: *def }).collect();
        let default = devices.iter().find(|d| d.is_default).map(|d| DeviceRef { id: d.id.clone(), name: d.name.clone() });
        DEVICES.with(|d| *d.borrow_mut() = devices::Present { devices, default });
    }

    fn dref(id: &str) -> DeviceRef {
        DeviceRef { id: id.into(), name: format!("{id} name") }
    }

    /// `real_worker`, whose outputs are silent stand-ins on the stand-in devices, playing song 1
    /// from 0.1 s on `choice`.
    fn device_worker(name: &str, choice: Option<&str>) -> (Worker, PathBuf) {
        let (mut w, _cons, dir) = real_worker(name, &["a1", "a2", "b1"], 100_000);
        w.list_devices = fake_devices;
        w.null_output = true;
        w.choice = choice.map(dref);
        w.out = None;
        w.start_at(1, 0.1, true);
        (w, dir)
    }

    fn on(w: &Worker) -> Option<&str> {
        w.device.as_ref().map(|d| d.id.as_str())
    }

    fn playing(w: &Worker) -> bool {
        w.shared.playing.load(Ordering::Relaxed)
    }

    #[test]
    fn a_chosen_device_is_kept_when_the_default_moves() {
        connect(&[("dac", false), ("hdmi", true)]);
        let (mut w, dir) = device_worker("dev-keep", Some("dac"));
        assert_eq!((on(&w), playing(&w), w.opens), (Some("dac"), true, 1));
        // A monitor or a Bluetooth headset becomes the default: playback stays on the DAC.
        connect(&[("dac", false), ("hdmi", false), ("bt", true)]);
        w.devices_changed(true);
        assert_eq!((on(&w), w.opens), (Some("dac"), 1), "not reopened");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_system_default_is_followed_when_nothing_is_chosen() {
        connect(&[("speakers", true), ("dac", false)]);
        let (mut w, dir) = device_worker("dev-follow", None);
        assert_eq!(on(&w), Some("speakers"));
        connect(&[("speakers", false), ("dac", true)]);
        w.devices_changed(true);
        assert_eq!((on(&w), playing(&w), w.play_index), (Some("dac"), true, Some(1)));
        // Something else changed (a microphone came): no reopening.
        let opens = w.opens;
        w.devices_changed(false);
        assert_eq!(w.opens, opens);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choosing_another_device_keeps_the_place_and_the_queue() {
        connect(&[("speakers", true), ("dac", false)]);
        let (mut w, dir) = device_worker("dev-switch", None);
        w.position = 1.5;
        w.handle(Cmd::SetDevice(Some(dref("dac"))));
        assert_eq!((on(&w), playing(&w), w.play_index, w.queue.len()), (Some("dac"), true, Some(1), 3));
        assert!((w.position - 1.5).abs() < 0.01, "from the same place: {}", w.position);
        // Back to the system default.
        w.handle(Cmd::SetDevice(None));
        assert_eq!(on(&w), Some("speakers"));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unplugged_dac_pauses_and_playback_carries_on_when_it_is_back() {
        connect(&[("speakers", true), ("dac", false)]);
        let (mut w, dir) = device_worker("dev-unplug", Some("dac"));
        // Unplugged while playing: the stream fails and is reopened, but the DAC isn't there.
        connect(&[("speakers", true)]);
        w.recover_output();
        assert!(!playing(&w), "paused, not playing on the laptop's speakers");
        assert!(w.out.is_none());
        assert_eq!(on(&w), Some("dac"), "nothing else was opened");
        w.emit();
        let missing = w.status.lock().device_missing.clone().expect("told");
        assert_eq!((missing.name.as_str(), missing.fallback.as_deref(), missing.resume), ("dac name", Some("speakers name"), true));
        // Play pressed meanwhile: still nothing on the speakers.
        w.handle(Cmd::Play);
        assert!(!playing(&w) && w.out.is_none());
        // Back soon: playing again, on the DAC, from the same song.
        connect(&[("speakers", true), ("dac", false)]);
        w.devices_changed(false);
        assert_eq!((on(&w), playing(&w), w.play_index, w.missing.is_none()), (Some("dac"), true, Some(1), true));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_device_back_too_late_or_after_a_pause_stays_paused() {
        connect(&[("speakers", true), ("dac", false)]);
        let (mut w, dir) = device_worker("dev-late", Some("dac"));
        connect(&[("speakers", true)]);
        w.recover_output();
        w.missing.as_mut().unwrap().since = Instant::now() - devices::RESUME_WINDOW - Duration::from_secs(1);
        connect(&[("speakers", true), ("dac", false)]);
        w.devices_changed(false);
        assert!(w.missing.is_none() && !playing(&w), "back after the window: stays paused");
        // Paused when it went: stays paused when it's back, also at once.
        w.handle(Cmd::Play);
        assert!(playing(&w));
        connect(&[("speakers", true)]);
        w.recover_output();
        w.handle(Cmd::Pause);
        connect(&[("speakers", true), ("dac", false)]);
        w.devices_changed(false);
        assert!(w.missing.is_none() && !playing(&w));
        // Missing when Cadence starts (paused): the same.
        let (mut w2, dir2) = device_worker("dev-start", Some("gone"));
        w2.handle(Cmd::Pause);
        assert!(w2.missing.is_some() && w2.out.is_none());
        drop((w, w2));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn play_on_the_default_until_the_chosen_device_is_back() {
        connect(&[("speakers", true), ("dac", false)]);
        let (mut w, dir) = device_worker("dev-standin", Some("dac"));
        connect(&[("speakers", true)]);
        w.recover_output();
        w.handle(Cmd::UseDefaultForNow);
        assert_eq!((on(&w), playing(&w), w.stand_in), (Some("speakers"), true, true));
        w.emit();
        assert!(!w.status.lock().device.as_ref().unwrap().chosen);
        assert!(w.status.lock().device_missing.is_none());
        // The DAC is back: playback moves back to it.
        connect(&[("speakers", true), ("dac", false)]);
        w.devices_changed(false);
        assert_eq!((on(&w), playing(&w), w.stand_in), (Some("dac"), true, false));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A profile source over a book of its own (as the app's, `profiles::source`, over the settings).
    fn book_source(dsp: DspSettings, volume: f32, enabled: bool) -> (ProfileSource, Arc<Mutex<(crate::profiles::Profiles, DspSettings, f32)>>) {
        let book = Arc::new(Mutex::new((crate::profiles::Profiles::default(), dsp, volume)));
        let b = book.clone();
        let source: ProfileSource = Box::new(move |d: &DeviceRef| {
            if !enabled {
                return None;
            }
            let mut g = b.lock();
            let (p, dsp, volume) = &mut *g;
            p.switch_to(d, dsp, volume);
            Some((dsp.clone(), *volume))
        });
        (source, book)
    }

    #[test]
    fn moving_to_another_device_brings_its_settings_with_one_restart() {
        connect(&[("speakers", true), ("dac", false)]);
        let (mut w, dir) = device_worker("dev-profile", None);
        // The settings the engine has (`real_worker` turns Sound Check off).
        let mut base = DspSettings::default();
        base.leveling.mode = loudness::LevelingMode::Off;
        let (source, book) = book_source(base, 1.0, true);
        // Cadence started on the speakers: their settings are the ones there were (nothing changes).
        w.handle(Cmd::SetProfiles(Some(source)));
        assert_eq!((w.opens, w.profile_seq, w.volume), (1, 1, 1.0));
        // The DAC already has its own: EQ, crossfeed, Direct output, quieter.
        let mut dac = DspSettings::default();
        dac.eq.enabled = true;
        dac.eq.preset = "Rock".into();
        dac.crossfeed.enabled = true;
        dac.exclusive = true;
        dac.tone.loudness = false;
        book.lock().0.devices.insert("dac".into(), crate::profiles::Profile::of("dac name", &dac, 0.2));
        w.handle(Cmd::SetDevice(Some(dref("dac"))));
        assert_eq!(w.opens, 2, "one reopen for the move, Direct output and all");
        assert_eq!((w.eq.settings().preset.as_str(), w.crossfeed.enabled(), w.exclusive, w.tone_settings.loudness), ("Rock", true, true, false));
        assert_eq!(f32::from_bits(w.shared.volume.load(Ordering::Relaxed)), 0.2, "its own volume, before the first sample");
        assert!(w.out.as_ref().unwrap().device.contains("exclusive=true"));
        // Back to the speakers: theirs again.
        w.handle(Cmd::SetDevice(None));
        assert_eq!((w.opens, w.exclusive, w.volume, w.eq.settings().enabled), (3, false, 1.0, false));
        assert_eq!(book.lock().0.devices["dac"].eq.preset, "Rock", "kept");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_profiles_off_a_device_move_changes_no_setting() {
        connect(&[("speakers", true), ("dac", false)]);
        let (mut w, dir) = device_worker("dev-off", None);
        let mut dsp = DspSettings::default();
        dsp.eq.enabled = true;
        w.set_dsp(dsp);
        w.set_volume(0.5);
        let (source, _book) = book_source(DspSettings::default(), 1.0, false);
        w.handle(Cmd::SetProfiles(Some(source)));
        w.handle(Cmd::SetDevice(Some(dref("dac"))));
        assert_eq!((on(&w), w.eq.settings().enabled, w.volume, w.profile_seq), (Some("dac"), true, 0.5, 0));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
