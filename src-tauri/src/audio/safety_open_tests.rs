//! The hearing-safety rule when nothing is open, and when the real mode changes: a volume set while
//! no output plays is only the volume wanted (never a device's level), every new open is a new
//! generation, and the first raise above the start level on a Direct output happens while it plays.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::safety_tests::*;
use super::tests::*;
use super::*;
use crate::state::Settings;

/// Really open in Direct output now (the silent test output says which mode it "opened" in).
pub(super) fn direct_open(w: &Worker) -> bool {
    w.out.as_ref().is_some_and(|o| o.device.contains("exclusive=true"))
}

/// Paused for longer than the device is kept: let go.
pub(super) fn release(w: &mut Worker) {
    w.handle(Cmd::Pause);
    w.idle_since = Some(Instant::now() - Duration::from_secs(21));
    w.release_if_idle();
    assert!(w.out.is_none() && w.open_now().is_none(), "released");
}

/// The volume slider (`profiles::set_volume`): stamped, and taken by the settings while it's for
/// the device open.
pub(super) fn slide(w: &mut Worker, book: &Arc<Mutex<Settings>>, volume: f32, device_gen: u64) {
    w.handle(Cmd::SetVolumeFor { volume, device_gen });
    if device_gen == stamp(w) {
        book.lock().volume = volume;
    }
}

/// `slide`, for the device the latest status is of.
pub(super) fn slide_now(w: &mut Worker, book: &Arc<Mutex<Settings>>, volume: f32) {
    let g = stamp(w);
    slide(w, book, volume, g);
}

fn saved_direct(book: &Arc<Mutex<Settings>>, id: &str) -> Option<f32> {
    book.lock().sound_profiles.saved_volume(id, true)
}

/// The DAC chosen, Direct output on at a level of 0.1 played there before.
fn dac_at_a_tenth(follow: bool) -> Settings {
    let mut s = upgraded(0.1, true);
    s.sound_profiles.set_level("dac", true, 0.1);
    if !follow {
        s.output_device = Some(dref("dac"));
    }
    s
}

#[test]
fn a_volume_set_while_the_dac_is_unplugged_is_not_its_level_and_it_does_not_play_on_by_itself() {
    connect(&[("speakers", true), ("dac", false)]);
    let (mut w, dir, book) = launch("open-missing", &dac_at_a_tenth(false));
    w.start_at(1, 0.1, true);
    assert!(direct_open(&w) && playing(&w) && vol(&w) <= 0.1 + 1e-6);
    // Unplugged while playing: the stream fails, and the DAC isn't there to open again.
    connect(&[("speakers", true)]);
    w.listed_seen = None;
    w.reopen(false);
    assert!(w.missing.is_some() && w.out.is_none() && !playing(&w));
    // Nothing is heard: the listener drags the slider to the top.
    slide_now(&mut w, &book, 1.0);
    assert_eq!(w.volume, 1.0, "the slider moves");
    w.sync_book();
    assert_eq!(saved_direct(&book, "dac"), Some(0.1), "not the DAC's level");
    // Back within the window: stays paused (the volume was changed meanwhile).
    connect(&[("speakers", true), ("dac", false)]);
    w.devices_changed(false);
    assert!(!playing(&w), "no playing on by itself");
    assert!(vol(&w) <= 0.1 + 1e-6 || w.out.is_none(), "{}", vol(&w));
    // Play: the DAC's own level, told.
    w.handle(Cmd::Play);
    assert!(playing(&w) && direct_open(&w));
    assert!(vol(&w) <= 0.1 + 1e-6, "{}", vol(&w));
    w.sync_book();
    assert_eq!(saved_direct(&book, "dac"), Some(0.1));
    done(w, dir);
}

#[test]
fn following_the_default_a_volume_set_after_the_dac_was_let_go_never_reaches_it() {
    connect(&[("dac", true), ("speakers", false)]);
    let (mut w, dir, book) = launch("open-follow", &dac_at_a_tenth(true));
    w.start_at(1, 0.1, true);
    assert!(direct_open(&w) && vol(&w) <= 0.1 + 1e-6);
    release(&mut w);
    // The speakers become the default: nothing moves (nothing is open).
    connect(&[("dac", false), ("speakers", true)]);
    w.devices_changed(true);
    slide_now(&mut w, &book, 1.0);
    w.sync_book();
    assert_eq!(saved_direct(&book, "dac"), Some(0.1), "not the DAC's level");
    // Play opens the speakers; later the DAC is the default again, while playing.
    w.handle(Cmd::Play);
    assert_eq!(on(&w), Some("speakers"));
    connect(&[("dac", true), ("speakers", false)]);
    w.devices_changed(true);
    assert_eq!(on(&w), Some("dac"));
    assert!(vol(&w) <= 0.1 + 1e-6, "{}", vol(&w));
    done(w, dir);
}

#[test]
fn a_volume_raised_after_the_dac_was_let_go_is_lowered_on_play_and_told() {
    connect(&[("dac", true)]);
    let (mut w, dir, book) = launch("open-release", &dac_at_a_tenth(false));
    w.start_at(1, 0.1, true);
    release(&mut w);
    let g = stamp(&w);
    slide(&mut w, &book, 1.0, g);
    assert_eq!(w.volume, 1.0, "the slider moves, as the volume wanted");
    w.sync_book();
    assert_eq!(saved_direct(&book, "dac"), Some(0.1));
    w.notice = None;
    w.handle(Cmd::Play);
    assert!(playing(&w) && direct_open(&w));
    assert!(vol(&w) <= 0.1 + 1e-6 && w.volume <= 0.1 + 1e-6, "{}", vol(&w));
    assert!(w.notice.as_deref().is_some_and(|n| n.starts_with("Volume lowered to 32% for dac name (Direct output)")), "{:?}", w.notice);
    w.emit();
    let status = w.status.lock().clone();
    assert_eq!(status.volume, w.volume, "the slider snaps to what plays");
    assert_ne!(status.device_gen, g, "a new open: a drag from before ends there");
    done(w, dir);
}

#[test]
fn a_volume_dragged_up_while_paused_on_the_dac_is_held_until_it_plays() {
    connect(&[("dac", true)]);
    let mut s = upgraded(0.3, true);
    s.sound_profiles.set_level("dac", true, 0.3);
    let (mut w, dir, book) = launch("open-paused", &s);
    w.start_at(1, 0.1, true);
    assert_eq!(vol(&w), 0.3);
    w.handle(Cmd::Pause);
    assert!(w.out.is_some(), "still held (not let go yet)");
    w.notice = None;
    slide_now(&mut w, &book, 1.0);
    assert!(vol(&w) <= 0.3 + 1e-6, "not while nothing is heard: {}", vol(&w));
    assert!(w.notice.is_some());
    // Lowering is always taken.
    slide_now(&mut w, &book, 0.2);
    assert_eq!(vol(&w), 0.2);
    // Playing: up as far as the listener goes.
    w.handle(Cmd::Play);
    slide_now(&mut w, &book, 1.0);
    assert_eq!(vol(&w), 1.0);
    done(w, dir);
}

#[test]
fn falling_back_to_the_mixer_and_back_is_a_new_generation_and_unmute_from_zero_is_held() {
    connect(&[("dac", true)]);
    let (mut w, dir, book) = launch("open-b2", &upgraded(1.0, true));
    // Another app holds the DAC: the mixer plays, at full (the Windows volume is low).
    w.direct_fails = true;
    w.start_at(1, 0.1, true);
    assert!(!direct_open(&w));
    slide_now(&mut w, &book, 1.0);
    assert_eq!(vol(&w), 1.0, "the listener's own, through the mixer");
    slide_now(&mut w, &book, 0.0);
    let old = stamp(&w);
    // The next song: the DAC is free, Direct output really opens (muted).
    w.direct_fails = false;
    w.handle(Cmd::Jump(2));
    assert!(direct_open(&w) && vol(&w) == 0.0);
    assert_ne!(stamp(&w), old, "another real mode: another generation");
    // Unmute with the old generation: dropped.
    slide(&mut w, &book, 1.0, old);
    assert_eq!(vol(&w), 0.0);
    // With the new one, but from zero: no further than the start level.
    w.notice = None;
    slide_now(&mut w, &book, 1.0);
    assert!(vol(&w) <= CAP, "{}", vol(&w));
    assert_told_lowered(&w);
    // Paused, the same.
    slide_now(&mut w, &book, 0.0);
    w.handle(Cmd::Pause);
    slide_now(&mut w, &book, 1.0);
    assert!(vol(&w) <= CAP, "{}", vol(&w));
    // Playing at a level heard: up step by step, to any level, and that is its level then.
    w.handle(Cmd::Play);
    for v in [0.2, 0.4, 0.7, 1.0] {
        slide_now(&mut w, &book, v);
    }
    assert_eq!(vol(&w), 1.0);
    w.sync_book();
    assert_eq!(saved_direct(&book, "dac"), Some(1.0));
    done(w, dir);
}

#[test]
fn a_dac_opened_muted_records_zero_and_unmute_goes_no_further_than_the_start_level() {
    connect(&[("dac", true)]);
    let (mut w, dir, book) = launch("open-muted", &upgraded(0.0, true));
    w.start_at(1, 0.1, true);
    assert!(direct_open(&w) && vol(&w) == 0.0);
    w.sync_book();
    assert_eq!(saved_direct(&book, "dac"), Some(0.0), "what it played at");
    w.emit();
    let g = w.status.lock().device_gen;
    slide(&mut w, &book, 1.0, g);
    assert!(vol(&w) <= CAP, "{}", vol(&w));
    done(w, dir);
}

#[test]
fn the_mixer_opened_before_the_book_never_hides_a_lower_level_kept_from_the_last_run() {
    connect(&[("speakers", true)]);
    let mut s = Settings { dsp: base_dsp(), volume: 0.8, device_profiles: false, ..Default::default() };
    s.sound_profiles.set_level("speakers", false, 0.1);
    let (mut w, dir, settings) = engine("open-s1", &s);
    // Cadence starting: the mixer opens before the book comes (`run`).
    w.open_output(None);
    let book = Arc::new(Mutex::new(settings));
    w.handle(Cmd::SetProfiles(Some(crate::profiles::settings_book(book.clone()))));
    assert!(vol(&w) <= 0.1 + 1e-6, "the speakers' own level from the last run: {}", vol(&w));
    w.start_at(1, 0.1, true);
    assert!(vol(&w) <= 0.1 + 1e-6, "{}", vol(&w));
    done(w, dir);
}
