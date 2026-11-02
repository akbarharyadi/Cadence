//! C ABI for the native Windows front end (native/windows): the same core, commands and
//! background work as the Tauri app, without Tauri.
//!
//! The UI calls `cadence_start` once (data folder, window handle, event callback), then
//! `cadence_call(command, json_args)` for everything else: the commands of `cadence_commands!`
//! with the web UI's names and camelCase arguments, plus a few native-only ones. The reply is
//! `{"ok": value}` or `{"err": "message"}`, freed with `cadence_free`. Slow commands block, so the
//! UI calls them off its UI thread. Events arrive on Cadence's threads as (name, json).

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};

use crate::ctx::{Ctx, Host};

pub type EventFn = extern "C" fn(name: *const c_char, json: *const c_char);

static CTX: OnceLock<Ctx> = OnceLock::new();

/// Events go to the native UI's callback.
struct NativeHost(EventFn);

impl Host for NativeHost {
    fn emit(&self, event: &str, payload: Value) {
        let (Ok(name), Ok(json)) = (CString::new(event), CString::new(payload.to_string())) else {
            return;
        };
        (self.0)(name.as_ptr(), json.as_ptr());
    }
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller passes a NUL-terminated UTF-8 string that outlives the call.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Opens the library and the player and starts the background work. `log_dir` gets cadence.log.
/// Returns 0 on success, 1 if already started.
#[unsafe(no_mangle)]
pub extern "C" fn cadence_start(data_dir: *const c_char, local_dir: *const c_char, hwnd: isize, on_event: EventFn) -> i32 {
    if CTX.get().is_some() {
        return 1;
    }
    let data_dir = PathBuf::from(text(data_dir));
    let local_dir = PathBuf::from(text(local_dir));
    logging::init(&local_dir.join("logs"));
    let (ctx, status) = crate::hub::open(data_dir, &local_dir, Arc::new(NativeHost(on_event)));
    // The native app shows the song in its own tray icon (it gets player-status).
    crate::hub::start_status_hub(ctx.clone(), status, hwnd, |_| {});
    crate::hub::start_background(&ctx);
    let _ = CTX.set(ctx);
    0
}

/// Runs a command; returns `{"ok": …}` or `{"err": "…"}` as JSON, to be freed with `cadence_free`.
#[unsafe(no_mangle)]
pub extern "C" fn cadence_call(cmd: *const c_char, args: *const c_char) -> *mut c_char {
    let reply = match CTX.get() {
        None => Err("Cadence hasn't started".to_string()),
        Some(ctx) => {
            let cmd = text(cmd);
            let mut args: Value = serde_json::from_str(&text(args)).unwrap_or(Value::Null);
            if args.is_null() {
                args = json!({});
            }
            dispatch(ctx, &cmd, args.clone()).unwrap_or_else(|| native(ctx, &cmd, &args))
        }
    };
    let out = match reply {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e }),
    };
    CString::new(out.to_string()).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn cadence_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: `s` came from `CString::into_raw` in `cadence_call`.
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Saves what must survive a quit (the listening history). Called when the app closes.
#[unsafe(no_mangle)]
pub extern "C" fn cadence_stop() {
    if let Some(ctx) = CTX.get() {
        ctx.state.save_history();
    }
}

macro_rules! ffi_dispatch {
    ($($kind:ident $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        /// The shared commands (api.rs); None for a name that isn't one of them.
        fn dispatch(ctx: &Ctx, cmd: &str, args: Value) -> Option<Result<Value, String>> {
            match cmd {
                $(stringify!($name) => Some(ffi_one!($kind ctx, args, $name($($arg: $ty),*))),)*
                _ => None,
            }
        }
    };
}

macro_rules! ffi_one {
    ($kind:ident $ctx:ident, $args:ident, $name:ident($($arg:ident: $ty:ty),*)) => {{
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        #[allow(dead_code)]
        struct Args {
            $($arg: $ty,)*
        }
        match serde_json::from_value::<Args>($args) {
            Err(e) => Err(format!("{}: {e}", stringify!($name))),
            #[allow(unused_variables)]
            Ok(a) => ffi_result!($kind crate::api::$name($ctx, $(a.$arg),*)),
        }
    }};
}

macro_rules! ffi_result {
    (plain $e:expr) => {
        serde_json::to_value($e).map_err(|e| e.to_string())
    };
    (fallible $e:expr) => {
        $e.and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
    };
    (blocking $e:expr) => {
        ffi_result!(fallible $e)
    };
}

crate::cadence_commands!(ffi_dispatch);

/// Commands only the native app needs.
fn native(ctx: &Ctx, cmd: &str, args: &Value) -> Result<Value, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    match cmd {
        // The pre-blurred cover behind a page, the player or Now Playing.
        "blurred" => Ok(json!(blurred(&ctx.state.data_dir, Path::new(&s("path")), &s("kind")))),
        // The UI's settings (theme, sort orders…) live with the rest of Cadence's data.
        "ui_settings" => Ok(crate::state::read_json::<Value>(&ctx.state.data_dir.join("native-ui.json"))),
        "set_ui_settings" => {
            crate::state::write_json(&ctx.state.data_dir.join("native-ui.json"), args.get("settings").unwrap_or(&Value::Null));
            Ok(Value::Null)
        }
        // Updates (update.rs): look, download (progress as "update-progress" events), install.
        "update_check" => crate::update::check().map(|a| json!(a)).map_err(|e| format!("{e:#}")),
        "update_download" => {
            let dir = std::env::temp_dir().join("cadence-update");
            let mut last = -1.0;
            crate::update::download(&dir, |f| {
                if f - last >= 0.01 || f >= 1.0 {
                    last = f;
                    ctx.emit("update-progress", f);
                }
            })
            .map(|p| json!(p.to_string_lossy()))
            .map_err(|e| format!("{e:#}"))
        }
        "update_install" => crate::update::install(Path::new(&s("path"))).map(|_| Value::Null).map_err(|e| format!("{e:#}")),
        "app_version" => Ok(json!(env!("CARGO_PKG_VERSION"))),
        _ => Err(format!("unknown command {cmd}")),
    }
}

/// A small, heavily blurred and saturated copy of a cover, made once and kept: the native UI
/// stretches it over the window instead of blurring the full cover on the GPU every frame.
/// The blur is the web UI's CSS one (blur(σ) saturate(s) brightness(b)) scaled to 128 px.
fn blurred(data_dir: &Path, cover: &Path, kind: &str) -> Option<String> {
    // (σ as a share of the shown image's width, saturation, brightness)
    let (sigma, sat, bright) = match kind {
        "np" => (0.052, 1.6, 0.7),
        "glow" => (0.12, 1.7, 1.0),
        // The mini player's wash: blur(36px) saturate(1.6) over a cover 840 px wide.
        "mini" => (0.043, 1.6, 1.0),
        _ => (0.069, 1.5, 1.0),
    };
    let dir = data_dir.join("art-blur");
    let stem = cover.file_stem()?.to_string_lossy();
    let out = dir.join(format!("{stem}-{kind}.png"));
    if out.exists() {
        return Some(out.to_string_lossy().into_owned());
    }
    let _ = std::fs::create_dir_all(&dir);
    const SIZE: u32 = 128;
    let img = image::open(cover).ok()?.resize_to_fill(SIZE, SIZE, image::imageops::FilterType::Triangle);
    let mut img = image::imageops::blur(&img.to_rgba8(), sigma * SIZE as f32);
    for p in img.pixels_mut() {
        let [r, g, b, a] = p.0.map(|c| c as f32);
        let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let f = |c: f32| ((l + (c - l) * sat) * bright).clamp(0.0, 255.0) as u8;
        p.0 = [f(r), f(g), f(b), a as u8];
    }
    img.save(&out).ok()?;
    Some(out.to_string_lossy().into_owned())
}

/// A small file logger (the Tauri app has tauri-plugin-log): `cadence.log`, started fresh when it
/// passes 1 MB, decoder chatter left out as in the Tauri app.
mod logging {
    use std::io::Write;
    use std::path::Path;
    use std::sync::Mutex;

    struct FileLog(Mutex<std::fs::File>);

    impl log::Log for FileLog {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Info && (!m.target().starts_with("symphonia") || m.level() <= log::Level::Error)
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                if let Ok(mut f) = self.0.lock() {
                    let now = crate::history::now();
                    let _ = writeln!(f, "[{now}][{}][{}] {}", r.target(), r.level(), r.args());
                }
            }
        }
        fn flush(&self) {}
    }

    pub fn init(dir: &Path) {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("cadence.log");
        let fresh = std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(true);
        let file = std::fs::OpenOptions::new().create(true).append(!fresh).write(true).truncate(fresh).open(&path);
        if let Ok(file) = file {
            if log::set_boxed_logger(Box::new(FileLog(Mutex::new(file)))).is_ok() {
                log::set_max_level(log::LevelFilter::Info);
            }
        }
    }
}
