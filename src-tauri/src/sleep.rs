//! Sleep timer: pause after a number of minutes (fading the sound out first). "After this song",
//! "after N songs" and "after this album" are the engine's (`audio::StopAfter`), since they have
//! to act exactly at a song boundary.
//!
//! The fade is the engine's own gain on top of the volume (`Engine::set_fade`): the volume slider
//! stays where the listener put it, and moving it (or a volume key) during the fade still works.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::audio::Cmd;
use crate::ctx::Ctx;

/// The sound fades out over this long before the timer ends.
const FADE: Duration = Duration::from_secs(20);
const STEP: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct Sleep {
    /// Bumped by every start and cancel: a countdown whose number is out of date stops.
    generation: AtomicU64,
    /// Unix seconds when the minutes countdown ends (0 = none).
    ends_at: AtomicU64,
}

impl Sleep {
    pub fn ends_at(&self) -> Option<u64> {
        match self.ends_at.load(Ordering::Relaxed) {
            0 => None,
            t => Some(t),
        }
    }

    /// Stops a running countdown. Returns whether there was one.
    pub fn cancel(&self) -> bool {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.ends_at.swap(0, Ordering::SeqCst) != 0
    }

    /// Pauses after `minutes`, the last seconds fading out.
    pub fn start_minutes(&self, ctx: &Ctx, minutes: u32) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let total = Duration::from_secs(u64::from(minutes.max(1)) * 60);
        self.ends_at.store(crate::history::now() + total.as_secs(), Ordering::SeqCst);
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("cadence-sleep".into()).spawn(move || run(ctx, generation, total));
    }
}

fn run(ctx: Ctx, generation: u64, total: Duration) {
    let sleep = &ctx.state.sleep;
    let alive = || sleep.generation.load(Ordering::SeqCst) == generation;
    let started = Instant::now();
    let fade_at = total.saturating_sub(FADE);
    while started.elapsed() < fade_at {
        if !alive() {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let engine = &ctx.state.engine;
    if engine.status().playing {
        let fade = total.saturating_sub(started.elapsed()).max(STEP);
        let steps = (fade.as_millis() / STEP.as_millis()).max(1) as u32;
        for k in 1..=steps {
            if !alive() {
                // Cancelled while fading: the sound comes back.
                engine.set_fade(1.0);
                return;
            }
            if !engine.status().playing {
                break; // Paused by hand meanwhile.
            }
            let left = 1.0 - k as f32 / steps as f32;
            engine.set_fade(left * left);
            std::thread::sleep(STEP);
        }
        if alive() {
            engine.send(Cmd::Pause);
            // Let the pause settle before the sound comes back.
            std::thread::sleep(Duration::from_millis(400));
        }
        engine.set_fade(1.0);
    }
    if alive() {
        sleep.ends_at.store(0, Ordering::SeqCst);
        ctx.emit("sleep-timer", Option::<u64>::None);
    }
}
