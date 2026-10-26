//! The USB Audio Class 2 knowledge behind Android's USB exclusive mode (`usb_dac`), kept apart from
//! the USB plumbing so it can be tested anywhere: finding the DAC's playback setting in its
//! descriptors, choosing a rate it has, and sizing packets to its clock.

use anyhow::{Result, anyhow, bail};

/// The parts of the DAC Cadence plays through.
#[derive(Clone, Debug)]
pub struct Dac {
    /// AudioControl interface (clock requests go to it) and the clock that runs playback.
    pub control: u8,
    pub clock: u8,
    /// AudioStreaming interface and its stereo PCM setting with the most bits.
    pub stream: u8,
    pub alt: u8,
    /// Bytes per sample on the wire, and how many of the bits carry audio.
    pub subslot: usize,
    pub bits: usize,
    pub endpoint: u8,
    /// Most bytes one packet may carry.
    pub max_packet: usize,
    /// Packets go every 2^`interval_log` (micro)frames.
    pub interval_log: u32,
    /// The endpoint where an asynchronous DAC says how fast it takes samples, and its packet size.
    pub feedback: Option<(u8, usize)>,
    /// High speed (packets per 125 µs microframe) rather than full speed (per 1 ms frame).
    pub high_speed: bool,
}

#[derive(Default, Debug)]
struct Alt {
    interface: u8,
    alt: u8,
    terminal: u8,
    format_type: u8,
    formats: u32,
    channels: u8,
    subslot: u8,
    bits: u8,
    out: Option<(u8, u16, u8)>,
    feedback: Option<(u8, u16)>,
}

/// An entity on the way to the clock source.
pub enum Clock {
    Source,
    /// Picks one of these clocks.
    Selector(Vec<u8>),
    /// Runs off this clock.
    Multiplier(u8),
}

/// What the descriptors say: the DAC's playback setting (with the clock its terminal names) and the
/// clock entities, by ID, to follow from there.
pub struct Parsed {
    pub dac: Dac,
    pub clocks: Vec<(u8, Clock)>,
}

fn le16(d: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([d[i], d[i + 1]])
}

/// Finds a USB Audio 2.0 stereo playback setting in the device's descriptors (as usbfs reads them:
/// the device descriptor, then its configuration with everything in it).
pub fn parse(desc: &[u8], high_speed: bool) -> Result<Parsed> {
    let mut iface = (0u8, 0u8, 0u8, 0u8); // number, alternate setting, class, subclass
    let mut control = None;
    let mut uac2 = false;
    let mut terminals: Vec<(u8, u8)> = Vec::new(); // input terminal ID → its clock
    let mut clocks: Vec<(u8, Clock)> = Vec::new();
    let mut alts: Vec<Alt> = Vec::new();
    let mut i = 0;
    while i + 2 <= desc.len() {
        let len = desc[i] as usize;
        if len < 2 || i + len > desc.len() {
            break;
        }
        let d = &desc[i..i + len];
        let audio_control = iface.2 == 1 && iface.3 == 1;
        let streaming = iface.2 == 1 && iface.3 == 2 && iface.1 > 0;
        match d[1] {
            // Interface
            4 if len >= 9 => {
                iface = (d[2], d[3], d[5], d[6]);
                if d[5] == 1 && d[6] == 1 {
                    control.get_or_insert(d[2]);
                    uac2 |= d[7] == 0x20;
                } else if d[5] == 1 && d[6] == 2 && d[3] > 0 {
                    alts.push(Alt { interface: d[2], alt: d[3], ..Default::default() });
                }
            }
            // Class-specific AudioControl: the header, terminals and clocks
            0x24 if audio_control && len >= 4 => match d[2] {
                0x01 if len >= 5 => uac2 |= le16(d, 3) >= 0x0200,
                0x02 if len >= 8 => terminals.push((d[3], d[7])),
                0x0A => clocks.push((d[3], Clock::Source)),
                0x0B if len >= 5 + d[4] as usize => clocks.push((d[3], Clock::Selector(d[5..5 + d[4] as usize].to_vec()))),
                0x0C if len >= 5 => clocks.push((d[3], Clock::Multiplier(d[4]))),
                _ => {}
            },
            // Class-specific AudioStreaming: the format of this alternate setting
            0x24 if streaming => {
                if let Some(a) = alts.last_mut() {
                    match d[2] {
                        0x01 if len >= 11 => {
                            a.terminal = d[3];
                            a.format_type = d[5];
                            a.formats = u32::from_le_bytes([d[6], d[7], d[8], d[9]]);
                            a.channels = d[10];
                        }
                        0x02 if len >= 6 && d[3] == 1 => {
                            a.subslot = d[4];
                            a.bits = d[5];
                        }
                        _ => {}
                    }
                }
            }
            // Endpoint: isochronous OUT for the music, isochronous IN marked "feedback"
            5 if streaming && len >= 7 => {
                if let Some(a) = alts.last_mut()
                    && d[3] & 3 == 1
                {
                    if d[2] & 0x80 == 0 {
                        a.out = Some((d[2], le16(d, 4), d[6]));
                    } else if (d[3] >> 4) & 3 == 1 {
                        a.feedback = Some((d[2], le16(d, 4)));
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    let control = control.ok_or_else(|| anyhow!("it isn't a USB audio device"))?;
    if !uac2 {
        bail!("it uses USB Audio 1.0, which Cadence's own driver doesn't handle yet");
    }
    let alt = alts
        .iter()
        .filter(|a| {
            a.out.is_some()
                && a.channels == 2
                && a.format_type == 1
                && a.formats & 1 != 0
                && matches!(a.subslot, 2..=4)
                && a.bits >= 16
                && a.bits as usize <= a.subslot as usize * 8
        })
        .max_by_key(|a| (a.bits, a.subslot))
        .ok_or_else(|| anyhow!("it has no stereo PCM playback Cadence can use"))?;
    let clock = terminals
        .iter()
        .find(|(id, _)| *id == alt.terminal)
        .map(|(_, clock)| *clock)
        .ok_or_else(|| anyhow!("its playback has no clock"))?;
    let (endpoint, max_packet, interval) = alt.out.unwrap();
    // High-bandwidth endpoints carry up to 3 transactions per microframe.
    let max_packet = (max_packet & 0x7FF) as usize * (1 + ((max_packet >> 11) & 3) as usize);
    Ok(Parsed {
        dac: Dac {
            control,
            clock,
            stream: alt.interface,
            alt: alt.alt,
            subslot: alt.subslot as usize,
            bits: alt.bits as usize,
            endpoint,
            max_packet,
            interval_log: interval.clamp(1, 16) as u32 - 1,
            feedback: alt.feedback.map(|(ep, size)| (ep, (size & 0x7FF) as usize)),
            high_speed,
        },
        clocks,
    })
}

/// `rate` if the DAC has it (per the (min, max, step) ranges its clock gives), else the closest it has
/// from the same family (44.1 or 48 kHz multiples), preferring the highest at or below `rate`.
pub fn pick_rate(ranges: &[(u32, u32, u32)], rate: u32) -> u32 {
    let has = |r: u32| ranges.iter().any(|&(min, max, step)| min <= r && r <= max && (step == 0 || (r - min) % step == 0));
    if ranges.is_empty() || has(rate) {
        return rate;
    }
    let family = if rate % 11_025 == 0 { 44_100 } else { 48_000 };
    let candidates: Vec<u32> = [1, 2, 4, 8, 16].iter().map(|m| family * m).filter(|&r| has(r)).collect();
    candidates
        .iter()
        .copied()
        .filter(|&r| r <= rate)
        .max()
        .or_else(|| candidates.iter().copied().min())
        .or_else(|| ranges.iter().map(|&(_, max, _)| max).max())
        .unwrap_or(rate)
}

/// How many frames go in each packet, following the DAC's own clock when it reports it.
pub struct Pacing {
    /// Frames per (micro)frame in 16.16: the nominal rate, and the DAC's as it reports it.
    nominal: u64,
    freq: u64,
    /// The fraction of a frame carried over to the next packet.
    phase: u64,
    interval_log: u32,
    /// How far the DAC's feedback value is shifted from 16.16 (devices differ; found from the first).
    shift: Option<i32>,
}

impl Pacing {
    pub fn new(rate: u32, high_speed: bool, interval_log: u32) -> Self {
        let rate = rate as u64;
        // Linux's arithmetic (sound/usb/endpoint.c): frames per microframe or frame, in 16.16.
        let nominal = if high_speed {
            ((rate << 10) + 62) / 125
        } else {
            ((rate << 13) + 62) / 125
        };
        Self {
            nominal,
            freq: nominal,
            phase: 0,
            interval_log,
            shift: None,
        }
    }

    /// Frames in the next packet (at most `max`).
    pub fn next_frames(&mut self, max: usize) -> usize {
        self.phase = (self.phase & 0xFFFF) + (self.freq << self.interval_log);
        ((self.phase >> 16) as usize).min(max)
    }

    /// Takes a feedback value: how many frames per (micro)frame the DAC's clock really takes.
    pub fn feedback(&mut self, raw: &[u8]) {
        let f = match raw.len() {
            3 => u32::from_le_bytes([raw[0], raw[1], raw[2], 0]) as u64,
            n if n >= 4 => u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as u64,
            _ => return,
        };
        if f == 0 {
            return;
        }
        let shifted = match self.shift {
            Some(s) if s >= 0 => f << s,
            Some(s) => f >> -s,
            None => {
                // As Linux does: shift the first value until it looks like the nominal rate.
                let (low, high) = (self.nominal - self.nominal / 4, self.nominal + self.nominal / 2);
                let (mut g, mut s) = (f, 0);
                while g < low && s < 8 {
                    g <<= 1;
                    s += 1;
                }
                while g > high && s > -8 {
                    g >>= 1;
                    s -= 1;
                }
                self.shift = Some(s);
                g
            }
        };
        if shifted >= self.nominal - self.nominal / 8 && shifted <= self.nominal + self.nominal / 8 {
            self.freq = shifted;
        } else {
            // Out of range: work the format out again from the next value.
            self.shift = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The "TTGK USB HiFi Audio" dongle, cut down: a HID interface, then the playback part (high
    /// speed, asynchronous with a feedback endpoint).
    fn descriptors() -> Vec<u8> {
        let mut d = vec![18, 1, 0, 2, 0xEF, 2, 1, 64, 0x2A, 0x26, 1, 0, 8, 0, 1, 2, 0, 1];
        d.extend([9, 2, 0, 0, 3, 1, 0, 0xA0, 50]);
        d.extend([9, 4, 0, 0, 1, 3, 0, 0, 0]);
        d.extend([7, 5, 0x81, 3, 1, 0, 6]);
        // AudioControl, with a clock source (5) and a USB streaming input terminal (2) on it
        d.extend([9, 4, 1, 0, 0, 1, 1, 0x20, 0]);
        d.extend([9, 0x24, 1, 0, 2, 8, 0, 0, 0]);
        d.extend([8, 0x24, 0x0A, 5, 3, 7, 0, 0]);
        d.extend([17, 0x24, 2, 2, 1, 1, 0, 5, 2, 3, 0, 0, 0, 0, 0, 0, 0]);
        // AudioStreaming: alt 0 (idle), alt 1 (16-bit), alt 2 (32-bit)
        d.extend([9, 4, 2, 0, 0, 1, 2, 0x20, 0]);
        for (alt, subslot, bits, size) in [(1u8, 2u8, 16u8, 200u16), (2, 4, 32, 400)] {
            d.extend([9, 4, 2, alt, 2, 1, 2, 0x20, 0]);
            d.extend([16, 0x24, 1, 2, 0, 1, 1, 0, 0, 0, 2, 3, 0, 0, 0, 0]);
            d.extend([6, 0x24, 2, 1, subslot, bits]);
            d.extend([7, 5, 0x03, 0x05, size as u8, (size >> 8) as u8, 1]);
            d.extend([8, 0x25, 1, 0, 0, 0, 0, 0]);
            d.extend([7, 5, 0x84, 0x11, 4, 0, 4]);
        }
        d
    }

    #[test]
    fn finds_the_most_precise_stereo_setting() {
        let d = parse(&descriptors(), true).unwrap().dac;
        assert_eq!((d.control, d.clock, d.stream, d.alt), (1, 5, 2, 2));
        assert_eq!((d.subslot, d.bits, d.endpoint, d.max_packet), (4, 32, 0x03, 400));
        assert_eq!(d.feedback, Some((0x84, 4)));
        assert_eq!(d.interval_log, 0);
    }

    #[test]
    fn refuses_usb_audio_1() {
        let mut d = descriptors();
        // AudioControl protocol 0 and bcdADC 1.00
        let ac = d.windows(8).position(|w| w == [9, 4, 1, 0, 0, 1, 1, 0x20]).unwrap();
        d[ac + 7] = 0;
        let header = d.windows(5).position(|w| w == [9, 0x24, 1, 0, 2]).unwrap();
        d[header + 4] = 1;
        assert!(parse(&d, true).is_err());
    }

    #[test]
    fn picks_a_rate_the_dac_has() {
        let dac = [(44_100, 44_100, 0), (48_000, 48_000, 0), (88_200, 96_000, 7_800), (176_400, 192_000, 15_600)];
        assert_eq!(pick_rate(&dac, 44_100), 44_100);
        assert_eq!(pick_rate(&dac, 96_000), 96_000);
        assert_eq!(pick_rate(&dac, 352_800), 176_400);
        assert_eq!(pick_rate(&dac, 384_000), 192_000);
        assert_eq!(pick_rate(&[], 22_050), 22_050);
        // A continuous range takes anything in it; below it, the family's lowest it has.
        assert_eq!(pick_rate(&[(8_000, 192_000, 0)], 32_000), 32_000);
        assert_eq!(pick_rate(&[(44_100, 192_000, 0)], 32_000), 48_000);
    }

    #[test]
    fn packets_add_up_to_the_rate() {
        // 44.1 kHz over 8000 microframes a second: 5 or 6 frames each, 44 100 in all.
        let mut p = Pacing::new(44_100, true, 0);
        let mut total = 0;
        for _ in 0..8000 {
            let n = p.next_frames(50);
            assert!((5..=6).contains(&n));
            total += n;
        }
        assert!((44_099..=44_101).contains(&total), "{total}");
    }

    #[test]
    fn follows_the_dacs_clock() {
        // A DAC running 0.1% fast at 48 kHz reports 6.006 frames per microframe (16.16).
        let mut p = Pacing::new(48_000, true, 0);
        let fast = (6.006 * 65_536.0) as u32;
        p.feedback(&fast.to_le_bytes());
        let total: usize = (0..8000).map(|_| p.next_frames(50)).sum();
        assert!((48_040..=48_056).contains(&total), "{total}");
        // The same in full speed's 10.14 format (3 bytes, frames per 1 ms frame) is found too.
        let mut p = Pacing::new(48_000, false, 0);
        let fs = (48.048 * 16_384.0) as u32;
        p.feedback(&fs.to_le_bytes()[..3]);
        let total: usize = (0..1000).map(|_| p.next_frames(200)).sum();
        assert!((48_040..=48_056).contains(&total), "{total}");
        // Nonsense is ignored.
        let mut p = Pacing::new(48_000, true, 0);
        p.feedback(&[0, 0, 0, 0]);
        assert_eq!((0..8000).map(|_| p.next_frames(50)).sum::<usize>(), 48_000);
    }
}
