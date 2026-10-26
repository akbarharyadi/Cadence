//! Float samples to the integer formats DACs take, without changing a bit of the music (used by the
//! direct outputs: WASAPI exclusive on Windows, bit-perfect USB on Android).

#[derive(Clone, Copy)]
pub struct Format {
    /// Bits per sample in the stream (16, 24 or 32).
    pub store_bits: usize,
    /// Bits that carry audio (24 in a 32-bit container: the low byte is zero).
    pub valid_bits: usize,
}

/// Triangular (TPDF) dither, only for 16-bit output.
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

/// Converts float samples to the device's little-endian integer format.
///
/// Symphonia decodes N-bit audio as `value / 2^(N-1)`, so multiplying back gives the exact original
/// integers: a 24-bit file at full volume comes out bit-for-bit identical. Dither is added only to
/// 16-bit output and only when a value isn't already a whole 16-bit number (so 16-bit files stay exact).
pub fn encode(input: &[f32], out: &mut [u8], f: Format, dither: &mut Dither) {
    match (f.store_bits, f.valid_bits) {
        (32, 24) => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(4)) {
                let v = (*s as f64 * 8_388_608.0)
                    .round()
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
                let v = (*s as f64 * 8_388_608.0)
                    .round()
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;
                o.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        _ => {
            for (s, o) in input.iter().zip(out.chunks_exact_mut(2)) {
                let x = *s * 32_768.0;
                let x = if x.fract() != 0.0 {
                    x + dither.next()
                } else {
                    x
                };
                let v = x.round().clamp(-32_768.0, 32_767.0) as i16;
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
