//! The hearing-safety rule over random runs of the engine (silent test output, stand-in device
//! list): a few hundred fixed-seed sequences of what the listener and the system do, checked after
//! every step.
//! - With Direct output really open on a device, the volume heard is never above the start level or
//!   what was last heard there (except a raise made while it played at a level heard), nor above
//!   the start level or the loudest level deliberately set there while it played.
//! - A level is only ever recorded (in the engine or in the settings) for the device and mode that
//!   was open at the time.
//! - What reaches the device is the volume times the sleep timer's fade: the fade starting,
//!   coming back over a second (cancelled, replaced) and ending in a pause are among the events,
//!   and the fade coming back is checked at each of its steps.
//!
//! Not covered here (blind spots, tested elsewhere or only on hardware): the real device branch of
//! `open_output` (the silent output stands in for it: no WASAPI, no fallback by a real error, no
//! Android key), the mixer's own rule ("never louder than what was playing" without a level), and
//! the renderer's glide between volumes. The startup open of the mixer before the book comes
//! (`first_open`, as `run` does) is in: every start and restart goes through it.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use super::safety_fade_tests::{effective, set_fade};
use super::safety_open_tests::{direct_open, release, slide, slide_now};
use super::tests::*;
use super::*;
use crate::state::Settings;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn coin(&mut self) -> bool {
        self.below(2) == 0
    }

    fn volume(&mut self) -> f32 {
        [0.0, 0.04, 0.09, 0.2, 0.5, 0.81, 1.0][self.below(7) as usize]
    }
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    Play,
    Pause,
    /// Paused long enough for the device to be let go.
    Release,
    /// The DAC unplugged (the stream fails).
    Missing,
    Back,
    /// The system default flips between the DAC and the speakers.
    Default,
    /// Another app takes the DAC: Direct output falls back to the mixer from the next open on.
    FallBack,
    DirectFree,
    Jump(usize),
    ToggleDirect,
    ToggleProfiles,
    Mute,
    Volume { v: f32, stale: bool },
    /// The settings replaced under the engine (a restore, a hand edit): loud profiles, Direct on.
    Replace,
    /// Cadence quits and starts again on the saved settings.
    Restart,
    /// The sleep timer's fade lowers the sound (the volume stays where it is).
    Fade(f32),
    /// The timer cancelled or replaced: the fade comes back over a second.
    FadeBack,
    /// The fade ran out: paused, and the fade back at 1 for the next Play.
    FadeEnd,
}

fn event(r: &mut Rng) -> Ev {
    match r.below(20) {
        0 | 1 => Ev::Play,
        2 => Ev::Pause,
        3 => Ev::Release,
        4 => Ev::Missing,
        5 => Ev::Back,
        6 => Ev::Default,
        7 => Ev::FallBack,
        8 => Ev::DirectFree,
        9 => Ev::Jump(r.below(3) as usize),
        10 => Ev::ToggleDirect,
        11 => Ev::ToggleProfiles,
        12 => Ev::Mute,
        13 => Ev::Replace,
        14 => Ev::Restart,
        15 => Ev::Fade([0.6, 0.2, 0.02, 0.0][r.below(4) as usize]),
        16 => Ev::FadeBack,
        17 => Ev::FadeEnd,
        _ => Ev::Volume { v: r.volume(), stale: r.below(3) == 0 },
    }
}

/// The engine as Cadence starts it on these saved settings (read back from their file), the book
/// given, nothing open yet.
fn start(name: &str, saved: &Settings) -> (Worker, Arc<Mutex<Settings>>) {
    let settings: Settings = serde_json::from_str(&serde_json::to_string(saved).unwrap()).unwrap();
    let (mut w, _cons, _dir) = real_worker(name, &["a1", "a2", "b1"], 20_000);
    w.list_devices = fake_devices;
    w.null_output = true;
    w.out = None;
    w.choice = settings.output_device.clone();
    w.take_dsp(settings.dsp.clone());
    w.set_volume(settings.volume);
    // Cadence starting: the mixer opens before the book comes (as `run` does).
    w.first_open();
    let book = Arc::new(Mutex::new(settings));
    w.handle(Cmd::SetProfiles(Some(crate::profiles::settings_book(book.clone()))));
    (w, book)
}

/// (device, really Direct output) of the output open now.
fn open_id(w: &Worker) -> Option<(String, bool)> {
    w.out.as_ref()?;
    Some((on(w)?.to_string(), direct_open(w)))
}

/// Every level recorded: (device, Direct output, in the settings rather than the engine).
fn levels(w: &Worker, book: &Arc<Mutex<Settings>>) -> HashMap<(String, bool, bool), f32> {
    let mut all: HashMap<(String, bool, bool), f32> = w.levels.iter().map(|((k, d), v)| ((k.clone(), *d, false), *v)).collect();
    for (k, l) in &book.lock().sound_profiles.levels {
        for (d, v) in [(false, l.shared), (true, l.direct)] {
            if let Some(v) = v {
                all.insert((k.clone(), d, true), v);
            }
        }
    }
    all
}

/// What the step started from.
struct Before {
    open: Option<(String, bool)>,
    playing: bool,
    volume: f32,
    /// The sleep timer's fade.
    fade: f32,
    levels: HashMap<(String, bool, bool), f32>,
}

struct World {
    name: String,
    w: Worker,
    book: Arc<Mutex<Settings>>,
    dac_in: bool,
    default_dac: bool,
    old_gen: u64,
    /// Per device: the volume last heard with Direct output (playing, not at zero).
    last_heard: HashMap<String, f32>,
    /// Per device: the loudest volume set with Direct output while it played at a level heard.
    deliberate: HashMap<String, f32>,
    /// The loudest volume × fade seen while the fade came back during the step.
    fade_peak: f32,
}

impl World {
    fn new(name: &str, r: &mut Rng) -> Self {
        let mut s = Settings { dsp: base_dsp(), volume: r.volume(), device_profiles: r.coin(), ..Default::default() };
        s.dsp.exclusive = r.coin();
        if r.coin() {
            s.output_device = Some(dref("dac"));
        }
        let mut heard = HashMap::new();
        if let Some(l) = [None, Some(0.05f32), Some(0.3)][r.below(3) as usize] {
            s.sound_profiles.set_level("dac", true, l);
            heard.insert("dac".to_string(), l);
        }
        let default_dac = r.coin();
        plug(true, default_dac);
        let (mut w, book) = start(name, &s);
        w.start_at(1, 0.1, r.coin());
        World { name: name.into(), w, book, dac_in: true, default_dac, old_gen: 0, last_heard: heard.clone(), deliberate: heard, fade_peak: 0.0 }
    }

    fn plug(&self) {
        plug(self.dac_in, self.default_dac);
    }

    fn apply(&mut self, ev: Ev, r: &mut Rng) {
        match ev {
            Ev::Play => self.w.handle(Cmd::Play),
            Ev::Pause => self.w.handle(Cmd::Pause),
            Ev::Release if direct_open(&self.w) => release(&mut self.w),
            Ev::Release => {}
            Ev::Missing => {
                self.dac_in = false;
                self.plug();
                // (`recover_output` without its pause for the system to settle.)
                self.w.listed_seen = None;
                self.w.reopen(false);
            }
            Ev::Back => {
                self.dac_in = true;
                self.plug();
                self.w.devices_changed(false);
            }
            Ev::Default => {
                self.default_dac = !self.default_dac;
                self.plug();
                self.w.devices_changed(true);
            }
            Ev::FallBack => self.w.direct_fails = true,
            Ev::DirectFree => self.w.direct_fails = false,
            Ev::Jump(i) => self.w.handle(Cmd::Jump(i)),
            Ev::ToggleDirect => {
                let mut dsp = self.w.applied_dsp.clone();
                dsp.exclusive = !dsp.exclusive;
                self.book.lock().dsp = dsp.clone();
                self.w.handle(Cmd::SetDspFor { dsp, device_gen: stamp(&self.w) });
            }
            Ev::ToggleProfiles => {
                let mut s = self.book.lock();
                s.device_profiles = !s.device_profiles;
            }
            Ev::Mute => slide_now(&mut self.w, &self.book, 0.0),
            Ev::Volume { v, stale } => {
                let g = if stale { self.old_gen } else { stamp(&self.w) };
                slide(&mut self.w, &self.book, v, g);
            }
            Ev::Replace => {
                let dsp = {
                    let mut s = self.book.lock();
                    s.volume = 1.0;
                    for p in s.sound_profiles.devices.values_mut() {
                        p.volume = Some(1.0);
                        p.exclusive = true;
                    }
                    s.dsp.exclusive = true;
                    s.dsp.clone()
                };
                self.w.handle(Cmd::SetDsp(dsp));
            }
            Ev::Fade(g) => set_fade(&self.w, g),
            Ev::FadeBack => {
                let from = fade_of(&self.w);
                for k in 1..=10 {
                    set_fade(&self.w, from + (1.0 - from) * k as f32 / 10.0);
                    self.fade_peak = self.fade_peak.max(effective(&self.w));
                }
            }
            Ev::FadeEnd => {
                set_fade(&self.w, 0.0);
                self.w.handle(Cmd::Pause);
                set_fade(&self.w, 1.0);
            }
            Ev::Restart => {
                self.w.sync_book();
                let saved = self.book.lock().clone();
                let (w, book) = start(&self.name, &saved);
                (self.w, self.book) = (w, book);
                self.w.start_at(1, 0.1, r.coin());
            }
        }
        // (As the engine's loop does once a round.)
        self.w.note_heard();
        if self.w.book_dirty {
            self.w.sync_book();
        }
    }

    fn before(&self, ev: Ev) -> Before {
        let restart = matches!(ev, Ev::Restart);
        Before {
            open: open_id(&self.w).filter(|_| !restart),
            playing: playing(&self.w),
            volume: vol(&self.w),
            fade: fade_of(&self.w),
            levels: if restart { HashMap::new() } else { levels(&self.w, &self.book) },
        }
    }

    fn check(&mut self, ev: Ev, b: &Before, at: &str) {
        let now = open_id(&self.w);
        let fade_peak = std::mem::take(&mut self.fade_peak);
        for ((id, direct, kept), v) in levels(&self.w, &self.book) {
            if b.levels.get(&(id.clone(), direct, kept)) == Some(&v) || (kept && matches!(ev, Ev::Restart)) {
                continue;
            }
            let pair = Some((id.clone(), direct));
            assert!(pair == b.open || pair == now, "{at}: level {v} recorded for {id} (Direct output: {direct}, settings: {kept}), which wasn't open in that mode (before: {:?}, now: {now:?})", b.open);
        }
        let Some((id, true)) = now else { return };
        let v = vol(&self.w);
        let commanded = match ev {
            Ev::Volume { v, .. } => Some(v),
            Ev::Mute => Some(0.0),
            _ => None,
        };
        let heard_then = b.open == Some((id.clone(), true)) && b.playing && b.volume > 0.0 && b.fade >= 1.0;
        let mut allowed = self.last_heard.get(&id).copied().unwrap_or(0.0).max(NEW_DIRECT_VOLUME);
        if let Some(c) = commanded.filter(|_| heard_then) {
            allowed = allowed.max(c);
            let d = self.deliberate.entry(id.clone()).or_insert(0.0);
            *d = d.max(v);
        }
        assert!(v <= allowed + 1e-6, "{at}: {v} on {id} with Direct output, above what was last heard there ({allowed}); before: {:?} {} {} {}; heard {:?}", b.open, b.playing, b.volume, b.fade, self.w.open_now().map(|o| o.heard));
        let peak = fade_peak.max(effective(&self.w));
        assert!(peak <= allowed + 1e-6, "{at}: {peak} reached {id} (volume × fade), above what was last heard there ({allowed})");
        let deliberate = self.deliberate.get(&id).copied().unwrap_or(0.0).max(NEW_DIRECT_VOLUME);
        assert!(v <= deliberate + 1e-6, "{at}: {v} on {id} with Direct output, above any level set there while it played ({deliberate})");
        if playing(&self.w) && v > 0.0 && fade_of(&self.w) >= 1.0 {
            self.last_heard.insert(id, v);
        }
    }
}

/// The stand-in devices: the speakers, and the DAC while it's plugged in.
fn plug(dac_in: bool, default_dac: bool) {
    let mut list = vec![("speakers", !(dac_in && default_dac))];
    if dac_in {
        list.push(("dac", default_dac));
    }
    connect(&list);
}

#[test]
fn random_runs_never_play_a_direct_output_louder_than_was_heard_there_nor_record_what_wasnt_open() {
    let name = "prop";
    for seed in 1..=300u64 {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut world = World::new(name, &mut r);
        let mut done = Vec::new();
        for step in 0..30 {
            if r.below(3) == 0 {
                world.old_gen = stamp(&world.w);
            }
            let ev = event(&mut r);
            let b = world.before(ev);
            world.apply(ev, &mut r);
            done.push(ev);
            world.check(ev, &b, &format!("seed {seed}, step {step} after {done:?}"));
        }
    }
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("cadence-worker-{name}-{}", std::process::id())));
}

/// The sleep timer's fade now.
fn fade_of(w: &Worker) -> f32 {
    w.shared.fade()
}
