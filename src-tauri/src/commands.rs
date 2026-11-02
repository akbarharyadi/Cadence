//! Tauri commands called from the web UI: thin wrappers over api.rs (the list is
//! `cadence_commands!`), plus the window ones only the Tauri app has.

use tauri::{AppHandle, State};

use crate::ctx::Ctx;

macro_rules! tauri_commands {
    ($($kind:ident $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(tauri_command!($kind $name($($arg: $ty),*) -> $ret);)*
    };
}

macro_rules! tauri_command {
    (plain $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty) => {
        #[tauri::command]
        pub fn $name(ctx: State<'_, Ctx>, $($arg: $ty),*) -> $ret {
            crate::api::$name(&ctx, $($arg),*)
        }
    };
    (fallible $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty) => {
        #[tauri::command]
        pub fn $name(ctx: State<'_, Ctx>, $($arg: $ty),*) -> Result<$ret, String> {
            crate::api::$name(&ctx, $($arg),*)
        }
    };
    (blocking $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty) => {
        #[tauri::command]
        pub async fn $name(ctx: State<'_, Ctx>, $($arg: $ty),*) -> Result<$ret, String> {
            let ctx = ctx.inner().clone();
            tauri::async_runtime::spawn_blocking(move || crate::api::$name(&ctx, $($arg),*))
                .await
                .map_err(|e| e.to_string())?
        }
    };
}

crate::cadence_commands!(tauri_commands);

// Async on purpose: creating a window from a synchronous command deadlocks on Windows.
#[tauri::command]
pub async fn toggle_mini(app: AppHandle) {
    #[cfg(desktop)]
    crate::tray::toggle_mini(&app);
    #[cfg(mobile)]
    let _ = app;
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    #[cfg(desktop)]
    crate::tray::show_main(&app);
    #[cfg(mobile)]
    let _ = app;
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}
