//! What is open right now, and the hearing-safety rule (`safety`) as the worker applies it.
//!
//! `Worker::open` is the one record of the output that plays: the device it's on (its key) and
//! the mode it really runs in. It exists exactly while `Worker::out` does: only `opened` sets them
//! (where an open succeeds) and only `close_output` lets them go (every way the output closes:
//! the start of every open, a failed open, waiting for a missing device, a reopen after a failure
//! or a device change, a mode change, the release after a pause; the engine shutting down drops
//! both with the worker). `open_now` also checks `out`, so the two can't disagree.
//!
//! Everything that records a level reads it: `remember_level`, the book's `mode` (`sync_book`),
//! and through them what the settings are rewritten to. A volume set while nothing is open is the
//! volume wanted (the slider and the settings keep it), never a device's level: the next open
//! holds it to that device's level in that mode like any other (`hold_to_limit`).

use super::*;

/// The output open now (see the module comment).
pub(super) struct Open {
    /// The device's key for its levels (None: not known, so it has no history and gets none).
    pub key: Option<String>,
    /// It really runs in Direct output (a Direct output that fell back to the mixer is false).
    pub direct: bool,
    /// Direct output: the volume last heard on it (`heard_now`), where a raise that isn't heard
    /// may go (`unheard_raise`).
    pub heard: f32,
    /// The volume in effect isn't its level yet: set while it wasn't heard, beyond what may be
    /// recorded then (`remember_level`); it is once it's heard (`note_heard`).
    pub unrecorded: bool,
}

/// What `open_output` worked out before opening (`opened` finishes with it).
pub(super) struct Opening {
    pub key: Option<String>,
    /// The levels recorded on that device, read before anything was written for this open.
    pub rec: safety::Recorded,
    /// What was open when this open began (None: nothing was).
    pub previous: Option<Open>,
}

impl Worker {
    /// The output open now, if one is.
    pub(super) fn open_now(&self) -> Option<&Open> {
        self.open.as_ref().filter(|_| self.out.is_some())
    }

    /// Lets the output go (the only place it's let go). What it heard is noted and written to the
    /// book first, while it's still known whose it is. Returns what was open.
    pub(super) fn close_output(&mut self) -> Option<Open> {
        self.note_heard();
        if self.book_dirty && self.open_now().is_some() {
            self.sync_book();
        }
        self.out = None;
        self.open.take()
    }

    /// The key for the device about to be opened (None: not known). `previous`: the output open
    /// until this open began.
    pub(super) fn key_for(&self, device: Option<&DeviceRef>, previous: Option<&Open>) -> Option<String> {
        match device {
            Some(d) => Some(d.id.clone()),
            // Android routes by itself and names the DAC only once it's open (`opened`): until
            // then the one open last is the guess (`opened` only ever lowers from it).
            None if cfg!(target_os = "android") => self.last_key.clone(),
            // The list couldn't be read (off Windows): the system default is opened. Reopening the
            // stream that was open (a rate switch) keeps its key, or every song would start at the
            // cap again; a fresh open (nothing open before) is a device not known.
            None => previous.and_then(|p| p.key.clone()),
        }
    }

    /// Cadence starting (`run`): the mixer opens at once (Direct output takes the device only once
    /// something plays). The book may not have come yet: `recorded` and `profiles_came` see to it.
    pub(super) fn first_open(&mut self) {
        if !self.exclusive {
            self.open_output(None);
        }
    }

    /// An output opened (`direct`: it really runs in Direct output). The only place `out` is set.
    /// A new generation unless it replaces an open output on the same device in the same mode
    /// (a rate switch), so a command or a drag from before can't land on it.
    pub(super) fn opened(&mut self, out: Output, direct: bool, o: Opening) {
        #[cfg(target_os = "android")]
        let o = self.android_key(&out, direct, o);
        let same = o.previous.as_ref().is_some_and(|p| p.key.is_some() && p.key == o.key && p.direct == direct);
        if !same {
            self.next_device_gen();
        }
        let heard = o.key.as_ref().and_then(|k| self.heard.get(k).copied()).or(o.rec.direct).unwrap_or(self.volume);
        self.last_key = o.key.clone();
        self.out = Some(out);
        self.open = Some(Open { key: o.key, direct, heard, unrecorded: false });
        self.remember_level();
        self.opened_at = Some(Instant::now());
        self.dirty = true;
    }

    /// Android: every output was keyed "" (DAC A's level applied to DAC B plugged in later). The
    /// key is the DAC's name as Android gives it (its USB product name; no serial is available to
    /// Cadence), known once it's open: a different one is held to its own levels at once, before
    /// a sample of music reaches the new stream (its ring is still empty). Two DACs of the same
    /// model share their levels.
    #[cfg(target_os = "android")]
    fn android_key(&mut self, out: &Output, direct: bool, mut o: Opening) -> Opening {
        let real = Some(format!("android:{}", out.device));
        if real != o.key {
            o.rec = self.recorded(real.as_deref());
            // (From the volume already held for the guess: only ever lower.)
            self.hold_to_limit(o.rec, direct, self.volume);
            o.key = real;
        }
        o
    }

    /// The hearing-safety rule (`safety`), for the device being opened (or open) in this mode
    /// (`direct`: Direct output): the volume may not be higher than `rec`, the level recorded on
    /// that device in that mode, read before anything was written for this open; without one, no
    /// more than `NEW_DIRECT_VOLUME` with Direct output, and never louder than `before` (what was
    /// playing) through the mixer. Lowered, the volume is told (the status carries it, so the
    /// slider follows) and written back to the settings. Whatever any other part of Cadence did to
    /// the settings, nothing louder gets past here.
    pub(super) fn hold_to_limit(&mut self, rec: safety::Recorded, direct: bool, before: f32) {
        let v = self.held(self.volume, rec, direct, before);
        self.set_volume(v);
    }

    /// `wanted`, held to the limit (`hold_to_limit`): what may be applied. Told when lowered.
    pub(super) fn held(&mut self, wanted: f32, rec: safety::Recorded, direct: bool, before: f32) -> f32 {
        let limit = safety::limit(rec, direct, before);
        if wanted.is_finite() && wanted <= limit + 1e-6 {
            return wanted;
        }
        self.lowered(wanted, limit, direct);
        limit
    }

    /// The volume was lowered to `limit` from `wanted`: logged, told once, and written back.
    fn lowered(&mut self, wanted: f32, limit: f32, direct: bool) {
        let name = self.device.as_ref().map(|d| devices::display_name(&d.name)).unwrap_or_else(|| "this output".into());
        log::warn!("volume {wanted:.3} is above the {limit:.3} allowed on {name}{}: lowered", if direct { " with Direct output" } else { "" });
        self.book_dirty = true;
        self.book_sync = true;
        let pct = (limit.sqrt() * 100.0).round();
        self.notice = Some(if direct {
            format!("Volume lowered to {pct}% for {name} (Direct output). Turn it up while it plays.")
        } else {
            format!("Volume lowered to {pct}% for {name}.")
        });
    }

    /// The levels recorded on device `key` (None: not known, so none): the lower of this run's
    /// and the book's (kept across runs), so a level recorded before the book came (the mixer opens
    /// at startup before it) never hides a lower one kept from the last run. Only `remember_level`
    /// writes this run's: what was in effect while that device was open in that mode.
    pub(super) fn recorded(&mut self, key: Option<&str>) -> safety::Recorded {
        let Some(key) = key else { return safety::Recorded::default() };
        let mut get = |direct: bool| -> Option<f32> {
            let run = safety::level(self.levels.get(&(key.to_string(), direct)).copied());
            let kept = safety::level(self.profiles.as_mut().and_then(|b| b.saved_volume(key, direct)));
            match (run, kept) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };
        let shared = get(false);
        safety::Recorded::new(shared, get(true))
    }

    /// An automatic move that would play much louder, or on a device never played in this mode
    /// while the volume isn't low (`known`: it was): the song waits there for Play.
    pub(super) fn hold_if_unsafe(&mut self, known: bool, before: f32, device: Option<&DeviceRef>) {
        let Some(why) = safety::waits(known, before, self.volume) else { return };
        let name = device.map(|d| devices::display_name(&d.name)).unwrap_or_default();
        log::info!("moved to {name} by itself ({why:?}, {before:.3} → {:.3}): paused there", self.volume);
        self.hold = true;
        self.notice = Some(match why {
            safety::Wait::Louder => format!("Playback moved to {name}, which is set much louder, so it's paused. Press Play to carry on."),
            safety::Wait::Unknown => format!("Playback moved to {name}, which Cadence hasn't played on before, so it's paused. Press Play to carry on."),
        });
    }

    /// The mode the volume is heard in: the one the output open now really runs in, else the one
    /// it would open in.
    pub(super) fn level_mode(&self) -> bool {
        self.open_now().map_or(self.exclusive && !self.plain, |o| o.direct)
    }

    /// The volume in effect now is the open device's level in the mode it really runs in. Nothing
    /// is recorded while nothing is open (or the device isn't known): a volume set then wasn't
    /// heard on any device. With Direct output, a volume not heard yet is recorded only as far as
    /// the device may open without being heard (`safety::unheard_ceiling`: what it last played,
    /// or the start level with no history); beyond that it's the volume wanted until it's heard.
    pub(super) fn remember_level(&mut self) {
        let heard_now = self.heard_now();
        let v = self.volume;
        let Some(o) = self.open.as_mut().filter(|_| self.out.is_some()) else { return };
        let Some(key) = o.key.clone() else { return };
        if o.direct && !heard_now && v > safety::unheard_ceiling(o.heard, 0.0) + 1e-6 {
            o.unrecorded = true;
            return;
        }
        o.unrecorded = false;
        if self.levels.insert((key, o.direct), v) != Some(v) {
            self.book_dirty = true;
        }
    }

    /// The volume is heard as it is: playing, above zero, and nothing else lowering what reaches
    /// the device. The renderer sends the volume times the sleep timer's fade (`Shared::fade`);
    /// while the fade is under 1 the volume isn't what's heard. (Its own fades on pause, seek and
    /// skip last milliseconds and only while not playing or flushing; Sound Check, the tone and a
    /// crossfade's fade-in shape the music, not the level the listener judges by.)
    pub(super) fn heard_now(&self) -> bool {
        self.volume > 0.0 && self.shared.fade() >= 1.0 && self.shared.playing.load(Ordering::Relaxed)
    }

    /// Heard on an open output now (`heard_now`): with Direct output that volume is what was last
    /// heard there, and a volume set while it wasn't heard is its level from now on.
    pub(super) fn note_heard(&mut self) {
        if self.out.is_none() || !self.heard_now() {
            return;
        }
        let v = self.volume;
        let Some(o) = self.open.as_mut() else { return };
        let pending = o.unrecorded;
        if o.direct {
            o.heard = v;
            if let Some(k) = o.key.as_ref().filter(|k| self.heard.get(*k) != Some(&v)) {
                self.heard.insert(k.clone(), v);
            }
        }
        if pending {
            self.remember_level();
        }
    }

    /// The listener set the volume (for the device open now, or wanted for the next open).
    pub(super) fn user_volume(&mut self, v: f32) {
        // Changed while the chosen device is missing (nothing was heard): it doesn't carry on by
        // itself when it's back; Play does, at its own level.
        let changed = v.is_finite() && (v - self.volume).abs() > 1e-6;
        if let Some(w) = self.missing.as_mut().filter(|_| changed) {
            w.resume = false;
        }
        let v = self.unheard_raise(v);
        self.set_volume(v);
        self.remember_level();
        self.note_heard();
        self.book_dirty = true;
    }

    /// The first-raise rule (`safety::unheard_ceiling`): on a Direct output that isn't being heard
    /// now (paused, at zero, or under the sleep timer's fade: `heard_now`), a volume command goes no
    /// higher than what was last heard there (the start level with no history). Told when held.
    fn unheard_raise(&mut self, v: f32) -> f32 {
        let heard_now = self.heard_now();
        let Some(o) = self.open_now().filter(|o| o.direct) else { return v };
        let ceiling = safety::unheard_ceiling(o.heard, self.volume);
        if heard_now || !v.is_finite() || v <= ceiling + 1e-6 {
            return v;
        }
        self.lowered(v, ceiling, true);
        ceiling
    }
}
