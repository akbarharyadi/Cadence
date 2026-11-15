//! The hearing-safety rule with the sleep timer's fade: what reaches the device is the volume times
//! the fade, so a raise while the fade hides it isn't heard, and the fade coming back (cancelled,
//! replaced, or after its pause) never brings a level louder than was heard before it.

use super::safety_open_tests::*;
use super::safety_tests::*;
use super::tests::*;
use super::*;
use crate::state::Settings;

/// The sleep timer's fade, as `Engine::set_fade` sets it.
pub(super) fn set_fade(w: &Worker, gain: f32) {
    w.shared.set_fade(gain);
}

/// What reaches the device: the volume times the fade (`Renderer::target_gain`).
pub(super) fn effective(w: &Worker) -> f32 {
    vol(w) * w.shared.fade()
}

/// The fade coming back over a second, as the sleep timer does when it's cancelled or replaced:
/// the loudest the device gets on the way.
fn fade_back(w: &Worker, from: f32) -> f32 {
    let mut loudest = effective(w);
    for k in 1..=10 {
        set_fade(w, from + (1.0 - from) * k as f32 / 10.0);
        loudest = loudest.max(effective(w));
    }
    loudest
}

/// The DAC with Direct output at a level of `level` played there before, playing now.
fn playing_at(name: &str, level: f32) -> (Worker, std::path::PathBuf, std::sync::Arc<parking_lot::Mutex<Settings>>) {
    connect(&[("dac", true)]);
    let mut s = upgraded(level, true);
    s.sound_profiles.set_level("dac", true, level);
    let (mut w, dir, book) = launch(name, &s);
    w.start_at(1, 0.1, true);
    assert!(direct_open(&w) && playing(&w) && (vol(&w) - level).abs() < 1e-6);
    (w, dir, book)
}

fn saved_direct(book: &std::sync::Arc<parking_lot::Mutex<Settings>>) -> Option<f32> {
    book.lock().sound_profiles.saved_volume("dac", true)
}

#[test]
fn a_raise_while_the_sleep_fade_hides_it_is_held_and_cancelling_brings_nothing_louder() {
    let (mut w, dir, book) = playing_at("fade-cancel", 0.1);
    let mut loudest = effective(&w);
    for g in [0.8, 0.5, 0.2, 0.05] {
        set_fade(&w, g);
        loudest = loudest.max(effective(&w));
    }
    // Nothing audible changes: the slider goes to the top (to counter the fade, or by accident).
    slide_now(&mut w, &book, 1.0);
    loudest = loudest.max(effective(&w));
    assert!(vol(&w) <= 0.1 + 1e-6, "not heard while the fade hides it: {}", vol(&w));
    // The timer is cancelled: the sound comes back.
    loudest = loudest.max(fade_back(&w, 0.05));
    assert!(loudest <= 0.1 + 1e-6, "louder than heard before the fade: {loudest}");
    w.sync_book();
    assert_eq!(saved_direct(&book), Some(0.1), "the level heard before the fade");
    done(w, dir);
}

#[test]
fn after_the_fade_ended_in_a_pause_a_raise_is_held_as_any_paused_one() {
    let (mut w, dir, book) = playing_at("fade-end", 0.1);
    set_fade(&w, 0.0);
    w.handle(Cmd::Pause);
    set_fade(&w, 1.0);
    slide_now(&mut w, &book, 1.0);
    w.handle(Cmd::Play);
    assert!(vol(&w) <= 0.1 + 1e-6, "{}", vol(&w));
    // Also once let go.
    release(&mut w);
    slide_now(&mut w, &book, 1.0);
    w.handle(Cmd::Play);
    assert!(vol(&w) <= 0.1 + 1e-6, "{}", vol(&w));
    w.sync_book();
    assert_eq!(saved_direct(&book), Some(0.1));
    done(w, dir);
}

#[test]
fn without_a_sleep_timer_a_raise_while_playing_is_taken_and_kept() {
    let (mut w, dir, book) = playing_at("fade-none", 0.1);
    slide_now(&mut w, &book, 0.6);
    assert_eq!(vol(&w), 0.6);
    w.sync_book();
    assert_eq!(saved_direct(&book), Some(0.6));
    done(w, dir);
}

#[test]
fn lowering_the_volume_during_the_fade_is_taken_and_kept() {
    let (mut w, dir, book) = playing_at("fade-lower", 0.1);
    set_fade(&w, 0.3);
    slide_now(&mut w, &book, 0.04);
    assert_eq!(vol(&w), 0.04);
    assert!(fade_back(&w, 0.3) <= 0.04 + 1e-6);
    w.sync_book();
    assert_eq!(saved_direct(&book), Some(0.04));
    done(w, dir);
}

#[test]
fn a_paused_raise_on_a_dac_with_a_quiet_history_never_opens_it_louder_than_it_last_played() {
    let (mut w, dir, book) = playing_at("fade-history", 0.05);
    w.handle(Cmd::Pause);
    w.notice = None;
    slide_now(&mut w, &book, 0.09);
    assert!(vol(&w) <= 0.05 + 1e-6, "{}", vol(&w));
    assert!(w.notice.is_some(), "told");
    release(&mut w);
    w.handle(Cmd::Play);
    assert!(vol(&w) <= 0.05 + 1e-6, "{}", vol(&w));
    w.sync_book();
    assert_eq!(saved_direct(&book), Some(0.05));
    done(w, dir);
}
