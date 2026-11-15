//! Sound settings per output device: the equalizer, crossfeed, "keep it full when quieter", Direct
//! output and the volume are remembered for each device (by its id, `audio::devices`), and come back
//! when playback moves to it. The settings in use (`Settings::dsp`, `Settings::volume`) are always
//! the current device's: a change to them is saved as that device's (`remember`), and moving to
//! another device puts its own in their place (`Profiles::switch_to`, asked by the engine).
//!
//! Everything else (Sound Check, Smart EQ, crossfade, sing-along) stays one setting for all devices.
//! With "Remember sound settings per device" off, nothing here runs: one set for all, as before.

use std::collections::BTreeMap;
use std::sync::Weak;

use serde::{Deserialize, Serialize};

use crate::audio::crossfeed::CrossfeedSettings;
use crate::audio::eq::EqSettings;
use crate::audio::{DeviceRef, DspSettings};
use crate::state::AppState;

/// The volume a device without one of its own starts at when Direct output is on for it: 30 % on
/// the volume slider (the engine's volume is the slider's value squared). Direct output sends full
/// scale to the DAC, with no Windows volume after it.
pub const NEW_DIRECT_VOLUME: f32 = 0.09;

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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profiles {
    /// The device whose settings `Settings::dsp` and `Settings::volume` are.
    pub current: Option<String>,
    pub devices: BTreeMap<String, Profile>,
}

impl Profiles {
    /// A change to the settings in use: kept as the current device's.
    pub fn record(&mut self, dsp: &DspSettings, volume: f32) {
        let Some(id) = &self.current else { return };
        let name = self.devices.get(id).map(|p| p.name.clone()).unwrap_or_default();
        self.devices.insert(id.clone(), Profile::of(&name, dsp, volume));
    }

    /// Playback moves to `device`: what's in use stays with the device it leaves, and `device`'s
    /// own settings and volume come into `dsp` and `volume`.
    /// - The first device ever (no profiles yet: the first start after the upgrade) keeps the
    ///   settings in use, volume and all: nothing changes.
    /// - A device seen before gets its own, at exactly its own volume (never louder).
    /// - A new device starts from the settings in use, at `start_volume`.
    pub fn switch_to(&mut self, device: &DeviceRef, dsp: &mut DspSettings, volume: &mut f32) {
        if self.current.as_deref() == Some(device.id.as_str()) && self.devices.contains_key(&device.id) {
            if let Some(p) = self.devices.get_mut(&device.id).filter(|_| !device.name.trim().is_empty()) {
                p.name = device.name.clone();
            }
            return;
        }
        let first = self.devices.is_empty();
        if self.current.is_some() {
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

/// What the engine asks when playback moves to another device (`Cmd::SetProfiles`): with the
/// setting on, that device's settings (also saved as the ones in use); off, nothing.
pub fn source(state: Weak<AppState>) -> crate::audio::ProfileSource {
    Box::new(move |device: &DeviceRef| {
        let state = state.upgrade()?;
        let applied = {
            let mut guard = state.settings.lock();
            let s = &mut *guard;
            if !s.device_profiles {
                return None;
            }
            s.sound_profiles.switch_to(device, &mut s.dsp, &mut s.volume);
            (s.dsp.clone(), s.volume)
        };
        state.save_settings_later();
        Some(applied)
    })
}

/// The settings in use changed (the Sound page, the volume): saved as the current device's too.
pub fn remember(state: &AppState) {
    let mut guard = state.settings.lock();
    let s = &mut *guard;
    if s.device_profiles {
        s.sound_profiles.record(&s.dsp, s.volume);
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
        p.switch_to(&dev("dac"), &mut dsp, &mut volume);
        assert_eq!((dsp, volume), (before.clone(), 1.0), "nothing reset, not even a Direct output volume at 100 %");
        assert_eq!(p.current.as_deref(), Some("dac"));
        assert_eq!(p.devices["dac"], Profile::of("dac name", &before, 1.0));
    }

    #[test]
    fn each_device_gets_its_own_settings_back() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Rock", false, true), 0.5);
        p.switch_to(&dev("dac"), &mut dsp, &mut volume);
        // To the headphones (new: starts from what's in use), then tuned for them.
        p.switch_to(&dev("phones"), &mut dsp, &mut volume);
        dsp.eq.preset = "Vocal".into();
        dsp.crossfeed.enabled = true;
        dsp.exclusive = false;
        volume = 0.2;
        p.record(&dsp, volume);
        // Back to the DAC: its own, exactly.
        p.switch_to(&dev("dac"), &mut dsp, &mut volume);
        assert_eq!((dsp.eq.preset.as_str(), dsp.crossfeed.enabled, dsp.exclusive, volume), ("Rock", false, true, 0.5));
        // And the headphones' again.
        p.switch_to(&dev("phones"), &mut dsp, &mut volume);
        assert_eq!((dsp.eq.preset.as_str(), dsp.crossfeed.enabled, dsp.exclusive, volume), ("Vocal", true, false, 0.2));
    }

    #[test]
    fn global_settings_stay_global() {
        let mut p = Profiles::default();
        let mut dsp = tuned("Rock", false, false);
        let mut volume = 0.5;
        p.switch_to(&dev("a"), &mut dsp, &mut volume);
        p.switch_to(&dev("b"), &mut dsp, &mut volume);
        dsp.vocal_cut = 0.5;
        dsp.crossfade.enabled = true;
        dsp.tone.smart = false;
        p.record(&dsp, volume);
        p.switch_to(&dev("a"), &mut dsp, &mut volume);
        assert_eq!((dsp.vocal_cut, dsp.crossfade.enabled, dsp.tone.smart), (0.5, true, false), "not per device");
    }

    #[test]
    fn a_new_device_with_direct_output_starts_quietly_and_never_louder_than_its_own() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Flat", false, false), 0.8);
        p.switch_to(&dev("speakers"), &mut dsp, &mut volume);
        // Direct output on, then a new DAC: capped.
        dsp.exclusive = true;
        p.record(&dsp, volume);
        p.switch_to(&dev("dac"), &mut dsp, &mut volume);
        assert_eq!(volume, NEW_DIRECT_VOLUME);
        // A new device without Direct output: the volume in use.
        dsp.exclusive = false;
        volume = 0.6;
        p.record(&dsp, volume);
        p.switch_to(&dev("phones"), &mut dsp, &mut volume);
        assert_eq!(volume, 0.6);
        // Already quieter than the cap: stays.
        assert_eq!(start_volume(0.04, true), 0.04);
        // A device's own volume is used as it is, also below what's playing now.
        p.devices.get_mut("dac").unwrap().volume = Some(0.01);
        volume = 1.0;
        p.switch_to(&dev("dac"), &mut dsp, &mut volume);
        assert_eq!(volume, 0.01);
        // A profile without a volume (an older or edited file): the start rule, for its own mode.
        p.devices.get_mut("phones").unwrap().volume = None;
        p.devices.get_mut("phones").unwrap().exclusive = true;
        volume = 1.0;
        p.switch_to(&dev("phones"), &mut dsp, &mut volume);
        assert_eq!(volume, NEW_DIRECT_VOLUME);
    }

    #[test]
    fn changes_are_saved_into_the_current_device_only() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Flat", false, false), 0.5);
        // No device yet: nothing to save into.
        p.record(&dsp, volume);
        assert!(p.devices.is_empty());
        p.switch_to(&dev("a"), &mut dsp, &mut volume);
        p.switch_to(&dev("b"), &mut dsp, &mut volume);
        dsp.eq.preset = "Bass".into();
        p.record(&dsp, 0.3);
        assert_eq!((p.devices["b"].eq.preset.as_str(), p.devices["b"].volume), ("Bass", Some(0.3)));
        assert_eq!((p.devices["a"].eq.preset.as_str(), p.devices["a"].volume), ("Flat", Some(0.5)), "a is left alone");
        // The same device again: nothing changes, the name follows the system's.
        let mut renamed = dev("b");
        renamed.name = "Headphones (K7)".into();
        let before = (dsp.clone(), 0.3f32);
        let mut v = 0.3;
        p.switch_to(&renamed, &mut dsp, &mut v);
        assert_eq!((dsp, v), before);
        assert_eq!(p.devices["b"].name, "Headphones (K7)");
    }

    #[test]
    fn turning_it_on_keeps_what_plays_now() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Rock", false, false), 0.5);
        p.switch_to(&dev("a"), &mut dsp, &mut volume);
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
        s.sound_profiles.switch_to(s.output_device.as_ref().unwrap(), &mut s.dsp, &mut s.volume);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"outputDevice\"") && json.contains("\"deviceProfiles\":false") && json.contains("\"soundProfiles\""));
        let back: crate::state::Settings = serde_json::from_str(&json).unwrap();
        assert_eq!((back.output_device, back.device_profiles, back.sound_profiles), (s.output_device, false, s.sound_profiles));
    }

    #[test]
    fn profiles_round_trip_and_old_files_load() {
        let mut p = Profiles::default();
        let (mut dsp, mut volume) = (tuned("Rock", true, true), 0.25);
        p.switch_to(&dev("wasapi:{0.0.0.00000000}.{abc}"), &mut dsp, &mut volume);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Profiles>(&json).unwrap(), p);
        // A profile written by a later version with fewer fields still loads.
        let partial: Profile = serde_json::from_str(r#"{"name":"DAC","exclusive":true}"#).unwrap();
        assert!(partial.exclusive && partial.volume.is_none() && partial.loudness);
    }
}
