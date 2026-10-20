//! Sing along: turns the lead vocal down.
//!
//! Lead vocals almost always sit in the centre of a stereo mix. The centre (mid = (L+R)/2) is
//! reduced only in the voice range (about 150 Hz – 7 kHz), so bass, kick drum and cymbal air stay,
//! and everything panned to the sides (side = (L−R)/2) is untouched. Mono songs are left alone.

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// Butterworth (Q = 0.707) high- or low-pass, RBJ cookbook.
    fn new(freq: f64, rate: f64, high: bool) -> Self {
        let w = 2.0 * std::f64::consts::PI * freq.min(rate * 0.45) / rate;
        let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let cos = w.cos();
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = if high {
            ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0)
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0)
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            ..Default::default()
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

pub struct VocalCut {
    /// 0 = vocals as they are, 1 = as far down as this method goes.
    amount: f32,
    rate: u32,
    hp: Biquad,
    lp: Biquad,
}

impl VocalCut {
    pub fn new(amount: f32) -> Self {
        let mut v = Self {
            amount: 0.0,
            rate: 0,
            hp: Biquad::default(),
            lp: Biquad::default(),
        };
        v.set_amount(amount);
        v
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = amount.clamp(0.0, 1.0);
    }

    pub fn active(&self) -> bool {
        self.amount > 0.0
    }

    pub fn reset(&mut self) {
        self.rate = 0;
    }

    /// Processes interleaved audio in place (stereo only).
    pub fn process(&mut self, samples: &mut [f32], channels: usize, rate: u32) {
        if !self.active() || channels != 2 {
            return;
        }
        if rate != self.rate {
            self.rate = rate;
            self.hp = Biquad::new(150.0, rate as f64, true);
            self.lp = Biquad::new(7_000.0, rate as f64, false);
        }
        let k = self.amount as f64;
        for f in samples.chunks_exact_mut(2) {
            let (l, r) = (f[0] as f64, f[1] as f64);
            let mid = 0.5 * (l + r);
            let side = 0.5 * (l - r);
            let voice = self.lp.run(self.hp.run(mid));
            let mid = mid - k * voice;
            f[0] = (mid + side) as f32;
            f[1] = (mid - side) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_voice_drops_sides_stay() {
        let rate = 48_000;
        let tone = |i: usize| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate as f32).sin() * 0.5;
        // Centre 1 kHz: should be strongly reduced at full amount.
        let mut centre: Vec<f32> = (0..9600).flat_map(|i| [tone(i), tone(i)]).collect();
        // Hard-left 1 kHz: half of it is "side", which stays.
        let mut left: Vec<f32> = (0..9600).flat_map(|i| [tone(i), 0.0]).collect();
        let rms = |v: &[f32]| (v[4800..].iter().map(|x| x * x).sum::<f32>() / (v.len() - 4800) as f32).sqrt();
        let (c0, l0) = (rms(&centre), rms(&left));
        VocalCut::new(1.0).process(&mut centre, 2, rate);
        VocalCut::new(1.0).process(&mut left, 2, rate);
        assert!(rms(&centre) < c0 * 0.3, "centre {} -> {}", c0, rms(&centre));
        assert!(rms(&left) > l0 * 0.5, "left {} -> {}", l0, rms(&left));
    }
}
