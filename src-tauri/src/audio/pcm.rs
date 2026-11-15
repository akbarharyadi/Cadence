//! Float samples to the integer formats DACs take, without changing a bit of the music (used by the
//! direct outputs: WASAPI exclusive on Windows, bit-perfect USB on Android, and integer devices
//! through cpal).
//!
//! Dither is added only where a sample isn't already a whole step of the output format: music that
//! nothing changed (volume at full, no processing) comes out bit-for-bit, and music that was changed
//! (volume, EQ, Sound Check, resampling) is rounded with TPDF dither instead of plain truncation.

#![cfg_attr(not(any(windows, target_os = "android")), allow(dead_code))]

#[derive(Clone, Copy)]
pub struct Format {
    /// Bits per sample in the stream (16, 24 or 32).
    pub store_bits: usize,
    /// Bits that carry audio (24 in a 32-bit container: the low byte is zero).
    pub valid_bits: usize,
}

/// Triangular (TPDF) dither noise source.
#[derive(Default)]
pub struct Dither {
    state: u32,
}

impl Dither {
    /// A uniform random number in [0, 1) (xorshift: cheap and good enough for dither noise).
    fn uniform(&mut self) -> f32 {
        if self.state == 0 {
            self.state = 0x9E37_79B9;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state as f32 / u32::MAX as f32
    }

    /// Triangular noise in (-1, 1) LSB: the difference of two uniform values.
    fn next(&mut self) -> f32 {
        self.uniform() - self.uniform()
    }
}

/// `x` (in steps of the output format) rounded to a whole step: TPDF dither (±1 step) first when
/// it isn't one already.
fn round_dithered(x: f64, dither: &mut Dither) -> f64 {
    if x.fract() != 0.0 {
        (x + f64::from(dither.next())).round()
    } else {
        x
    }
}

/// Rounds float samples (in place) to whole steps of a `bits`-bit integer format, dithered where
/// needed and kept within range, for a device that converts floats by truncating (cpal).
pub fn requantize(samples: &mut [f32], bits: u32, dither: &mut Dither) {
    let scale = f64::from(1u32 << (bits.clamp(8, 24) - 1));
    for s in samples {
        let x = round_dithered(f64::from(*s) * scale, dither).clamp(-scale, scale - 1.0);
        *s = (x / scale) as f32;
    }
}

/// Converts float samples to the device's little-endian integer format.
///
/// Symphonia decodes N-bit audio as `value / 2^(N-1)`, so multiplying back gives the exact original
/// integers: a 24-bit file at full volume comes out bit-for-bit identical. Dither is added (16- and
/// 24-bit output) only when a value isn't already a whole number at the output's depth, so untouched
/// 16- and 24-bit music stays exact. 32-bit output holds every bit an f32 carries: no dither.
pub fn encode(input: &[f32], out: &mut [u8], f: Format, dither: &mut Dither) {
    match (f.store_bits, f.valid_bits) {
        (32, 24) => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(4)) {
                let v = round_dithered(*s as f64 * 8_388_608.0, dither)
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;
                o.copy_from_slice(&(v << 8).to_le_bytes());
            }
        }
        (32, _) => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(4)) {
                let v = (*s as f64 * 2_147_483_648.0)
                    .round()
                    .clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
                o.copy_from_slice(&v.to_le_bytes());
            }
        }
        (24, _) => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(3)) {
                let v = round_dithered(*s as f64 * 8_388_608.0, dither)
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;
                o.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        _ => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(2)) {
                let v = round_dithered(*s as f64 * 32_768.0, dither).clamp(-32_768.0, 32_767.0) as i16;
                o.copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_four_bit_is_exact() {
        // Values as Symphonia produces them from 24-bit integers.
        let ints = [0i32, 1, -1, 8_388_607, -8_388_608, 123_456, -654_321];
        let floats: Vec<f32> = ints.iter().map(|&i| i as f32 / 8_388_608.0).collect();
        let mut out = vec![0u8; floats.len() * 4];
        encode(
            &floats,
            &mut out,
            Format {
                store_bits: 32,
                valid_bits: 24,
            },
            &mut Dither::default(),
        );
        for (i, chunk) in ints.iter().zip(out.chunks_exact(4)) {
            assert_eq!(i32::from_le_bytes(chunk.try_into().unwrap()) >> 8, *i);
        }
    }

    #[test]
    fn sixteen_bit_source_gets_no_dither() {
        let ints = [0i16, 1, -1, 32_767, -32_768, 12_345];
        let floats: Vec<f32> = ints.iter().map(|&i| i as f32 / 32_768.0).collect();
        let mut out = vec![0u8; floats.len() * 2];
        encode(
            &floats,
            &mut out,
            Format {
                store_bits: 16,
                valid_bits: 16,
            },
            &mut Dither::default(),
        );
        for (i, chunk) in ints.iter().zip(out.chunks_exact(2)) {
            assert_eq!(i16::from_le_bytes(chunk.try_into().unwrap()), *i);
        }
    }

    #[test]
    fn changed_audio_is_dithered_at_24_bit() {
        // A 24-bit ramp at -6 dB volume: halves of odd values aren't whole 24-bit numbers.
        let floats: Vec<f32> = (0..20_000).map(|i| ((i % 2000) as f32 - 1000.0) * 0.5 / 8_388_608.0).collect();
        let f = Format { store_bits: 32, valid_bits: 24 };
        let mut out = vec![0u8; floats.len() * 4];
        encode(&floats, &mut out, f, &mut Dither::default());
        let mut changed = 0;
        let mut sum = 0.0f64;
        for (s, chunk) in floats.iter().zip(out.chunks_exact(4)) {
            let v = i32::from_le_bytes(chunk.try_into().unwrap()) >> 8;
            let exact = *s as f64 * 8_388_608.0;
            let err = v as f64 - exact;
            assert!(err.abs() <= 1.5, "{exact} became {v}");
            if exact.fract() == 0.0 {
                assert_eq!(v as f64, exact, "whole values are left alone");
            } else {
                changed += (v as f64 != exact.round()) as u32;
                sum += err;
            }
        }
        // Dithered: plain rounding would never differ from `round`, and the error averages out.
        assert!(changed > 1000, "{changed}");
        assert!((sum / 10_000.0).abs() < 0.05, "biased: {}", sum / 10_000.0);
    }

    #[test]
    fn requantize_keeps_whole_steps_and_dithers_the_rest() {
        let mut d = Dither::default();
        let ints = [0i32, 1, -1, 32_767, -32_768, 12_345];
        let mut x: Vec<f32> = ints.iter().map(|&i| i as f32 / 32_768.0).collect();
        let before = x.clone();
        requantize(&mut x, 16, &mut d);
        assert_eq!(x, before, "16-bit music through a 16-bit device: untouched");
        // 24-bit music to a 16-bit device: every value lands on a 16-bit step, near the original.
        let mut y: Vec<f32> = (0..1000).map(|i| (i * 977 % 8_000_000) as f32 / 8_388_608.0).collect();
        let orig = y.clone();
        requantize(&mut y, 16, &mut d);
        for (a, b) in y.iter().zip(orig.iter()) {
            let steps = *a as f64 * 32_768.0;
            assert_eq!(steps.fract(), 0.0);
            assert!((steps - *b as f64 * 32_768.0).abs() <= 1.5);
        }
        // Full scale stays in range (cpal's I24 would overflow on +1.0).
        let mut z = vec![1.0f32, -1.0];
        requantize(&mut z, 24, &mut d);
        assert_eq!(z, vec![8_388_607.0 / 8_388_608.0, -1.0]);
    }

    #[test]
    fn thirty_two_bit_keeps_every_bit_of_16_and_24_bit_music() {
        // Android's USB DACs are usually set to plain 32-bit: the music sits in the top bits.
        let ints = [0i32, 1, -1, 8_388_607, -8_388_608, 123_456];
        let floats: Vec<f32> = ints.iter().map(|&i| i as f32 / 8_388_608.0).collect();
        let mut out = vec![0u8; floats.len() * 4];
        let f = Format { store_bits: 32, valid_bits: 32 };
        encode(&floats, &mut out, f, &mut Dither::default());
        for (i, chunk) in ints.iter().zip(out.chunks_exact(4)) {
            assert_eq!(i32::from_le_bytes(chunk.try_into().unwrap()), *i << 8);
        }
        let shorts = [0i16, 1, -1, 32_767, -32_768];
        let floats: Vec<f32> = shorts.iter().map(|&i| i as f32 / 32_768.0).collect();
        let mut out = vec![0u8; floats.len() * 4];
        encode(&floats, &mut out, f, &mut Dither::default());
        for (i, chunk) in shorts.iter().zip(out.chunks_exact(4)) {
            assert_eq!(i32::from_le_bytes(chunk.try_into().unwrap()), (*i as i32) << 16);
        }
    }
}
