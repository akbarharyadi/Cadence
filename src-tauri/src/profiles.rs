//! Sound settings per output device: the equalizer, crossfeed, "keep it full when quieter", Direct
//! output and the volume are remembered for each device (by its id, `audio::devices`), and come back
//! when playback moves to it. The settings in use (`Settings::dsp`, `Settings::volume`) are always
//! the current device's: a change to them is saved as that device's (`remember`), and moving to
//! another device puts its own in their place (`Profiles::switch_to`, asked by the engine).
//!
//! Everything else (Sound Check, Smart EQ, crossfade, sing-along) stays one setting for all devices.
//! With "Remember sound settings per device" off, one set for all, as before; the volume each
//! device last played at is still kept (`Profiles::levels`), for the engine's hearing-safety rule.
//!
//! Which device the settings in use belong to is the engine's to say (it's the one that opened
//! it): `current` is only this file's record of it, and is repaired from the engine whenever the
//! two disagree (settings replaced by a restore, edited by hand...). Nothing here is applied to the
//! open device because the file says so.

use std::collections::BTreeMap;
use std::sync::Weak;

use serde::{Deserialize, Serialize};

use crate::audio::crossfeed::CrossfeedSettings;
use crate::audio::eq::EqSettings;
use crate::audio::{Cmd, DeviceRef, DspSettings};
use crate::state::AppState;

/// The volume a device without one of its own starts at when Direct output is on for it: 30 % on
/// the volume slider (the engine's volume is the slider's value squared). Direct output sends full
/// scale to the DAC, with no Windows volume after it. (The engine enforces it: `audio`.)
pub const NEW_DIRECT_VOLUME: f32 = crate::audio::NEW_DIRECT_VOLUME;

/// One device's own settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    /// The device's name when last seen (shown while it isn't connected).
    pub name: String,
    pub eq: EqSettings,
    pub crossfeed: CrossfeedSettings,
    /// "Keep it full when quieter" (loudness compensation depends on how loud the device plays).
    pub loudness: bool,
    /// Direct output (WASAPI exclusive), and its fixed rate (0: each song's own).
    pub exclusive: bool,
    pub exclusive_rate: u32,
    /// None: not known yet (`start_volume` decides).
    pub volume: Option<f32>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            name: String::new(),
            eq: EqSettings::default(),
            crossfeed: CrossfeedSettings::default(),
            // As a new install has it.
            loudness: crate::audio::tone::ToneSettings::default().loudness,
            exclusive: false,
            exclusive_rate: 0,
            // A volume that isn't in the file isn't known (`start_volume` decides), not 100 %.
            volume: None,
        }
    }
}

impl Profile {
    pub fn of(name: &str, dsp: &DspSettings, volume: f32) -> Self {
        Profile {
            name: name.to_string(),
            eq: dsp.eq.clone(),
            crossfeed: dsp.crossfeed,
            loudness: dsp.tone.loudness,
            exclusive: dsp.exclusive,
            exclusive_rate: dsp.exclusive_rate,
            volume: Some(volume),
        }
    }

    /// Puts this device's settings into `dsp` (the rest of it is left as it is).
    pub fn put_into(&self, dsp: &mut DspSettings) {
        dsp.eq = self.eq.clone();
        dsp.crossfeed = self.crossfeed;
        dsp.tone.loudness = self.loudness;
        dsp.exclusive = self.exclusive;
        dsp.exclusive_rate = self.exclusive_rate;
    }
}

/// The volume a device starts at when it has none of its own: the volume in use, but no louder
/// than `NEW_DIRECT_VOLUME` when Direct output is on for it.
pub fn start_volume(current: f32, direct: bool) -> f32 {
    if direct { current.min(NEW_DIRECT_VOLUME) } else { current }
}

/// The volume last in effect on one device, through the system mixer and with Direct output (they
/// aren't comparable: Direct output has no system volume after it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Levels {
    pub shared: Option<f32>,
    pub direct: Option<f32>,
}

impl Levels {
    fn get(&self, direct: bool) -> Option<f32> {
        if direct { self.direct } else { self.shared }
    }

    fn set(&mut self, direct: bool, volume: f32) {
        *(if direct { &mut self.direct } else { &mut self.shared }) = Some(volume);
    }
}

/// A volume from the file, if it's one (a damaged or hand-edited file can hold anything; what
/// isn't a volume counts as not known, so the cautious rule applies).
fn sane(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profiles {
    /// The device whose settings `Settings::dsp` and `Settings::volume` are, as last recorded
    /// (the engine's open device decides; this follows it).
    pub current: Option<String>,
    pub devices: BTreeMap<String, Profile>,
    /// The volume last in effect on each device (by id), kept whether or not sound settings are
    /// remembered per device: playback moving to a device never plays it louder than this.
    pub levels: BTreeMap<String, Levels>,
}

impl Profiles {
    /// A change to the settings in use: kept as the current device's.
    pub fn record(&mut self, dsp: &DspSettings, volume: f32) {
        self.record_named("", dsp, volume);
    }

    /// `record`, naming a device seen for the first time `name`.
    fn record_named(&mut self, name: &str, dsp: &DspSettings, volume: f32) {
        let Some(id) = &self.current else { return };
        let name = match self.devices.get(id) {
            Some(p) if !p.name.trim().is_empty() => p.name.clone(),
            _ => name.to_string(),
        };
        self.devices.insert(id.clone(), Profile::of(&name, dsp, volume));
    }

    /// The settings in use are `device`'s (the engine has it open): `current` points at it, and
    /// they're kept as its own.
    pub fn record_for(&mut self, device: &DeviceRef, dsp: &DspSettings, volume: f32) {
        self.point_at(device);
        self.record_named(&device.name, dsp, volume);
    }

    /// `current` is `device` (repaired when the file said otherwise); a known device takes the
    /// name the system has for it now.
    pub fn point_at(&mut self, device: &DeviceRef) {
        if self.current.as_deref() != Some(device.id.as_str()) {
            if let Some(was) = &self.current {
                log::info!("the sound settings in use were recorded as {was}'s, but the engine has {} open: now its", device.id);
            }
            self.current = Some(device.id.clone());
        }
        if let Some(p) = self.devices.get_mut(&device.id).filter(|_| !device.name.trim().is_empty()) {
            p.name = device.name.clone();
        }
    }

    /// The volume last in effect on device `id` in this mode (Direct output or not), if known: its
    /// level, else its profile's volume when the profile is for that mode.
    pub fn saved_volume(&self, id: &str, direct: bool) -> Option<f32> {
        sane(self.levels.get(id).and_then(|l| l.get(direct)))
            .or_else(|| sane(self.devices.get(id).filter(|p| p.exclusive == direct).and_then(|p| p.volume)))
    }

    /// The volume in effect on device `id` in this mode now.
    pub fn set_level(&mut self, id: &str, direct: bool, volume: f32) {
        if volume.is_finite() {
            self.levels.entry(id.to_string()).or_default().set(direct, volume.clamp(0.0, 1.0));
        }
    }

    /// Playback moves to `device`: what's in use stays with the device it leaves, and `device`'s
    /// own settings and volume come into `dsp` and `volume`.
    /// - `from`: the device the engine had open until now, whose the settings in use are (`current`
    ///   is repaired to it). None: nothing was open yet this run, and the file's `current` says.
    /// - The first device ever (no profiles yet: the first start after the upgrade) keeps the
    ///   settings in use, volume and all: nothing changes.
    /// - The device already in use (`from` is `device`) keeps them too: nothing of another
    ///   device's is put on it.
    /// - A device seen before gets its own, at exactly its own volume (never louder).
    /// - A new device starts from the settings in use, at `start_volume`.
    ///
    /// The engine still holds whatever comes out to its safety rule (`audio`).
    pub fn switch_to(&mut self, from: Option<&DeviceRef>, device: &DeviceRef, dsp: &mut DspSettings, volume: &mut f32) {
        let first = self.devices.is_empty();
        if let Some(f) = from {
            // What's in use is the engine's on `from`: kept as its own (also when it's `device`).
            self.record_for(f, dsp, *volume);
        }
        if self.current.as_deref() == Some(device.id.as_str()) && self.devices.contains_key(&device.id) {
            self.point_at(device);
            if from.is_none() {
                self.record(dsp, *volume);
            }
            return;
        }
        if self.current.is_some() && from.is_none() {
            self.record(dsp, *volume);
        }
        match self.devices.get_mut(&device.id) {
            Some(p) => {
                if !device.name.trim().is_empty() {
                    p.name = device.name.clone();
                }
                p.put_into(dsp);
                *volume = p.volume.unwrap_or_else(|| start_volume(*volume, p.exclusive));
                p.volume = Some(*volume);
            }
            None => {
                if !first {
                    *volume = start_volume(*volume, dsp.exclusive);
                }
                self.devices.insert(device.id.clone(), Profile::of(&device.name, dsp, *volume));
            }
        }
        self.current = Some(device.id.clone());
    }
}

/// The engine's book of sound settings (`Cmd::SetProfiles`), over the app's settings. The engine
/// calls it on its worker thread (never in the audio callback).
struct Book {
    state: Weak<AppState>,
}

impl crate::audio::SoundBook for Book {
    fn moved(&mut self, from: Option<&DeviceRef>, to: &DeviceRef, dsp: &DspSettings, volume: f32) -> Option<(DspSettings, f32)> {
        let state = self.state.upgrade()?;
        let applied = moved_in(&mut state.settings.lock(), from, to, dsp, volume);
        if applied.is_some() {
            state.save_settings_later();
        }
        applied
    }

    fn saved_volume(&mut self, id: &str, direct: bool) -> Option<f32> {
        let state = self.state.upgrade()?;
        let s = state.settings.lock();
        s.sound_profiles.saved_volume(id, direct)
    }

    fn applied(&mut self, device: Option<&DeviceRef>, direct: bool, dsp: &DspSettings, volume: f32, in_use: bool) {
        let Some(state) = self.state.upgrade() else { return };
        if applied_in(&mut state.settings.lock(), device, direct, dsp, volume, in_use) {
            state.save_settings_later();
        }
    }
}

/// `SoundBook::moved` over these settings: with sound settings per device, `to`'s own (also
/// written as the ones in use); off, nothing.
pub(crate) fn moved_in(s: &mut crate::state::Settings, from: Option<&DeviceRef>, to: &DeviceRef, dsp: &DspSettings, volume: f32) -> Option<(DspSettings, f32)> {
    if !s.device_profiles {
        return None;
    }
    let (mut d, mut v) = (dsp.clone(), volume);
    s.sound_profiles.switch_to(from, to, &mut d, &mut v);
    s.dsp = d.clone();
    s.volume = v;
    Some((d, v))
}

/// The book over settings of their own (tests: the engine's, without the app around it).
#[cfg(test)]
pub(crate) fn settings_book(settings: std::sync::Arc<parking_lot::Mutex<crate::state::Settings>>) -> crate::audio::ProfileSource {
    struct Own(std::sync::Arc<parking_lot::Mutex<crate::state::Settings>>);
    impl crate::audio::SoundBook for Own {
        fn moved(&mut self, from: Option<&DeviceRef>, to: &DeviceRef, dsp: &DspSettings, volume: f32) -> Option<(DspSettings, f32)> {
            moved_in(&mut self.0.lock(), from, to, dsp, volume)
        }
        fn saved_volume(&mut self, id: &str, direct: bool) -> Option<f32> {
            self.0.lock().sound_profiles.saved_volume(id, direct)
        }
        fn applied(&mut self, device: Option<&DeviceRef>, direct: bool, dsp: &DspSettings, volume: f32, in_use: bool) {
            let _ = applied_in(&mut self.0.lock(), device, direct, dsp, volume, in_use);
        }
    }
    Box::new(Own(settings))
}

/// What the engine has in effect, written into the settings: the device's level is kept, and with
/// sound settings per device they're that device's (`current` repaired to it). `in_use`: the
/// engine changed them by itself, so the settings in use become exactly the engine's too (else
/// they're the listener's change, which `set_volume` / `set_dsp` already wrote).
/// Returns whether there was anything to write.
pub(crate) fn applied_in(s: &mut crate::state::Settings, device: Option<&DeviceRef>, direct: bool, dsp: &DspSettings, volume: f32, in_use: bool) -> bool {
    let device = device.filter(|d| !d.id.is_empty());
    if let Some(d) = device {
        s.sound_profiles.set_level(&d.id, direct, volume);
        if s.device_profiles {
            s.sound_profiles.record_for(d, dsp, volume);
        }
    }
    if in_use {
        if volume.is_finite() {
            s.volume = volume;
        }
        if s.dsp != *dsp {
            s.dsp = dsp.clone();
        }
    }
    device.is_some() || in_use
}

/// The engine's book over these settings (asked when playback moves to another device, and told
/// what it has in effect).
pub fn source(state: Weak<AppState>) -> crate::audio::ProfileSource {
    Box::new(Book { state })
}

/// The volume slider (`api::set_volume`): to the engine, stamped with the device it's meant for
/// (`seen`: the one the window last heard of, else the engine's open one now), so a change still
/// on its way when playback moves to another device is dropped there instead of landing on it.
/// The settings take it only while that device is still the one open; the engine writes back what
/// it has in effect either way (`SoundBook::applied`), and keeps it as the device's own.
pub fn set_volume(state: &AppState, volume: f32, seen: Option<u64>) {
    let device_gen = seen.unwrap_or_else(|| state.engine.device_generation());
    state.engine.send(Cmd::SetVolumeFor { volume, device_gen });
    let mut s = state.settings.lock();
    // Checked under the settings' lock: the engine moves on to another device before it writes its
    // settings there, so either this lands first (and is overwritten) or it isn't written.
    if state.engine.device_generation() == device_gen && volume.is_finite() {
        s.volume = volume.clamp(0.0, 1.0);
    }
}

/// The Sound page's settings (`api::set_dsp`), stamped like `set_volume`.
pub fn set_dsp(state: &AppState, dsp: DspSettings, seen: Option<u64>) {
    let device_gen = seen.unwrap_or_else(|| state.engine.device_generation());
    state.engine.send(Cmd::SetDspFor { dsp: dsp.clone(), device_gen });
    let mut s = state.settings.lock();
    if state.engine.device_generation() == device_gen {
        s.dsp = dsp;
    }
}

/// "Remember sound settings per device" on or off. Turned on, the settings in use become the
/// device's that's playing (`device`: the engine's), so nothing changes there.
pub fn set_enabled(state: &AppState, on: bool, device: Option<&DeviceRef>) {
    let mut guard = state.settings.lock();
    let s = &mut *guard;
    s.device_profiles = on;
    if on {
        enable(&mut s.sound_profiles, device, &s.dsp, s.volume);
    }
}

fn enable(profiles: &mut Profiles, device: Option<&DeviceRef>, dsp: &DspSettings, volume: f32) {
    let Some(d) = device else { return };
    profiles.current = Some(d.id.clone());
    let name = if d.name.trim().is_empty() { profiles.devices.get(&d.id).map(|p| p.name.clone()).unwrap_or_default() } else { d.name.clone() };
    profiles.devices.insert(d.id.clone(), Profile::of(&name, dsp, volume));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::crossfeed::CrossfeedLevel;

    fn dev(id: &str) -> DeviceRef {
        DeviceRef { id: id.into(), name: format!("{id} name") }
    }

    /// Settings with the EQ on a preset, crossfeed on and Direct output as given.
    fn tuned(preset: &str, crossfeed: bool, exclusive: bool) -> DspSettings {
        let mut d = DspSettings::default();
        d.eq.enabled = true;
        d.eq.preset = preset.into();
        d.eq.gains[0] = 3.0;
        d.crossfeed.enabled = crossfeed;
        d.crossfeed.level = CrossfeedLevel::Cmoy;
        d.exclusive = exclusive;
        d
    }

    #[test]
    fn the_first_device_after_the_upgrade_keeps_everything() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Rock", true, true), 1.0);
        let before = dsp.clone();
        p.switch_to(None, &dev("dac"), &mut dsp, &mut volume);
        assert_eq!((dsp, volume), (before.clone(), 1.0), "nothing reset, not even a Direct output volume at 100 %");
        assert_eq!(p.current.as_deref(), Some("dac"));
        assert_eq!(p.devices["dac"], Profile::of("dac name", &before, 1.0));
    }

    #[test]
    fn each_device_gets_its_own_settings_back() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Rock", false, true), 0.5);
        p.switch_to(None, &dev("dac"), &mut dsp, &mut volume);
        // To the headphones (new: starts from what's in use), then tuned for them.
        p.switch_to(None, &dev("phones"), &mut dsp, &mut volume);
        dsp.eq.preset = "Vocal".into();
        dsp.crossfeed.enabled = true;
        dsp.exclusive = false;
        volume = 0.2;
        p.record(&dsp, volume);
        // Back to the DAC: its own, exactly.
        p.switch_to(None, &dev("dac"), &mut dsp, &mut volume);
        assert_eq!((dsp.eq.preset.as_str(), dsp.crossfeed.enabled, dsp.exclusive, volume), ("Rock", false, true, 0.5));
        // And the headphones' again.
        p.switch_to(None, &dev("phones"), &mut dsp, &mut volume);
        assert_eq!((dsp.eq.preset.as_str(), dsp.crossfeed.enabled, dsp.exclusive, volume), ("Vocal", true, false, 0.2));
    }

    #[test]
    fn global_settings_stay_global() {
        let mut p = Profiles::default();
        let mut dsp = tuned("Rock", false, false);
        let mut volume = 0.5;
        p.switch_to(None, &dev("a"), &mut dsp, &mut volume);
        p.switch_to(None, &dev("b"), &mut dsp, &mut volume);
        dsp.vocal_cut = 0.5;
        dsp.crossfade.enabled = true;
        dsp.tone.smart = false;
        p.record(&dsp, volume);
        p.switch_to(None, &dev("a"), &mut dsp, &mut volume);
        assert_eq!((dsp.vocal_cut, dsp.crossfade.enabled, dsp.tone.smart), (0.5, true, false), "not per device");
    }

    #[test]
    fn a_new_device_with_direct_output_starts_quietly_and_never_louder_than_its_own() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Flat", false, false), 0.8);
        p.switch_to(None, &dev("speakers"), &mut dsp, &mut volume);
        // Direct output on, then a new DAC: capped.
        dsp.exclusive = true;
        p.record(&dsp, volume);
        p.switch_to(None, &dev("dac"), &mut dsp, &mut volume);
        assert_eq!(volume, NEW_DIRECT_VOLUME);
        // A new device without Direct output: the volume in use.
        dsp.exclusive = false;
        volume = 0.6;
        p.record(&dsp, volume);
        p.switch_to(None, &dev("phones"), &mut dsp, &mut volume);
        assert_eq!(volume, 0.6);
        // Already quieter than the cap: stays.
        assert_eq!(start_volume(0.04, true), 0.04);
        // A device's own volume is used as it is, also below what's playing now.
        p.devices.get_mut("dac").unwrap().volume = Some(0.01);
        volume = 1.0;
        p.switch_to(None, &dev("dac"), &mut dsp, &mut volume);
        assert_eq!(volume, 0.01);
        // A profile without a volume (an older or edited file): the start rule, for its own mode.
        p.devices.get_mut("phones").unwrap().volume = None;
        p.devices.get_mut("phones").unwrap().exclusive = true;
        volume = 1.0;
        p.switch_to(None, &dev("phones"), &mut dsp, &mut volume);
        assert_eq!(volume, NEW_DIRECT_VOLUME);
    }

    #[test]
    fn changes_are_saved_into_the_current_device_only() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Flat", false, false), 0.5);
        // No device yet: nothing to save into.
        p.record(&dsp, volume);
        assert!(p.devices.is_empty());
        p.switch_to(None, &dev("a"), &mut dsp, &mut volume);
        p.switch_to(None, &dev("b"), &mut dsp, &mut volume);
        dsp.eq.preset = "Bass".into();
        p.record(&dsp, 0.3);
        assert_eq!((p.devices["b"].eq.preset.as_str(), p.devices["b"].volume), ("Bass", Some(0.3)));
        assert_eq!((p.devices["a"].eq.preset.as_str(), p.devices["a"].volume), ("Flat", Some(0.5)), "a is left alone");
        // The same device again: nothing changes, the name follows the system's.
        let mut renamed = dev("b");
        renamed.name = "Headphones (K7)".into();
        let before = (dsp.clone(), 0.3f32);
        let mut v = 0.3;
        p.switch_to(None, &renamed, &mut dsp, &mut v);
        assert_eq!((dsp, v), before);
        assert_eq!(p.devices["b"].name, "Headphones (K7)");
    }

    #[test]
    fn turning_it_on_keeps_what_plays_now() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Rock", false, false), 0.5);
        p.switch_to(None, &dev("a"), &mut dsp, &mut volume);
        // Turned off, then the user moved to b and changed things; turned on again there.
        dsp.eq.preset = "Jazz".into();
        enable(&mut p, Some(&dev("b")), &dsp, 0.7);
        assert_eq!(p.current.as_deref(), Some("b"));
        assert_eq!((p.devices["b"].eq.preset.as_str(), p.devices["b"].volume), ("Jazz", Some(0.7)));
        assert_eq!(p.devices["a"].eq.preset, "Rock");
    }

    #[test]
    fn an_old_settings_file_loads_unchanged_and_the_new_settings_round_trip() {
        let old: crate::state::Settings = serde_json::from_str(r#"{"folders":["D:\\Music"],"volume":0.4,"dsp":{"exclusive":true,"eq":{"enabled":true,"preset":"Rock"}}}"#).unwrap();
        assert_eq!((old.volume, old.dsp.exclusive, old.dsp.eq.preset.as_str()), (0.4, true, "Rock"));
        assert!(old.output_device.is_none(), "the system default, as before");
        assert!(old.device_profiles, "on");
        assert_eq!(old.sound_profiles, Profiles::default(), "no profiles yet: the first device takes the settings as they are");
        let mut s = old.clone();
        s.output_device = Some(DeviceRef { id: "wasapi:{0.0.0.00000000}.{dac}".into(), name: "Speakers (FiiO K7) ✓".into() });
        s.device_profiles = false;
        s.sound_profiles.switch_to(None, s.output_device.as_ref().unwrap(), &mut s.dsp, &mut s.volume);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"outputDevice\"") && json.contains("\"deviceProfiles\":false") && json.contains("\"soundProfiles\""));
        let back: crate::state::Settings = serde_json::from_str(&json).unwrap();
        assert_eq!((back.output_device, back.device_profiles, back.sound_profiles), (s.output_device, false, s.sound_profiles));
    }

    #[test]
    fn the_engines_open_device_decides_whose_the_settings_in_use_are() {
        let mut p = Profiles::default();
        p.devices.insert("a".into(), Profile::of("a name", &tuned("Rock", false, false), 0.5));
        p.devices.insert("b".into(), Profile::of("b name", &tuned("Vocal", true, true), 1.0));
        // A restored file says b's are in use; the engine has a open, with these.
        p.current = Some("b".into());
        let (mut dsp, mut volume) = (tuned("Jazz", false, false), 0.3);
        p.switch_to(Some(&dev("a")), &dev("a"), &mut dsp, &mut volume);
        assert_eq!((dsp.eq.preset.as_str(), dsp.exclusive, dsp.crossfeed.enabled, volume), ("Jazz", false, false, 0.3), "nothing of b's on a");
        assert_eq!(p.current.as_deref(), Some("a"), "repaired");
        assert_eq!((p.devices["a"].eq.preset.as_str(), p.devices["a"].volume), ("Jazz", Some(0.3)));
        assert_eq!(p.devices["b"], Profile::of("b name", &tuned("Vocal", true, true), 1.0), "b's own untouched");
        // On to a new device: the settings in use stay a's (not b's), c starts from them.
        p.current = Some("b".into());
        p.switch_to(Some(&dev("a")), &dev("c"), &mut dsp, &mut volume);
        assert_eq!((p.current.as_deref(), p.devices["a"].volume, p.devices["b"].volume), (Some("c"), Some(0.3), Some(1.0)));
        assert_eq!((dsp.eq.preset.as_str(), volume), ("Jazz", 0.3));
        // And to b: exactly its own.
        p.switch_to(Some(&dev("c")), &dev("b"), &mut dsp, &mut volume);
        assert_eq!((dsp.eq.preset.as_str(), dsp.exclusive, volume), ("Vocal", true, 1.0));
    }

    #[test]
    fn saved_volumes_are_per_mode_and_a_bad_file_counts_as_none() {
        let mut p = Profiles::default();
        assert_eq!(p.saved_volume("dac", true), None);
        p.devices.insert("dac".into(), Profile::of("DAC", &tuned("Flat", false, true), 0.25));
        assert_eq!((p.saved_volume("dac", true), p.saved_volume("dac", false)), (Some(0.25), None), "its volume is a Direct output one");
        p.set_level("dac", false, 0.8);
        p.set_level("dac", true, 0.1);
        assert_eq!((p.saved_volume("dac", true), p.saved_volume("dac", false)), (Some(0.1), Some(0.8)), "the level first");
        p.set_level("dac", true, f32::NAN);
        assert_eq!(p.saved_volume("dac", true), Some(0.1), "not taken");
        // Hand-edited or damaged: not a volume, so not known (the engine's cautious rule applies).
        let odd: Profiles = serde_json::from_str(r#"{"levels":{"dac":{"direct":5.0,"shared":-1}},"devices":{"dac":{"exclusive":true,"volume":3}}}"#).unwrap();
        assert_eq!((odd.saved_volume("dac", true), odd.saved_volume("dac", false)), (None, None));
    }

    #[test]
    fn what_the_engine_has_in_effect_is_written_back_and_repairs_current() {
        let mut s = crate::state::Settings::default();
        s.sound_profiles.current = Some("b".into());
        s.volume = 1.0;
        let dsp = tuned("Rock", false, true);
        applied_in(&mut s, Some(&dev("a")), true, &dsp, 0.09, true);
        assert_eq!((s.sound_profiles.current.as_deref(), s.volume, s.dsp.exclusive), (Some("a"), 0.09, true));
        assert_eq!((s.sound_profiles.saved_volume("a", true), s.sound_profiles.devices["a"].name.as_str()), (Some(0.09), "a name"));
        // Nothing opened yet: the settings in use only.
        applied_in(&mut s, None, true, &dsp, 0.05, true);
        assert_eq!((s.volume, s.sound_profiles.devices.len()), (0.05, 1));
        // One set for all: no profiles, but the level is still kept, and a move changes nothing.
        s.device_profiles = false;
        applied_in(&mut s, Some(&dev("c")), false, &dsp, 0.4, false);
        assert!(!s.sound_profiles.devices.contains_key("c"));
        assert_eq!(s.sound_profiles.saved_volume("c", false), Some(0.4));
        assert_eq!(moved_in(&mut s, Some(&dev("c")), &dev("a"), &dsp, 0.4), None);
    }

    #[test]
    fn profiles_round_trip_and_old_files_load() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Rock", true, true), 0.25);
        p.switch_to(None, &dev("wasapi:{0.0.0.00000000}.{abc}"), &mut dsp, &mut volume);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Profiles>(&json).unwrap(), p);
        // A profile written by a later version with fewer fields still loads.
        let partial: Profile = serde_json::from_str(r#"{"name":"DAC","exclusive":true}"#).unwrap();
        assert!(partial.exclusive && partial.volume.is_none() && partial.loudness);
    }
}
