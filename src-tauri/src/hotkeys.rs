//! Global hotkeys (desktop): control Cadence from any app, with the window closed or hidden.
//!
//! Ctrl+Alt plus a key. The media keys on a keyboard already work through the system media
//! controls (media.rs); these are for keyboards without them.

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::ctx::Ctx;

/// The keys (with Ctrl+Alt) and what they do.
pub const KEYS: [(Code, &str); 6] = [
    (Code::Space, "toggle"),
    (Code::ArrowRight, "next"),
    (Code::ArrowLeft, "prev"),
    (Code::ArrowUp, "volume up"),
    (Code::ArrowDown, "volume down"),
    (Code::KeyH, "favorite"),
];

fn modifiers() -> Modifiers {
    Modifiers::CONTROL | Modifiers::ALT
}

pub fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            if let Some((_, action)) = KEYS.iter().find(|(code, _)| shortcut.matches(modifiers(), *code)) {
                crate::api::hotkey(&app.state::<Ctx>(), action);
            }
        })
        .build()
}

/// Turns the hotkeys on (grabbing the keys) or off. A key another program already holds is
/// skipped, with a note in the log.
pub fn set(app: &AppHandle, on: bool) {
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    if !on {
        return;
    }
    for (code, action) in KEYS {
        if let Err(e) = shortcuts.register(Shortcut::new(Some(modifiers()), code)) {
            log::warn!("hotkey Ctrl+Alt+{code:?} ({action}) isn't available: {e}");
        }
    }
}
