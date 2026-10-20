//! Tray icon, the mini player window, and showing/hiding the main window.

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::audio::Cmd;
use crate::state::AppState;

const TRAY_ID: &str = "cadence";
pub const MINI_LABEL: &str = "mini";

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Play / pause", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next", true, None::<&str>)?;
    let prev = MenuItem::with_id(app, "prev", "Previous", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show Cadence", true, None::<&str>)?;
    let mini = MenuItem::with_id(app, "mini", "Mini player", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Cadence", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(
        app,
        &[&toggle, &next, &prev, &sep()?, &show, &mini, &sep()?, &quit],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("Cadence")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let send = |cmd| app.state::<Arc<AppState>>().engine.send(cmd);
            match event.id().as_ref() {
                "toggle" => send(Cmd::Toggle),
                "next" => send(Cmd::Next),
                "prev" => send(Cmd::Prev),
                "show" => show_main(app),
                "mini" => toggle_mini(app),
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn set_tooltip<R: Runtime>(app: &AppHandle<R>, text: Option<String>) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let text = text
            .map(|t| format!("Cadence: {t}"))
            .unwrap_or_else(|| "Cadence".into());
        // Windows truncates tray tooltips at 127 characters.
        let text: String = text.chars().take(120).collect();
        let _ = tray.set_tooltip(Some(text));
    }
}

pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Opens the mini player in the bottom-right corner, or closes it if it's open.
pub fn toggle_mini<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MINI_LABEL) {
        let _ = w.close();
        return;
    }
    let (w, h) = (380.0, 112.0);
    let built = WebviewWindowBuilder::new(app, MINI_LABEL, WebviewUrl::App("mini".into()))
        .title("Cadence mini player")
        .inner_size(w, h)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .transparent(true)
        .shadow(true)
        .build();
    match built {
        Ok(win) => {
            // Bottom-right of the screen the main window is on, above the taskbar.
            if let Ok(Some(monitor)) = win.current_monitor() {
                let scale = monitor.scale_factor();
                let area = monitor.work_area();
                let x = area.position.x as f64 + area.size.width as f64 - (w + 20.0) * scale;
                let y = area.position.y as f64 + area.size.height as f64 - (h + 20.0) * scale;
                let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
            }
        }
        Err(e) => log::warn!("can't open the mini player: {e}"),
    }
}
