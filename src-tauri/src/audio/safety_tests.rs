//! The hearing-safety rule over whole engine runs: Cadence starting on saved settings (as read back
//! from their file), playing on the stand-in devices with the silent output (nothing real is listed
//! or opened), and starting again. What's checked is what reaches the renderer (`vol`), and what the
//! settings say afterwards.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use super::tests::*;
use super::*;
use crate::profiles::Profile;
use crate::state::Settings;

pub(super) const CAP: f32 = NEW_DIRECT_VOLUME + 1e-6;

/// The engine as Cadence starts it on these saved settings (read back from their file), without
/// the book yet. Nothing is open or playing.
pub(super) fn engine(name: &str, saved: &Settings) -> (Worker, PathBuf, Settings) {
    let settings: Settings = serde_json::from_str(&serde_json::to_string(saved).unwrap()).unwrap();
    let (mut w, _cons, dir) = real_worker(name, &["a1", "a2", "b1"], 100_000);
    w.list_devices = fake_devices;
    w.null_output = true;
    w.out = None;
    w.choice = settings.output_device.clone();
    w.take_dsp(settings.dsp.clone());
    w.set_volume(settings.volume);
    (w, dir, settings)
}

/// `engine`, with the book given before anything opens (as `hub::open` does).
pub(super) fn launch(name: &str, saved: &Settings) -> (Worker, PathBuf, Arc<Mutex<Settings>>) {
    let (mut w, dir, settings) = engine(name, saved);
    let book = Arc::new(Mutex::new(settings));
    w.handle(Cmd::SetProfiles(Some(crate::profiles::settings_book(book.clone()))));
    (w, dir, book)
}

pub(super) fn direct_dsp() -> DspSettings {
    let mut d = base_dsp();
    d.exclusive = true;
    d
}

/// The settings of someone upgrading: Direct output on, this volume, no device of their own yet.
pub(super) fn upgraded(volume: f32, per_device: bool) -> Settings {
    Settings { dsp: direct_dsp(), volume, device_profiles: per_device, ..Default::default() }
}

pub(super) fn done(w: Worker, dir: PathBuf) {
    drop(w);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Playing with Direct output really on, at no more than `cap`; told so when it was lowered.
fn assert_direct_at_most(w: &Worker, cap: f32) {
    assert_eq!(w.open_now().map(|o| o.direct), Some(true), "really in Direct output");
    assert!(vol(w) <= cap && w.volume <= cap, "{} / {}", vol(w), w.volume);
}

pub(super) fn assert_told_lowered(w: &Worker) {
    assert!(w.notice.as_deref().is_some_and(|n| n.starts_with("Volume lowered to 30% for dac name")), "{:?}", w.notice);
}

#[test]
fn the_first_play_after_the_upgrade_opens_the_dac_quietly_and_says_so() {
    connect(&[("dac", true)]);
    let (mut w, dir, book) = launch("safety-upgrade", &upgraded(1.0, true));
    w.start_at(1, 0.1, true);
    assert_direct_at_most(&w, CAP);
    assert_told_lowered(&w);
    w.sync_book();
    assert_eq!(book.lock().volume, w.volume, "the slider and the settings say what's heard");
    done(w, dir);
}

#[test]
fn the_first_play_after_the_upgrade_is_quiet_with_one_set_for_all_devices_too() {
    connect(&[("dac", true)]);
    let (mut w, dir, book) = launch("safety-upgrade-off", &upgraded(1.0, false));
    w.start_at(1, 0.1, true);
    assert_direct_at_most(&w, CAP);
    assert_told_lowered(&w);
    w.sync_book();
    assert_eq!(book.lock().volume, w.volume);
    done(w, dir);
}

#[test]
fn a_profile_volume_is_never_a_direct_output_level() {
    for current in [Some("dac"), None, Some("speakers")] {
        connect(&[("dac", true)]);
        let mut s = upgraded(1.0, true);
        s.sound_profiles.devices.insert("dac".into(), Profile::of("dac name", &direct_dsp(), 1.0));
        s.sound_profiles.current = current.map(String::from);
        let (mut w, dir, _book) = launch("safety-profile", &s);
        w.start_at(1, 0.1, true);
        assert_direct_at_most(&w, CAP);
        assert_told_lowered(&w);
        done(w, dir);
    }
}

#[test]
fn a_volume_raised_while_direct_output_fell_back_is_not_its_level_on_the_next_run() {
    for recorded in [None, Some(0.3f32)] {
        connect(&[("dac", true)]);
        let mut s = upgraded(0.05, true);
        if let Some(l) = recorded {
            s.sound_profiles.set_level("dac", true, l);
        }
        let (mut w, dir, book) = launch("safety-fallback", &s);
        // Direct output wasn't available: the mixer played, and was turned up to full there.
        w.direct_fails = true;
        w.start_at(1, 0.1, true);
        assert_eq!(w.open_now().map(|o| o.direct), Some(false));
        w.handle(Cmd::SetVolumeFor { volume: 1.0, device_gen: stamp(&w) });
        w.sync_book();
        assert_eq!(w.volume, 1.0, "the listener's own, through the mixer");
        let saved = book.lock().clone();
        done(w, dir);
        // The next start: Direct output works.
        let (mut w, dir, _) = launch("safety-fallback-2", &saved);
        w.start_at(1, 0.1, true);
        // Its own Direct output level from before, else the start level: never the mixer's 1.0.
        assert_direct_at_most(&w, recorded.unwrap_or(NEW_DIRECT_VOLUME) + 1e-6);
        done(w, dir);
    }
}

#[test]
fn a_volume_set_while_really_in_direct_output_is_its_level_from_then_on() {
    connect(&[("dac", true)]);
    let (mut w, dir, book) = launch("safety-own", &upgraded(1.0, true));
    w.start_at(1, 0.1, true);
    w.handle(Cmd::SetVolumeFor { volume: 0.5, device_gen: stamp(&w) });
    w.sync_book();
    let mut saved = book.lock().clone();
    done(w, dir);
    // The next start: exactly that, not the start level.
    let (mut w, dir, _) = launch("safety-own-2", &saved);
    w.start_at(1, 0.1, true);
    assert_direct_at_most(&w, 0.5);
    assert_eq!(vol(&w), 0.5);
    assert!(w.notice.is_none(), "nothing lowered: {:?}", w.notice);
    done(w, dir);
    // And not more, whatever the volume in the file says.
    saved.volume = 1.0;
    let (mut w, dir, _) = launch("safety-own-3", &saved);
    w.start_at(1, 0.1, true);
    assert_eq!(vol(&w), 0.5);
    done(w, dir);
}

#[test]
fn settings_reset_to_their_defaults_never_reach_the_dac_at_full_scale() {
    connect(&[("dac", true)]);
    // A damaged settings file read as the defaults: full volume, and nothing else known.
    let (mut w, dir, _book) = launch("safety-defaults", &Settings::default());
    assert_eq!(w.volume, 1.0);
    w.start_at(1, 0.1, true);
    assert_eq!(w.open_now().map(|o| o.direct), Some(false), "the defaults play through the mixer");
    // Direct output turned on.
    w.handle(Cmd::SetDspFor { dsp: direct_dsp(), device_gen: stamp(&w) });
    assert_direct_at_most(&w, CAP);
    assert_told_lowered(&w);
    done(w, dir);
    // Or a file that was reset with Direct output still on.
    let (mut w, dir, _book) = launch("safety-defaults-2", &Settings { dsp: direct_dsp(), ..Default::default() });
    w.start_at(1, 0.1, true);
    assert_direct_at_most(&w, CAP);
    done(w, dir);
}

#[test]
fn a_volume_key_read_from_the_last_status_is_dropped_once_playback_moved() {
    connect(&[("speakers", true), ("dac", false)]);
    let mut s = upgraded(0.81, true);
    s.sound_profiles.set_level("speakers", true, 0.81);
    let (mut w, dir, _book) = launch("safety-hotkey", &s);
    w.start_at(1, 0.1, true);
    assert_eq!(vol(&w), 0.81);
    w.emit();
    let shown = w.status.lock().clone();
    // Playback moves to the DAC (held to the start level) before the next status goes out.
    w.handle(Cmd::SetDevice(Some(dref("dac"))));
    assert!(vol(&w) <= CAP);
    let (volume, device_gen) = crate::profiles::volume_step(&shown, 0.05);
    assert!(volume > 0.81);
    w.handle(Cmd::SetVolumeFor { volume, device_gen });
    assert!(vol(&w) <= CAP, "the speakers' volume plus a step never lands on the DAC: {}", vol(&w));
    // From a status of the DAC's: a step from its own level.
    w.emit();
    let (volume, device_gen) = crate::profiles::volume_step(&w.status.lock().clone(), 0.05);
    w.handle(Cmd::SetVolumeFor { volume, device_gen });
    assert!((vol(&w) - 0.35f32 * 0.35).abs() < 1e-4, "{}", vol(&w));
    done(w, dir);
}

/// A book that notes the loudest volume the renderer had whenever the engine asked it something.
struct Spy {
    inner: ProfileSource,
    shared: Arc<Shared>,
    loudest: Arc<Mutex<f32>>,
}

impl Spy {
    fn look(&self) {
        let v = f32::from_bits(self.shared.volume.load(Ordering::Relaxed));
        let mut m = self.loudest.lock();
        *m = m.max(v);
    }
}

impl SoundBook for Spy {
    fn moved(&mut self, from: Option<&DeviceRef>, to: &DeviceRef, dsp: &DspSettings, volume: f32) -> Option<(DspSettings, f32)> {
        self.look();
        self.inner.moved(from, to, dsp, volume)
    }
    fn saved_volume(&mut self, id: &str, direct: bool) -> Option<f32> {
        self.look();
        self.inner.saved_volume(id, direct)
    }
    fn applied(&mut self, device: Option<&DeviceRef>, mode: Option<bool>, dsp: &DspSettings, volume: f32, in_use: bool) {
        self.look();
        self.inner.applied(device, mode, dsp, volume, in_use);
    }
}

#[test]
fn the_book_coming_after_the_dac_opened_never_puts_a_louder_volume_on_it() {
    connect(&[("dac", true)]);
    // The DAC has a profile at full volume (no level), and it opened before the book came.
    let mut s = upgraded(1.0, true);
    s.sound_profiles.devices.insert("dac".into(), Profile::of("dac name", &direct_dsp(), 1.0));
    let (mut w, dir, settings) = engine("safety-book-late", &s);
    w.start_at(1, 0.1, true);
    assert_direct_at_most(&w, CAP);
    let loudest = Arc::new(Mutex::new(0.0f32));
    let book = Arc::new(Mutex::new(settings));
    let spy = Spy { inner: crate::profiles::settings_book(book.clone()), shared: w.shared.clone(), loudest: loudest.clone() };
    w.handle(Cmd::SetProfiles(Some(Box::new(spy))));
    w.sync_book();
    assert_direct_at_most(&w, CAP);
    assert!(*loudest.lock() <= CAP, "applied, even for a moment: {}", loudest.lock());
    assert_eq!(book.lock().volume, w.volume);
    done(w, dir);
}

fn follow_default_to(w: &mut Worker, id: &str, others: &[&str]) {
    let mut list: Vec<(&str, bool)> = others.iter().map(|o| (*o, false)).collect();
    list.push((id, true));
    connect(&list);
    w.devices_changed(true);
    assert_eq!(on(w), Some(id));
}

#[test]
fn following_the_default_to_a_device_never_played_on_waits_for_play_unless_the_volume_is_low() {
    connect(&[("speakers", true), ("phones", false)]);
    let (mut w, dir, book) = launch("safety-follow", &Settings { dsp: base_dsp(), volume: 0.5, ..Default::default() });
    w.start_at(1, 0.1, true);
    assert_eq!((vol(&w), playing(&w)), (0.5, true), "started by the listener: plays");
    // Headphones become the default: never played on, so paused there, at the same volume.
    follow_default_to(&mut w, "phones", &["speakers"]);
    assert_eq!((playing(&w), vol(&w)), (false, 0.5));
    assert!(w.notice.as_deref().is_some_and(|n| n.contains("hasn't played on before") && n.contains("paused")), "{:?}", w.notice);
    w.handle(Cmd::Play);
    assert!(playing(&w));
    w.sync_book();
    assert_eq!(book.lock().sound_profiles.saved_volume("phones", false), Some(0.5), "known from now on");
    // Back and forth between devices played on before: it just follows.
    w.notice = None;
    follow_default_to(&mut w, "speakers", &["phones"]);
    follow_default_to(&mut w, "phones", &["speakers"]);
    assert_eq!((playing(&w), vol(&w), w.notice.as_deref()), (true, 0.5, None));
    done(w, dir);
    // At a low volume, a new device is followed at once (and never louder).
    connect(&[("speakers", true), ("phones", false)]);
    let (mut w, dir, _) = launch("safety-follow-low", &Settings { dsp: base_dsp(), volume: 0.05, ..Default::default() });
    w.start_at(1, 0.1, true);
    follow_default_to(&mut w, "phones", &["speakers"]);
    assert_eq!((playing(&w), vol(&w)), (true, 0.05));
    // A device the listener picks plays at once.
    connect(&[("speakers", true), ("phones", false), ("hdmi", false)]);
    w.handle(Cmd::SetVolumeFor { volume: 0.6, device_gen: stamp(&w) });
    w.handle(Cmd::SetDevice(Some(dref("hdmi"))));
    assert_eq!((on(&w), playing(&w), vol(&w)), (Some("hdmi"), true, 0.6));
    done(w, dir);
}
