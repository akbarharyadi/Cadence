//! The window's commands for Sync with phone (api.rs has one-line wrappers). The computer's
//! service exists only on the desktop; on the phone those commands say so.

use serde::Serialize;

use super::{Kind, Summary};
use crate::ctx::Ctx;

/// What the settings page shows about the service.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    /// The listener turned Phone sync on.
    pub enabled: bool,
    pub on: bool,
    pub address: String,
    pub computer: String,
    pub code: Option<String>,
    pub code_until: u64,
    /// The pairing code as a QR image (SVG), while pairing is open and the address is known.
    pub qr: Option<String>,
    pub phones: Vec<super::Phone>,
    /// An error from starting it, in words.
    pub error: String,
}

pub fn summary(ctx: &Ctx) -> Summary {
    super::summary(ctx)
}

/// What's kept, for the menus to say "Keep on phone" or "Remove from phone" without waiting.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub playlists: Vec<String>,
    pub albums: Vec<String>,
    /// Every library spelling of each kept artist (the menus have names, not keys).
    pub artist_names: Vec<String>,
    pub top: u32,
}

pub fn kept(ctx: &Ctx) -> Kept {
    let c = super::load(&ctx.state.data_dir);
    let ids = crate::identity::current();
    let mut artist_names = Vec::new();
    for key in &c.artists {
        let named: Vec<String> = ids.artists().iter().filter(|a| &a.key == key).flat_map(|a| a.names.iter().map(|(n, _)| n.clone())).collect();
        if named.is_empty() { artist_names.push(key.clone()) } else { artist_names.extend(named) }
    }
    Kept { playlists: c.playlists.into_iter().collect(), albums: c.albums.into_iter().collect(), artist_names, top: c.top }
}

/// Keeps a playlist, album or artist on the phone, or not; the new songs' keys are worked out
/// in the background so the phone needn't wait for them.
pub fn keep(ctx: &Ctx, kind: Kind, id: &str, keep: bool) {
    super::change(&ctx.state.data_dir, |c| super::keep(c, kind, id, keep));
    changed(ctx);
}

pub fn set_top(ctx: &Ctx, top: u32) {
    super::change(&ctx.state.data_dir, |c| c.top = super::top_choice(top));
    changed(ctx);
}

fn changed(ctx: &Ctx) {
    ctx.emit("phone-changed", ());
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name("cadence-phone-keys".into()).spawn(move || super::prepare_keys(&ctx));
}

// ---------- the phone's side ----------

/// What the phone's settings show about its computer.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkView {
    /// This device can sync from a computer (the phone).
    pub supported: bool,
    pub paired: bool,
    pub computer: String,
    pub address: String,
    /// Where synced music goes.
    pub folder: String,
    pub songs: usize,
    pub last_sync: u64,
    pub last_result: String,
}

pub fn link_view(ctx: &Ctx) -> LinkView {
    let l = super::client::load_link();
    let ledger = super::sync_job::load_ledger(&ctx.state.data_dir);
    LinkView {
        supported: super::sync_job::ROOT.is_some(),
        paired: l.paired(),
        computer: l.computer,
        address: l.address,
        folder: super::sync_job::ROOT.unwrap_or_default().to_string(),
        songs: ledger.files.len(),
        last_sync: ledger.last_sync,
        last_result: ledger.last_result,
    }
}

/// What a pairing link (from a QR code) offers, for the phone to ask about before pairing.
pub fn pair_offer(link: &str) -> Result<super::pair_link::Offer, String> {
    super::pair_link::parse(link)
}

pub fn link_pair(ctx: &Ctx, address: &str, code: &str, name: &str) -> Result<LinkView, String> {
    super::client::pair(address, code, name)?;
    Ok(link_view(ctx))
}

pub fn link_forget(ctx: &Ctx) -> Result<LinkView, String> {
    super::client::forget()?;
    Ok(link_view(ctx))
}

/// Starts a sync. False: one is running already.
pub fn sync_now(ctx: &Ctx) -> Result<bool, String> {
    let root = super::sync_job::ROOT.ok_or("Syncing from a computer is done on the phone.")?;
    std::fs::create_dir_all(root).map_err(|e| format!("Couldn't make the folder {root}: {e}"))?;
    // The synced music is part of the library: its folder is added unless a music folder has it.
    let listed = ctx.state.settings.lock().folders.iter().any(|f| std::path::Path::new(root).starts_with(f));
    if !listed {
        crate::api::add_folders(ctx, vec![root.to_string()]);
    }
    Ok(super::sync_job::start(ctx, root.into()))
}

/// Pauses the job running on this device (the computer prepares songs, the phone syncs).
pub fn pause(ctx: &Ctx, paused: bool) {
    super::keys::pause(ctx, paused);
    super::sync_job::pause(ctx, paused);
}

pub fn sync_stop(ctx: &Ctx) {
    super::sync_job::stop(ctx);
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop {
    use super::*;
    use crate::phone::server;

    pub fn service(ctx: &Ctx, error: String) -> Service {
        let s = server::status();
        let qr = s.code.as_deref().filter(|_| !s.address.is_empty()).and_then(|c| crate::phone::pair_link::qr_svg(&crate::phone::pair_link::make(&s.address, c, &s.computer)));
        Service {
            enabled: ctx.state.settings.lock().phone_sync,
            on: s.on,
            address: s.address,
            computer: s.computer,
            code: s.code,
            code_until: s.code_until,
            qr,
            phones: crate::phone::load(&ctx.state.data_dir).phones,
            error,
        }
    }

    pub fn set_enabled(ctx: &Ctx, enabled: bool) -> Service {
        ctx.state.settings.lock().phone_sync = enabled;
        ctx.state.save_settings_later();
        let error = if enabled { server::start(ctx).err().unwrap_or_default() } else { server::stop(); String::new() };
        service(ctx, error)
    }

    pub fn start_if_on(ctx: &Ctx) {
        if !ctx.state.settings.lock().phone_sync {
            return;
        }
        if let Err(e) = server::start(ctx) {
            log::warn!("phone sync not started: {e}");
        }
    }

    pub fn pair(ctx: &Ctx) -> Result<Service, String> {
        if !server::status().on {
            return Err("Turn on Phone sync first.".into());
        }
        server::start_pairing()?;
        Ok(service(ctx, String::new()))
    }

    pub fn unpair(ctx: &Ctx, phone_id: &str) -> Result<Service, String> {
        server::unpair(ctx, phone_id)?;
        Ok(service(ctx, String::new()))
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod desktop {
    use super::*;

    const ONLY_DESKTOP: &str = "Phone sync is set up on the computer.";

    pub fn service(_ctx: &Ctx, _error: String) -> Service {
        Service::default()
    }

    pub fn set_enabled(_ctx: &Ctx, _enabled: bool) -> Service {
        Service { error: ONLY_DESKTOP.into(), ..Default::default() }
    }

    pub fn start_if_on(_ctx: &Ctx) {}

    pub fn pair(_ctx: &Ctx) -> Result<Service, String> {
        Err(ONLY_DESKTOP.into())
    }

    pub fn unpair(_ctx: &Ctx, _phone_id: &str) -> Result<Service, String> {
        Err(ONLY_DESKTOP.into())
    }
}

pub use desktop::{pair, set_enabled, start_if_on, unpair};

pub fn service(ctx: &Ctx) -> Service {
    desktop::service(ctx, String::new())
}
