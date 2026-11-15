//! The hearing-safety rule's arithmetic (the engine applies it in `open_output`, before the first
//! sample of every output it opens): how loud a device may play, from what was recorded on it.
//!
//! The rule: a device's level in a mode (Direct output, or through the system mixer) is only ever
//! what was in effect while that device really ran in that mode, after this rule had been applied
//! to the open (`Recorded`). Nothing else counts: not a profile's volume, not a volume carried over
//! from another device, not one set while Direct output had fallen back to the mixer.
//! - Direct output, with a level: at most that level. Without one: at most `NEW_DIRECT_VOLUME`
//!   (and never more than what was playing).
//! - Through the mixer, with a level: at most that level. Without one: never louder than what was
//!   playing.
//! - An automatic move (the system default changed, the chosen device came back) waits for Play
//!   when it would play much louder than before, or on a device never played in that mode while the
//!   volume is above `NEW_DIRECT_VOLUME` (the device's own system volume is unknown: `waits`).
//! - A level is recorded only while that device is open in that mode (`opened`): a volume set
//!   while nothing is open is the volume wanted, held to the rule at the next open.
//! - The first raise (`unheard_ceiling`): on an open Direct output that isn't being heard (paused,
//!   at zero, or under the sleep timer's fade: what reaches the device is the volume times the
//!   fade), one volume command goes no higher than the volume last heard there (`NEW_DIRECT_VOLUME`
//!   for a device with none), or where it already is. While it plays at a level above zero with no
//!   fade any command is taken: the listener hears it rising. So Unmute from zero, a drag while
//!   paused and a drag under the fade can't jump a DAC to full scale (nor above what it last
//!   played); Unmute back to the level heard before muting, and lowering, always work. A volume
//!   held like that is recorded as the level only as far as this ceiling; past it, once heard.
//!
//! Every open is a new device generation (`Status::device_gen`) unless it replaces an open output
//! on the same device in the same real mode: a command, a drag or an Unmute level from before a
//! fallback to the mixer, a return to Direct output, a release or a device move is dropped.

use super::NEW_DIRECT_VOLUME;

/// A level that can be one: a damaged or hand-edited file (or a bad message) can hold anything,
/// and what isn't a volume between 0 and 1 counts as not recorded, so the cautious rule applies.
pub(super) fn level(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

/// The levels recorded on one device, read before anything is written for it (the move to it
/// writes its profile, and nothing written then may count).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Recorded {
    pub shared: Option<f32>,
    pub direct: Option<f32>,
}

impl Recorded {
    pub fn new(shared: Option<f32>, direct: Option<f32>) -> Self {
        Recorded { shared: level(shared), direct: level(direct) }
    }

    pub fn get(&self, direct: bool) -> Option<f32> {
        if direct { self.direct } else { self.shared }
    }
}

/// The most the volume may be on a device opened in this mode (`direct`: Direct output).
/// `before`: the volume in effect before playback came to it (or before this open).
pub(super) fn limit(rec: Recorded, direct: bool, before: f32) -> f32 {
    let before = if before.is_finite() { before.clamp(0.0, 1.0) } else { 0.0 };
    match rec.get(direct) {
        Some(l) => l,
        None if direct => before.min(NEW_DIRECT_VOLUME),
        None => before,
    }
}

/// The most one volume command may set on a Direct output that isn't being heard now: the volume
/// last heard there (`heard`; with none, or only silence, the start level), or where it is (`now`).
pub(super) fn unheard_ceiling(heard: f32, now: f32) -> f32 {
    let base = level(Some(heard)).filter(|h| *h > 0.0).unwrap_or(NEW_DIRECT_VOLUME);
    base.max(level(Some(now)).unwrap_or(0.0))
}

/// Why an automatic move waits for Play (None: it plays on).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Wait {
    /// The device's own level is more than twice what was playing.
    Louder,
    /// Never played in this mode: how loud its own system volume makes it isn't known.
    Unknown,
}

/// Whether an automatic move from `before` to `after` (already limited) waits for Play. `known`:
/// the device has a level in the mode it opens in.
///
/// A device never played through the mixer plays at the volume that was playing (never higher),
/// but its system volume may be far higher than the last device's. At or below
/// `NEW_DIRECT_VOLUME` that can't be loud even at full system volume (it's what Direct output
/// starts a new DAC at, with no system volume at all after it), so it plays on; above it, it waits
/// once (the level is recorded from then on).
pub(super) fn waits(known: bool, before: f32, after: f32) -> Option<Wait> {
    if after > before * 2.0 + 1e-6 {
        Some(Wait::Louder)
    } else if !known && after > NEW_DIRECT_VOLUME + 1e-6 {
        Some(Wait::Unknown)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_recorded_level_direct_output_starts_quietly_and_the_mixer_never_louder() {
        let none = Recorded::default();
        assert_eq!(limit(none, true, 1.0), NEW_DIRECT_VOLUME);
        assert_eq!(limit(none, true, 0.04), 0.04, "already quieter");
        assert_eq!(limit(none, false, 0.6), 0.6);
        assert_eq!(limit(none, true, f32::NAN), 0.0);
        let known = Recorded::new(Some(0.8), Some(0.5));
        assert_eq!((limit(known, true, 0.05), limit(known, false, 1.0)), (0.5, 0.8), "its own level");
    }

    #[test]
    fn what_isnt_a_volume_is_not_a_level() {
        let odd = Recorded::new(Some(-1.0), Some(5.0));
        assert_eq!(odd, Recorded::default());
        assert_eq!(Recorded::new(Some(f32::NAN), Some(f32::INFINITY)), Recorded::default());
        assert_eq!(limit(Recorded::new(None, Some(1.5)), true, 1.0), NEW_DIRECT_VOLUME);
    }

    #[test]
    fn a_raise_not_heard_goes_no_higher_than_the_start_level_or_what_was_heard() {
        assert_eq!(unheard_ceiling(0.0, 0.0), NEW_DIRECT_VOLUME, "from zero, nothing heard");
        assert_eq!(unheard_ceiling(0.5, 0.0), 0.5, "Unmute back to what was heard");
        assert_eq!(unheard_ceiling(0.05, 0.0), 0.05, "never above what it last played");
        assert_eq!(unheard_ceiling(0.2, 0.3), 0.3, "where it is");
        assert_eq!(unheard_ceiling(f32::NAN, 2.0), NEW_DIRECT_VOLUME, "not a volume: the start level");
    }

    #[test]
    fn an_automatic_move_waits_when_much_louder_or_unknown_and_loud() {
        assert_eq!(waits(true, 0.05, 0.5), Some(Wait::Louder));
        assert_eq!(waits(true, 0.5, 0.9), None, "known, not twice as loud");
        assert_eq!(waits(false, 0.5, 0.5), Some(Wait::Unknown), "never played there, and not quiet");
        assert_eq!(waits(false, 0.09, 0.09), None, "quiet enough anywhere");
        assert_eq!(waits(false, 0.05, 0.05), None);
    }
}
