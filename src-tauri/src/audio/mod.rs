//! The playback engine.
//!
//! One worker thread owns the decoder, DSP and the play queue, and keeps a lock-free ring buffer
//! topped up. The device side ([`output::Renderer`]) only copies samples out of that ring, so decoding
//! never blocks audio. Songs follow each other in the same stream: gapless.
//!
//! Signal chain (worker): decode → channel mapping → loudness gain → EQ → crossfeed → resample → ring.

pub mod crossfeed;
pub mod eq;
#[cfg(windows)]
mod exclusive;
#[cfg(target_os = "android")]
mod android_direct;
#[cfg(target_os = "android")]
mod usb_dac;
#[cfg(any(target_os = "android", test))]
mod usb_audio;
#[cfg(any(windows, target_os = "android"))]
mod pcm;
pub mod loudness;
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

#[derive(Clone, Debug)]
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
}

impl QueueItem {
    fn is_cue(&self) -> bool {
        self.start > 0.0 || self.end.is_some()
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
    /// Its Sound Check gain in dB (None = off), for its tone once it takes over.
    gain_db: Option<f32>,
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
                    let g = self.gain;
                    self.buf.extend(self.scratch.iter().map(|x| x * g));
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
    Restore {
        items: Vec<QueueItem>,
        index: usize,
        position: f64,
    },
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
}

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
}

pub struct Engine {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
    shared: Arc<Shared>,
}

impl Engine {
    pub fn start(
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
        std::thread::Builder::new()
            .name("cadence-audio".into())
            .spawn(move || {
                Worker::new(rx, status2, Box::new(on_status), shared2, volume, dsp, repeat, shuffle).run()
            })
            .expect("audio thread");
        Self { tx, status, shared }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
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
    /// Incoming-song audio left over when a crossfade completes (mapped and gain-adjusted).
    carry: Vec<f32>,
    carry_rate: u32,
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
    /// While shuffled: the order the songs after the current one had, to put back.
    unshuffled: Vec<String>,
    /// Pause at the next song boundary that matches (sleep timer).
    stop: Option<StopAfter>,
    volume: f32,
    error: Option<String>,
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
            unshuffled: Vec::new(),
            stop: None,
            volume,
            error: None,
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
        }
    }

    fn run(mut self) {
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
        let opened = match (self.exclusive, rate) {
            _ if self.plain => output::open_shared(self.shared.clone()),
            #[cfg(windows)]
            (true, Some(rate)) => match exclusive::open(self.shared.clone(), self.device_rate(rate)) {
                Ok(o) => {
                    self.note = None;
                    Ok(o)
                }
                Err(e) => {
                    log::warn!("exclusive mode failed: {e:#}");
                    self.note = Some(format!(
                        "Exclusive mode isn’t available right now ({e}), so Cadence is playing through the Windows mixer."
                    ));
                    output::open_shared(self.shared.clone())
                }
            },
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            (true, Some(rate)) => match output::open_direct(self.shared.clone(), self.device_rate(rate)) {
                Ok(o) => {
                    self.note = None;
                    Ok(o)
                }
                Err(e) => {
                    log::warn!("direct output failed: {e:#}");
                    self.note = Some(format!(
                        "Direct output isn’t available right now ({e}), so Cadence is playing through the system mixer."
                    ));
                    output::open_shared(self.shared.clone())
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
                output::open_shared(self.shared.clone())
            }
            _ => output::open_shared(self.shared.clone()),
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
        match self.play_index {
            // start_at reopens the device at the song's rate.
            Some(i) => self.start_at(i, self.position, playing),
            None if !self.exclusive => self.open_output(None),
            None => {}
        }
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
                self.unshuffled.clear();
                if self.shuffle && index < items.len() && items.len() > 1 {
                    // The chosen song first, the rest in a random order (put back in the list's
                    // order, from that song on, if shuffle is turned off).
                    let first = items.remove(index);
                    items.rotate_left(index);
                    self.unshuffled = items.iter().map(|q| q.id.clone()).collect();
                    shuffle_in_place(&mut items);
                    items.insert(0, first);
                    index = 0;
                }
                self.queue = items;
                if index < self.queue.len() {
                    self.start_at(index, 0.0, true);
                }
            }
            Cmd::Restore { items, index, position } => {
                self.unshuffled.clear();
                self.queue = items;
                if let Some(item) = self.queue.get(index) {
                    // Not right at the very end, or it would simply move on to the next song.
                    let at = position.clamp(0.0, (item.duration - 2.0).max(0.0));
                    self.start_at(index, at, false);
                }
            }
            Cmd::Play => self.play(),
            Cmd::Pause => self.shared.playing.store(false, Ordering::Relaxed),
            Cmd::Toggle => {
                if self.shared.playing.load(Ordering::Relaxed) {
                    self.shared.playing.store(false, Ordering::Relaxed);
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
                    let target = if self.position > 3.0 || i == 0 {
                        i
                    } else {
                        i - 1
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
                    self.queue = items;
                    self.start_at(0, 0.0, true);
                    return;
                }
                let at = if next {
                    self.play_index.unwrap() + 1
                } else {
                    self.queue.len()
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
                        self.unshuffled.clear();
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
                let item = self.queue.remove(from);
                self.shift_indices(from + 1, -1);
                self.queue.insert(to, item);
                self.shift_indices(to, 1);
                if prepared && (from == pi + 1 || to == pi + 1) {
                    self.restart_here(None);
                }
            }
            Cmd::StopAfter(rule) => self.stop = rule,
            Cmd::Remove(i) => {
                if i >= self.queue.len() || Some(i) == self.play_index {
                    return;
                }
                self.queue.remove(i);
                let was_prepared = self.decoding && self.dec_index == i;
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
            Cmd::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.shared
                    .volume
                    .store(self.volume.to_bits(), Ordering::Relaxed);
            }
            Cmd::SetDsp(dsp) => {
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
                    // Restarting the song (below) opens the device in the new mode.
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
                self.vocals.set_amount(dsp.vocal_cut);
                self.eq.set_settings(dsp.eq);
                self.crossfeed.set_settings(dsp.crossfeed);
                self.crossfade_settings = dsp.crossfade;
                self.leveling = dsp.leveling;
                // A new loudness gain has to start at the current position, not half a second later.
                if leveling_changed || mode_changed {
                    self.restart_here(None);
                }
            }
            Cmd::SetRepeat(r) => self.repeat = r,
            Cmd::SetShuffle(on) => self.set_shuffle(on),
            Cmd::Replace(pairs) => {
                for q in self.queue.iter_mut() {
                    if let Some((_, new)) = pairs.iter().find(|(old, _)| *old == q.id) {
                        *q = new.clone();
                    }
                }
                for id in self.unshuffled.iter_mut() {
                    if let Some((_, new)) = pairs.iter().find(|(old, _)| old == id) {
                        *id = new.id.clone();
                    }
                }
                self.dirty = true;
            }
        }
    }

    fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        self.dirty = true;
        let Some(pi) = self.play_index else {
            self.unshuffled.clear();
            return;
        };
        let mut rest = self.queue.split_off(pi + 1);
        if on {
            self.unshuffled = rest.iter().map(|q| q.id.clone()).collect();
            shuffle_in_place(&mut rest);
        } else {
            // Back in the order they had; songs added since keep their place at the end.
            let mut ordered = Vec::with_capacity(rest.len());
            for id in std::mem::take(&mut self.unshuffled) {
                if let Some(k) = rest.iter().position(|q| q.id == id) {
                    ordered.push(rest.remove(k));
                }
            }
            ordered.append(&mut rest);
            rest = ordered;
        }
        self.queue.append(&mut rest);
        // The next song may already be on its way to the device: prepare the new one instead.
        if self.dec_index > pi || self.boundaries.iter().any(|b| b.index > pi) {
            self.restart_here(None);
        }
    }

    /// Whether the song after `pi` is already being decoded or buffered for the device.
    fn next_is_prepared(&self, pi: usize) -> bool {
        self.dec_index > pi || self.boundaries.iter().any(|b| b.index > pi)
    }

    /// Puts `items` into the queue at `at`, never before the song that's playing.
    fn insert_items(&mut self, at: usize, items: Vec<QueueItem>) {
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

    /// Crossing from song `old` into `new`: whether the sleep timer's rule says to stop here
    /// (counting the song down when it doesn't).
    fn stop_here(&mut self, old: Option<usize>, new: usize) -> bool {
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
        self.dec_index = fix(self.dec_index);
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
        self.position = secs;
        self.decoding = false;
        self.dec = None;
        self.crossfade = None;
        self.crossfade_checked = None;
        self.carry.clear();
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
                    self.shared.playing.store(play, Ordering::Relaxed);
                    return;
                }
                Err(e) => {
                    log::warn!("can't play {}: {e:#}", self.queue[index].path.display());
                    self.error = Some(format!(
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

    /// Sets the per-song tone for song `index` and its linear gain (`self.gain`): Sound Check's gain
    /// plus back whatever level the tone EQ's own headroom took, as far as the song's peak allows.
    /// While an album plays in order its songs share one curve (the album's average), so gapless
    /// transitions never change the filters mid-stream.
    fn apply_tone(&mut self, index: usize, gain_db: Option<f32>) -> Option<ToneInfo> {
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
        let on = settings.enabled;
        self.tone_eq.set_settings(settings);
        let gain = gain_db.unwrap_or(0.0);
        let lift = if on { -self.tone_eq.auto_headroom_db() as f32 } else { 0.0 };
        let room = (-gain - 20.0 * peak.max(1e-6).log10() - 1.0).max(0.0);
        self.gain = 10f32.powf((gain + lift.min(room)) / 20.0);
        on.then(|| {
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
        })
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
        // Where the song starts in the file (a song of a CUE sheet starts partway in). `offset` is
        // the position in the song itself.
        let start = self.queue[index].start;
        let offset = if secs > 0.5 || start > 0.0 {
            (src.seek(start + secs).unwrap_or(start) - start).max(0.0)
        } else {
            0.0
        };
        let out_rate = self.out.as_ref().map(|o| o.rate).unwrap_or(src.rate);
        let gain_db = match self.leveling.mode {
            loudness::LevelingMode::Off => None,
            _ => Some(
                self.leveling
                    .gain_db(&self.queue[index].gain, self.album_context(index)),
            ),
        };
        let tone = self.apply_tone(index, gain_db);
        log::info!(
            "song {index}: {} Hz from {:.1}s, device {} Hz{}, gain {:.1} dB",
            src.rate,
            offset,
            out_rate,
            if may_reopen { "" } else { " (gapless)" },
            20.0 * self.gain.log10()
        );
        self.boundaries.push(Boundary {
            // `written` already counts samples still waiting in `pending`.
            start: self.written,
            index,
            offset,
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
        self.dec_frames = (offset * src.rate as f64) as u64;
        self.dec = Some(src);
        self.dec_index = index;
        self.decoding = true;
        self.error = None;
        Ok(Some(offset))
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
            // The rest of a song that faded in, already mapped and gain-adjusted.
            self.mapped.clear();
            self.mapped.append(&mut self.carry);
            self.carry_rate
        } else {
            let Some(dec) = self.dec.as_mut() else {
                return Step::Full;
            };
            // A song of a CUE sheet ends where the next one starts, partway into the file.
            let room = self.queue.get(self.dec_index).and_then(|q| q.end.map(|e| ((e - q.start) * f64::from(dec.rate)) as u64));
            if room.is_some_and(|r| self.dec_frames >= r) {
                return Step::EndOfSong;
            }
            match dec.next_chunk() {
                Ok(Some((samples, rate, src_ch))) => {
                    let frames = samples.len() / src_ch.max(1);
                    let samples = match room {
                        Some(r) if self.dec_frames + frames as u64 > r => &samples[..(r - self.dec_frames) as usize * src_ch.max(1)],
                        _ => samples,
                    };
                    self.dec_frames += (samples.len() / src_ch.max(1)) as u64;
                    map_channels(samples, src_ch, ch, &mut self.mapped);
                    if self.gain != 1.0 {
                        let g = self.gain;
                        self.mapped.iter_mut().for_each(|s| *s *= g);
                    }
                    rate
                }
                Ok(None) => return Step::EndOfSong,
                Err(e) => {
                    log::warn!("decode error: {e:#}");
                    return Step::EndOfSong;
                }
            }
        };
        self.maybe_start_crossfade(rate, out_rate);
        if let Some(xf) = self.crossfade.as_mut() {
            xf.mix_into(&mut self.mapped, ch);
        }
        self.tone_eq.set_format(ch, rate);
        self.tone_eq.process(&mut self.mapped);
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
        {
            return;
        }
        let Some(current) = self.queue.get(self.dec_index) else {
            return;
        };
        let remaining = current.duration - self.dec_frames as f64 / rate as f64;
        if remaining > cf.seconds as f64 || current.duration <= 0.0 {
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
        let gain_db = match self.leveling.mode {
            loudness::LevelingMode::Off => None,
            _ => Some(
                self.leveling
                    .gain_db(&self.queue[next].gain, self.album_context(next)),
            ),
        };
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
            // Set when the incoming song takes over (see `finish_track`).
            tone: None,
        });
        log::info!("crossfading into song {next} over {remaining:.1}s");
        self.crossfade = Some(Crossfade {
            src,
            index: next,
            gain: 10f32.powf(gain_db.unwrap_or(0.0) / 20.0),
            gain_db,
            rate,
            buf: Vec::new(),
            frames: 0,
            pos: 0,
            len: (remaining * rate as f64) as u64,
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
            // Its tone starts now; the gain already applied to the carried audio stays as it was.
            let tone = self.apply_tone(xf.index, xf.gain_db);
            if let Some(b) = self.boundaries.iter_mut().rev().find(|b| b.index == xf.index) {
                b.tone = tone;
            }
            return;
        }
        self.dec = None;
        let mut next = self.auto_next(self.dec_index);
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
                    self.stop = None;
                    self.shared.playing.store(false, Ordering::Relaxed);
                    self.start_at(index, 0.0, false);
                    return;
                }
                self.dirty = true;
            }
        }
        let ended = !self.decoding && self.pending.is_empty() && consumed >= self.written;
        if ended && self.shared.playing.load(Ordering::Relaxed) && self.play_index.is_some() {
            if let Some(n) = self.switch_pending.take() {
                self.start_at(n, 0.0, true);
                return;
            }
            // Queue finished: stop, ready to play again from the start of the last song.
            self.stop = None;
            self.shared.playing.store(false, Ordering::Relaxed);
            if let Some(i) = self.play_index {
                self.start_at(i, 0.0, false);
            }
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
            error: self.error.clone(),
            stop_after: self.stop.clone(),
        };
        (self.on_status)(&status);
        *self.status.lock() = status;
    }
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

/// Fisher–Yates with a small xorshift generator seeded from the clock (no crate needed for this).
fn shuffle_in_place<T>(items: &mut [T]) {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    for i in (1..items.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        items.swap(i, (x % (i as u64 + 1)) as usize);
    }
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
        // Repeating a song never ends the album.
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
}
