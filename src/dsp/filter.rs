//! RBJ biquads (transposed direct form II, f64 state).

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Default for Coefs {
    fn default() -> Self {
        Self::identity()
    }
}

impl Coefs {
    pub const fn identity() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }

    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    pub fn highpass(fs: f64, f: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * (f / fs).min(0.49);
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Self::norm((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// BS.1770 K-weighting stage 2: high-pass with the spec's unnormalised [1, -2, 1] numerator.
    pub fn k_highpass(fs: f64, f: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * f / fs;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Self { b0: 1.0, b1: -2.0, b2: 1.0, a1: -2.0 * c / (1.0 + alpha), a2: (1.0 - alpha) / (1.0 + alpha) }
    }

    pub fn peak(fs: f64, f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * (f / fs).min(0.49);
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Self::norm(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    pub fn low_shelf(fs: f64, f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * (f / fs).min(0.49);
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }

    pub fn high_shelf(fs: f64, f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * (f / fs).min(0.49);
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    pub c: Coefs,
    z1: f64,
    z2: f64,
}

impl Biquad {
    pub fn new(c: Coefs) -> Self {
        Self { c, z1: 0.0, z2: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}
