//! Whether Cadence's windows can be seen (for hub.rs, tray.rs and lib.rs): a window hidden in
//! the tray or minimized gets no 4-a-second player updates, its WebView2 is told it's not visible
//! (Chromium stops drawing and slows its timers), and its page is told to stop its own timers.
//! On showing it again the page gets one fresh status first, so the first frame is right.
//!
//! The state follows Tauri's window events, nothing polls: the main window's close-to-tray hide
//! and `tray::show_main`, and, for minimizing, the window's own minimized state (`is_minimized`,
//! Win32 `IsIconic`) asked on every `Moved` and `Resized` event. The size those events carry can't
//! tell: Cadence's windows have no system frame, so tao makes the client area the whole window
//! (its `WM_NCCALCSIZE`), and a minimized window keeps a small non-zero size (the iconic
//! placeholder at −32000, −32000); Tauri also replaces tao's size with `inner_size()` (tauri-
//! runtime-wry `WindowEventWrapper::parse`). Minimizing moves the window there (`Moved`), and
//! restoring moves it back and sizes it (`Moved`, `Resized`). On phones nothing here changes:
//! every window counts as visible and gets every update (the Android notification is driven from
//! them).
#![cfg_attr(mobile, allow(dead_code))]

use parking_lot::Mutex;

pub const MAIN: &str = "main";
pub const MINI: &str = "mini";
/// To a window's page when it was hidden (false) or shown again (true).
pub const EVENT: &str = "window-visible";

/// One window: hidden (in the tray) and minimized are separate (it can be both).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Win {
    pub hidden: bool,
    pub minimized: bool,
}

impl Win {
    pub fn visible(self) -> bool {
        !self.hidden && !self.minimized
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinEvent {
    Hide,
    /// Shown and restored (the tray, the taskbar, a second start).
    Show,
    /// The window moved or changed size: whether it is minimized now (asked of the window).
    Minimized(bool),
    Opened,
    Closed,
}

/// The windows' state: the main one always exists, the mini player only while it's open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Windows {
    pub main: Win,
    pub mini: Option<Win>,
}

/// Who gets a player update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Targets {
    /// Every window (as before).
    All,
    /// Only these (possibly none).
    Only(Vec<&'static str>),
}

/// What to do when a window's visibility flips, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// One full player status (with the queue) to the window.
    FreshStatus,
    /// The webview itself visible or not (WebView2's controller).
    Webview(bool),
    /// The page's `window-visible` event.
    TellPage(bool),
}

impl Windows {
    fn slot(&mut self, label: &str) -> Option<&mut Win> {
        match label {
            MAIN => Some(&mut self.main),
            MINI => self.mini.as_mut(),
            _ => None,
        }
    }

    pub fn visible(&self, label: &str) -> bool {
        match label {
            MAIN => self.main.visible(),
            MINI => self.mini.is_some_and(Win::visible),
            _ => true,
        }
    }

    /// Applies an event to a window. Some(visible) when that window's visibility flipped.
    pub fn apply(&mut self, label: &str, event: WinEvent) -> Option<bool> {
        let before = self.visible(label);
        match event {
            // Not flips: a mini player opening is a new page that asks for the status itself, and
            // a closed one needs nothing.
            WinEvent::Opened | WinEvent::Closed => {
                if label == MINI {
                    self.mini = (event == WinEvent::Opened).then(Win::default);
                }
                return None;
            }
            WinEvent::Hide => {
                if let Some(w) = self.slot(label) {
                    w.hidden = true;
                }
            }
            WinEvent::Show => {
                if let Some(w) = self.slot(label) {
                    *w = Win::default();
                }
            }
            WinEvent::Minimized(minimized) => {
                if let Some(w) = self.slot(label) {
                    w.minimized = minimized;
                }
            }
        }
        let after = self.visible(label);
        (before != after).then_some(after)
    }

    /// Who gets this update: changes that matter (song, queue, play/pause, an error, settings)
    /// go to every window, hidden ones too, so none misses them (rare, and a hidden webview
    /// doesn't draw them); position-only ticks only to windows that can be seen.
    pub fn targets(&self, significant: bool) -> Targets {
        let mini_shown = self.mini.is_none_or(Win::visible);
        if significant || (self.main.visible() && mini_shown) {
            return Targets::All;
        }
        let mut only = Vec::new();
        if self.main.visible() {
            only.push(MAIN);
        }
        if self.mini.is_some_and(Win::visible) {
            only.push(MINI);
        }
        Targets::Only(only)
    }
}

/// What a flip means: on showing, the fresh status goes first (so the page has it by the time it
/// draws again), then the webview draws again, then the page restarts its timers.
pub fn steps(visible: bool) -> &'static [Step] {
    if visible {
        &[Step::FreshStatus, Step::Webview(true), Step::TellPage(true)]
    } else {
        // The page stops its timers first; it keeps what it has (it gets the changes that matter).
        &[Step::TellPage(false), Step::Webview(false)]
    }
}

/// A player update without what moves on every update (the position, the output's peak level)
/// and without the queue (sent only when it changed, which counts by itself): when this is the
/// same as the last one, the update is only a tick.
pub fn headline(update: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut h: serde_json::Map<_, _> = update
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| *k != "queue" && *k != "position")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(out) = h.get_mut("output").and_then(|v| v.as_object_mut()) {
        out.remove("peakDb");
    }
    h
}

static WINDOWS: Mutex<Windows> = Mutex::new(Windows {
    main: Win { hidden: false, minimized: false },
    mini: None,
});

/// Who gets this player update (see `Windows::targets`).
pub fn targets(significant: bool) -> Targets {
    WINDOWS.lock().targets(significant)
}

/// A window event: updates the state and, when the window's visibility flipped, carries out the
/// steps (desktop). The lock is let go before anything is called.
#[cfg(desktop)]
pub fn on_event<R: tauri::Runtime>(app: &tauri::AppHandle<R>, label: &str, event: WinEvent) {
    use tauri::{Emitter, Manager};
    let flipped = WINDOWS.lock().apply(label, event);
    let Some(visible) = flipped else { return };
    log::debug!("window {label} {}", if visible { "shown" } else { "hidden" });
    let Some(w) = app.get_webview_window(label) else { return };
    for &step in steps(visible) {
        match step {
            Step::FreshStatus => {
                if let Some(ctx) = app.try_state::<crate::ctx::Ctx>() {
                    let _ = app.emit_to(label, "player-status", ctx.state.engine.status());
                }
            }
            Step::Webview(on) => set_webview_visible(&w, on),
            Step::TellPage(on) => {
                let _ = app.emit_to(label, EVENT, on);
            }
        }
    }
}

/// WebView2: the controller's IsVisible. Chromium then stops drawing, throttles timers and the
/// page's `document.visibilityState` turns "hidden" (wry only sets it when the webview itself is
/// hidden, not its window). Scripts and events keep running. Elsewhere the page's own visibility
/// and the `window-visible` event do the job.
#[cfg(desktop)]
fn set_webview_visible<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>, on: bool) {
    #[cfg(windows)]
    {
        let r = w.with_webview(move |pw| unsafe {
            if let Err(e) = pw.controller().SetIsVisible(on) {
                log::warn!("can't set the webview's visibility: {e}");
            }
        });
        if let Err(e) = r {
            log::warn!("can't reach the webview: {e}");
        }
    }
    #[cfg(not(windows))]
    let _ = (w, on);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_only() -> Windows {
        Windows::default()
    }

    #[test]
    fn hiding_and_showing_the_main_window() {
        let mut w = main_only();
        assert_eq!(w.apply(MAIN, WinEvent::Hide), Some(false));
        assert_eq!(w.apply(MAIN, WinEvent::Hide), None, "already hidden");
        assert_eq!(w.apply(MAIN, WinEvent::Show), Some(true));
        assert_eq!(w.apply(MAIN, WinEvent::Show), None);
    }

    #[test]
    fn minimizing_and_restoring_flip_once_each() {
        let mut w = main_only();
        // A plain move or resize of a window on screen changes nothing.
        assert_eq!(w.apply(MAIN, WinEvent::Minimized(false)), None);
        // Minimizing: `Moved` (to −32000, −32000), then `Resized` (the placeholder's size).
        assert_eq!(w.apply(MAIN, WinEvent::Minimized(true)), Some(false));
        assert_eq!(w.apply(MAIN, WinEvent::Minimized(true)), None, "the second event of the pair");
        assert!(!w.visible(MAIN));
        assert_eq!(w.targets(false), Targets::Only(vec![]), "no position ticks while minimized");
        // Restoring: `Moved` back, then `Resized`: one flip, which sends a fresh status first.
        let flipped = w.apply(MAIN, WinEvent::Minimized(false));
        assert_eq!(flipped, Some(true));
        assert_eq!(steps(flipped.unwrap())[0], Step::FreshStatus);
        assert_eq!(w.apply(MAIN, WinEvent::Minimized(false)), None);
        assert_eq!(w.targets(false), Targets::All);
    }

    #[test]
    fn minimizing_is_like_hiding_in_the_tray() {
        let (mut tray, mut min) = (main_only(), main_only());
        assert_eq!(tray.apply(MAIN, WinEvent::Hide), min.apply(MAIN, WinEvent::Minimized(true)));
        assert_eq!(tray.targets(false), min.targets(false));
        assert_eq!(tray.apply(MAIN, WinEvent::Show), min.apply(MAIN, WinEvent::Minimized(false)));
        assert_eq!(tray.targets(false), min.targets(false));
    }

    #[test]
    fn hidden_and_minimized_needs_showing() {
        let mut w = main_only();
        w.apply(MAIN, WinEvent::Minimized(true));
        assert_eq!(w.apply(MAIN, WinEvent::Hide), None, "already not visible");
        // Restored while still hidden in the tray: still not visible.
        assert_eq!(w.apply(MAIN, WinEvent::Minimized(false)), None);
        assert!(!w.visible(MAIN));
        assert_eq!(w.apply(MAIN, WinEvent::Show), Some(true));
        // `show_main` also unminimizes: the restore that follows changes nothing.
        assert_eq!(w.apply(MAIN, WinEvent::Minimized(false)), None);
    }

    #[test]
    fn the_mini_player_opening_and_closing_is_not_a_flip() {
        let mut w = main_only();
        assert!(!w.visible(MINI));
        assert_eq!(w.apply(MINI, WinEvent::Opened), None);
        assert!(w.visible(MINI));
        assert_eq!(w.apply(MINI, WinEvent::Minimized(true)), Some(false));
        assert_eq!(w.apply(MINI, WinEvent::Minimized(false)), Some(true));
        assert_eq!(w.apply(MINI, WinEvent::Closed), None);
        assert_eq!(w.mini, None);
        // Events for a closed mini player do nothing.
        assert_eq!(w.apply(MINI, WinEvent::Hide), None);
        assert_eq!(w.mini, None);
    }

    #[test]
    fn everyone_visible_gets_everything() {
        let mut w = main_only();
        assert_eq!(w.targets(false), Targets::All);
        w.apply(MINI, WinEvent::Opened);
        assert_eq!(w.targets(false), Targets::All);
    }

    #[test]
    fn main_hidden_and_mini_visible_ticks_go_to_the_mini_player_only() {
        let mut w = main_only();
        w.apply(MINI, WinEvent::Opened);
        w.apply(MAIN, WinEvent::Hide);
        assert_eq!(w.targets(false), Targets::Only(vec![MINI]));
        // The song changed: both hear it.
        assert_eq!(w.targets(true), Targets::All);
    }

    #[test]
    fn main_minimized_and_mini_visible() {
        let mut w = main_only();
        w.apply(MINI, WinEvent::Opened);
        w.apply(MAIN, WinEvent::Minimized(true));
        assert_eq!(w.targets(false), Targets::Only(vec![MINI]));
    }

    #[test]
    fn mini_hidden_and_main_visible() {
        let mut w = main_only();
        w.apply(MINI, WinEvent::Opened);
        w.apply(MINI, WinEvent::Minimized(true));
        assert_eq!(w.targets(false), Targets::Only(vec![MAIN]));
    }

    /// A flip is only ever reported for the window it happened to, and `on_event` addresses every
    /// step to that window's label: the other window gets neither the event nor the fresh status.
    #[test]
    fn main_minimized_with_the_mini_player_open_flips_only_the_main_window() {
        let mut w = main_only();
        assert_eq!(w.apply(MINI, WinEvent::Opened), None);
        assert_eq!(w.apply(MAIN, WinEvent::Minimized(true)), Some(false), "addressed to main");
        assert!(w.visible(MINI), "the mini player is not told it was hidden");
        assert_eq!(w.targets(false), Targets::Only(vec![MINI]), "ticks go to the mini player only");
        // The mini player's own (non-)events change nothing for it.
        assert_eq!(w.apply(MINI, WinEvent::Minimized(false)), None);
        assert_eq!(steps(false), [Step::TellPage(false), Step::Webview(false)]);
        // Restoring main: one flip for main, with the fresh status before the page is told.
        let flipped = w.apply(MAIN, WinEvent::Minimized(false));
        assert_eq!(flipped, Some(true));
        assert_eq!(steps(true), [Step::FreshStatus, Step::Webview(true), Step::TellPage(true)]);
        assert_eq!(w.targets(false), Targets::All);
    }

    #[test]
    fn the_mini_player_minimized_or_closed_leaves_main_showing() {
        let mut w = main_only();
        w.apply(MINI, WinEvent::Opened);
        assert_eq!(w.apply(MINI, WinEvent::Minimized(true)), Some(false), "addressed to mini");
        assert!(w.visible(MAIN));
        assert_eq!(w.targets(false), Targets::Only(vec![MAIN]));
        // Closing it while minimized: no flip, and main gets everything again.
        assert_eq!(w.apply(MINI, WinEvent::Closed), None);
        assert_eq!(w.targets(false), Targets::All);
        // A reopened one starts visible and asks for its own status.
        assert_eq!(w.apply(MINI, WinEvent::Opened), None);
        assert!(w.visible(MINI));
    }

    #[test]
    fn both_hidden_ticks_go_nowhere_but_changes_still_do() {
        let mut w = main_only();
        w.apply(MAIN, WinEvent::Hide);
        assert_eq!(w.targets(false), Targets::Only(vec![]));
        w.apply(MINI, WinEvent::Opened);
        w.apply(MINI, WinEvent::Minimized(true));
        assert_eq!(w.targets(false), Targets::Only(vec![]));
        assert_eq!(w.targets(true), Targets::All);
    }

    #[test]
    fn showing_sends_a_fresh_status_first() {
        assert_eq!(steps(true)[0], Step::FreshStatus);
        assert_eq!(steps(true), [Step::FreshStatus, Step::Webview(true), Step::TellPage(true)]);
        // Hiding tells the page before the webview stops (its timers stop at once).
        assert_eq!(steps(false), [Step::TellPage(false), Step::Webview(false)]);
    }

    #[test]
    fn only_position_and_peak_moving_is_a_tick() {
        let a = serde_json::json!({"queue": ["a"], "index": 0, "playing": true, "position": 1.0, "output": {"peakDb": -6.0, "mode": "shared"}});
        let b = serde_json::json!({"index": 0, "playing": true, "position": 1.25, "output": {"peakDb": -3.0, "mode": "shared"}});
        assert_eq!(headline(&a), headline(&b));
        let paused = serde_json::json!({"index": 0, "playing": false, "position": 1.25, "output": {"peakDb": -3.0, "mode": "shared"}});
        assert_ne!(headline(&b), headline(&paused));
        let next = serde_json::json!({"index": 1, "playing": true, "position": 0.0, "output": {"peakDb": -3.0, "mode": "shared"}});
        assert_ne!(headline(&b), headline(&next));
        let exclusive = serde_json::json!({"index": 0, "playing": true, "position": 1.5, "output": {"peakDb": -3.0, "mode": "exclusive"}});
        assert_ne!(headline(&b), headline(&exclusive));
    }

    #[test]
    fn hide_then_show_round_trip_delivers() {
        let mut w = main_only();
        w.apply(MAIN, WinEvent::Hide);
        assert_eq!(w.targets(false), Targets::Only(vec![]));
        let flipped = w.apply(MAIN, WinEvent::Show);
        assert_eq!(flipped, Some(true));
        assert_eq!(steps(flipped.unwrap())[0], Step::FreshStatus);
        assert_eq!(w.targets(false), Targets::All);
    }
}
