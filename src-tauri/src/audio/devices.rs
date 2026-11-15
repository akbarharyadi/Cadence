//! Output devices: which ones there are, which one to open, and (Windows) hearing when they change.
//!
//! A device is known by cpal's stable id as text (`DeviceId`'s `Display`: "wasapi:{0.0.0.00000000}.{…}"
//! on Windows, "coreaudio:<uid>" on macOS, "alsa:hw:CARD=…" on Linux), which stays the same across
//! restarts and replugging. On Windows the part after "wasapi:" is the endpoint id WASAPI itself
//! uses, so shared mode (cpal) and Direct output (the `wasapi` crate) open the very same device.
//!
//! Windows: Cadence never opens cpal's "default device" handle. cpal opens that one through
//! `ActivateAudioInterfaceAsync`, which can fail for the rest of the run once the process has a
//! multithreaded COM thread (cpal issue #1402; the exclusive stream and the device watcher are
//! such threads). The default is looked up by id instead and opened like any other device, and
//! following it when it changes is done here, from WASAPI's own notifications.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// A device as remembered in the settings: its id, and its name for when it isn't connected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeviceRef {
    pub id: String,
    pub name: String,
}

/// A connected output device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub name: String,
    /// The system's default output.
    pub is_default: bool,
}

/// The connected output devices, and the system's default (which may be missing from the list:
/// Linux's "default" PCM, for one).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Present {
    pub devices: Vec<Listed>,
    pub default: Option<DeviceRef>,
}

impl Present {
    pub fn has(&self, id: &str) -> bool {
        self.devices.iter().any(|d| d.id == id)
    }

    /// The default device's name, to offer playing there.
    pub fn default_name(&self) -> Option<String> {
        self.default.as_ref().map(|d| display_name(&d.name))
    }
}

/// What to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Open(DeviceRef),
    /// The chosen device isn't connected.
    Missing(DeviceRef),
    /// No output device at all.
    Nothing,
}

/// Which device to open: the chosen one if it's connected (whatever the system default is), the
/// system default when none is chosen.
pub fn choose(chosen: Option<&DeviceRef>, present: &Present) -> Target {
    match chosen {
        Some(c) => match present.devices.iter().find(|d| d.id == c.id) {
            // The name as the system has it now (renamed in Sound settings), else as remembered.
            Some(d) => Target::Open(DeviceRef { id: c.id.clone(), name: if d.name.trim().is_empty() { c.name.clone() } else { d.name.clone() } }),
            None => Target::Missing(c.clone()),
        },
        None => match &present.default {
            Some(d) => Target::Open(d.clone()),
            None => Target::Nothing,
        },
    }
}

/// A name fit to show: trimmed, control characters left out, never empty.
pub fn display_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.is_empty() { "Unnamed device".into() } else { clean.to_string() }
}

/// How long after the chosen device went away playback carries on by itself when it comes back.
pub const RESUME_WINDOW: Duration = Duration::from_secs(120);

/// Playback is paused because the chosen device isn't connected.
#[derive(Clone, Debug)]
pub struct Waiting {
    pub device: DeviceRef,
    /// Since when (or since Play was pressed while waiting).
    pub since: Instant,
    /// Music was playing (or Play was pressed): carry on when the device is back soon enough.
    pub resume: bool,
    /// The system default's name, to offer playing there instead.
    pub fallback: Option<String>,
}

impl Waiting {
    /// Whether playback should carry on by itself now that the device is back.
    pub fn resumes(&self, now: Instant) -> bool {
        self.resume && now.saturating_duration_since(self.since) <= RESUME_WINDOW
    }
}

/// The list for the device picker: the connected devices, then remembered ones that aren't
/// connected (the chosen one, and ones with sound settings of their own).
pub fn entries<'a>(present: &Present, chosen: Option<&'a DeviceRef>, remembered: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Entry> {
    let mut out: Vec<Entry> = present
        .devices
        .iter()
        .map(|d| Entry { id: d.id.clone(), name: display_name(&d.name), is_default: d.is_default, present: true })
        .collect();
    let absent = chosen.map(|c| (c.id.as_str(), c.name.as_str())).into_iter().chain(remembered);
    for (id, name) in absent {
        if !id.is_empty() && !out.iter().any(|e| e.id == id) {
            out.push(Entry { id: id.to_string(), name: display_name(name), is_default: false, present: false });
        }
    }
    out
}

/// One device of the picker's list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    /// Connected now (false: remembered, unplugged).
    pub present: bool,
}

/// The device with this id (cpal's `DeviceId` as text), if it's connected.
pub fn find(host: &cpal::Host, id: &str) -> Option<cpal::Device> {
    use cpal::traits::HostTrait;
    let id: cpal::DeviceId = id.parse().ok()?;
    host.device_by_id(&id)
}

/// The WASAPI endpoint id of a device id ("wasapi:{…}" → "{…}").
#[cfg(windows)]
pub fn endpoint_id(id: &str) -> &str {
    id.strip_prefix("wasapi:").unwrap_or(id)
}

/// The connected output devices, asked on this thread. Can take a moment (it asks the system);
/// never in the audio callback. The engine's worker asks on its own thread (where cpal keeps its
/// Windows device enumerator: it's made on the first thread that lists, and has to stay alive with
/// it); others use `present_elsewhere`.
#[cfg(not(target_os = "android"))]
pub fn present() -> anyhow::Result<Present> {
    list()
}

/// `present`, for threads of a shared pool (the window's commands): on Windows on a thread of its
/// own, so cpal's COM setup stays on that thread and goes with it.
#[cfg(not(target_os = "android"))]
pub fn present_elsewhere() -> anyhow::Result<Present> {
    #[cfg(windows)]
    return std::thread::Builder::new()
        .name("cadence-device-list".into())
        .spawn(list)?
        .join()
        .map_err(|_| anyhow::anyhow!("listing the output devices failed"))?;
    #[cfg(not(windows))]
    list()
}

#[cfg(not(target_os = "android"))]
fn list() -> anyhow::Result<Present> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let name = |d: &cpal::Device| d.description().map(|x| x.name().to_string()).unwrap_or_default();
    // Only the default's id and name: this doesn't open it (on Windows that's a plain endpoint lookup).
    let default = host
        .default_output_device()
        .and_then(|d| Some(DeviceRef { id: d.id().ok()?.to_string(), name: name(&d) }));
    let mut devices = Vec::new();
    for d in host.output_devices()? {
        let Ok(id) = d.id() else { continue };
        let id = id.to_string();
        if devices.iter().any(|x: &Listed| x.id == id) {
            continue;
        }
        let is_default = default.as_ref().is_some_and(|x| x.id == id);
        devices.push(Listed { name: name(&d), id, is_default });
    }
    // The default's own name when the list has it (cpal names the default handle after its endpoint too).
    let default = default.map(|d| match devices.iter().find(|x| x.id == d.id) {
        Some(x) if !x.name.trim().is_empty() => DeviceRef { id: d.id, name: x.name.clone() },
        _ => d,
    });
    Ok(Present { devices, default })
}

/// Windows: hears from WASAPI when output devices come, go or the default changes. The callbacks
/// only bump counters (they run on a Windows thread and must return at once); the engine's worker
/// looks at them on its next round.
#[cfg(windows)]
pub mod watch {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use wasapi::{DeviceEnumerator, DeviceEventCallbacks, Direction, Role};

    #[derive(Default)]
    pub struct Events {
        /// Bumped whenever a device came, went or changed state, or the default changed.
        pub changed: AtomicU64,
        /// The default output (render, console role) changed since the worker last looked.
        pub default_changed: AtomicBool,
    }

    /// Keeps the notifications coming; dropping it stops them (on the thread that registered them).
    pub struct Watcher {
        pub events: Arc<Events>,
        _stop: crossbeam_channel::Sender<()>,
    }

    pub fn start() -> Option<Watcher> {
        let events = Arc::new(Events::default());
        let (stop, stopped) = crossbeam_channel::bounded::<()>(0);
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<bool>(1);
        let ev = events.clone();
        std::thread::Builder::new()
            .name("cadence-devices".into())
            .spawn(move || {
                let _ = wasapi::initialize_mta();
                let registration = DeviceEnumerator::new().map_err(|e| e.to_string()).and_then(|enumerator| {
                    let mut callbacks = DeviceEventCallbacks::new();
                    let (a, b, c, d) = (ev.clone(), ev.clone(), ev.clone(), ev.clone());
                    callbacks.set_device_state_callback(move |_, _| {
                        a.changed.fetch_add(1, Ordering::AcqRel);
                    });
                    callbacks.set_device_added_callback(move |_| {
                        b.changed.fetch_add(1, Ordering::AcqRel);
                    });
                    callbacks.set_device_removed_callback(move |_| {
                        c.changed.fetch_add(1, Ordering::AcqRel);
                    });
                    callbacks.set_default_device_callback(move |direction, role, _| {
                        if direction == Direction::Render && role == Role::Console {
                            d.default_changed.store(true, Ordering::Release);
                            d.changed.fetch_add(1, Ordering::AcqRel);
                        }
                    });
                    enumerator.register_notification_callback(callbacks).map_err(|e| e.to_string())
                });
                match registration {
                    Ok(registration) => {
                        let _ = ready_tx.send(true);
                        // Until the engine goes; then unregistered here, on the thread that registered.
                        let _ = stopped.recv();
                        drop(registration);
                    }
                    Err(e) => {
                        log::warn!("can't hear about output device changes: {e}");
                        let _ = ready_tx.send(false);
                    }
                }
            })
            .ok()?;
        ready_rx.recv().ok().filter(|ok| *ok)?;
        Some(Watcher { events, _stop: stop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str, is_default: bool) -> Listed {
        Listed { id: id.into(), name: name.into(), is_default }
    }

    fn r(id: &str, name: &str) -> DeviceRef {
        DeviceRef { id: id.into(), name: name.into() }
    }

    fn present() -> Present {
        Present {
            devices: vec![dev("wasapi:dac", "Speakers (FiiO K7)", false), dev("wasapi:hdmi", "LG TV (HDMI)", true)],
            default: Some(r("wasapi:hdmi", "LG TV (HDMI)")),
        }
    }

    #[test]
    fn a_chosen_device_is_opened_whatever_the_default_is() {
        // The HDMI monitor became the default: playback stays on the DAC.
        assert_eq!(choose(Some(&r("wasapi:dac", "old name")), &present()), Target::Open(r("wasapi:dac", "Speakers (FiiO K7)")));
        // No choice: the default.
        assert_eq!(choose(None, &present()), Target::Open(r("wasapi:hdmi", "LG TV (HDMI)")));
    }

    #[test]
    fn a_missing_chosen_device_is_not_replaced_by_the_default() {
        let mut p = present();
        p.devices.retain(|d| d.id != "wasapi:dac");
        assert_eq!(choose(Some(&r("wasapi:dac", "FiiO K7")), &p), Target::Missing(r("wasapi:dac", "FiiO K7")));
        // Nothing connected at all.
        assert_eq!(choose(None, &Present::default()), Target::Nothing);
        assert_eq!(choose(Some(&r("wasapi:dac", "FiiO K7")), &Present::default()), Target::Missing(r("wasapi:dac", "FiiO K7")));
        // A device with no name keeps the remembered one.
        let p = Present { devices: vec![dev("wasapi:dac", "  ", false)], default: None };
        assert_eq!(choose(Some(&r("wasapi:dac", "FiiO K7")), &p), Target::Open(r("wasapi:dac", "FiiO K7")));
    }

    #[test]
    fn playback_resumes_only_if_it_was_playing_and_the_device_is_back_soon() {
        let start = Instant::now();
        let w = Waiting { device: r("a", "A"), since: start, resume: true, fallback: None };
        assert!(w.resumes(start + Duration::from_secs(5)));
        assert!(w.resumes(start + RESUME_WINDOW));
        assert!(!w.resumes(start + RESUME_WINDOW + Duration::from_secs(1)), "too late: stays paused");
        let paused = Waiting { resume: false, ..w };
        assert!(!paused.resumes(start + Duration::from_secs(1)), "was paused: stays paused");
    }

    #[test]
    fn the_list_shows_remembered_devices_that_are_unplugged() {
        let mut p = present();
        p.devices.retain(|d| d.id != "wasapi:dac");
        let list = entries(&p, Some(&r("wasapi:dac", "FiiO K7")), [("wasapi:hp", "Headphones"), ("wasapi:hdmi", "LG TV (HDMI)"), ("", "")]);
        assert_eq!(
            list.iter().map(|e| (e.id.as_str(), e.is_default, e.present)).collect::<Vec<_>>(),
            [("wasapi:hdmi", true, true), ("wasapi:dac", false, false), ("wasapi:hp", false, false)]
        );
        assert_eq!(list[1].name, "FiiO K7");
    }

    #[test]
    fn odd_names_are_shown_safely() {
        assert_eq!(display_name(""), "Unnamed device");
        assert_eq!(display_name(" \u{0}\u{7} "), "Unnamed device");
        assert_eq!(display_name("  ヘッドホン (USB DAC) \n"), "ヘッドホン (USB DAC)");
    }

    /// Lists the devices of this computer (enumeration only, nothing is opened). Manual:
    /// `cargo test --lib list_this_computers_devices -- --ignored --nocapture`.
    #[test]
    #[ignore]
    #[cfg(not(target_os = "android"))]
    fn list_this_computers_devices() {
        let p = super::present().expect("devices");
        println!("default: {:?}", p.default);
        for d in &p.devices {
            println!("{d:?}");
        }
        assert!(p.default.as_ref().is_none_or(|d| p.has(&d.id) || !cfg!(windows)));
    }
}
