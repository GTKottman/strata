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

    pub fn lowpass(fs: f64, f: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * (f / fs).min(0.49);
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Self::norm((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
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

impl Coefs {
    /// Peaking EQ with the analog response matched up to Nyquist (no high-frequency cramping).
    pub fn matched_peak(fs: f64, f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f;
        Self::matched(fs, [1.0, a * w / q, w * w], [w / (a * q), w * w], f)
    }

    /// High shelf with the analog response matched up to Nyquist (no high-frequency cramping).
    pub fn matched_high_shelf(fs: f64, f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f;
        let sa = a.sqrt();
        Self::matched(fs, [a * a, a * sa * w / q, a * w * w], [sa * w / q, a * w * w], f)
    }

    /// Biquad from the analog prototype (n2 s^2 + n1 s + n0) / (s^2 + d1 s + d0), s in rad/s.
    /// The bilinear transform squeezes the whole analog axis into 0..Nyquist, so boosts near the top
    /// bend ("cramping"). Here the poles come from impulse invariance (exact pole positions) and the
    /// zeros are solved so the magnitude equals the analog one at DC, at Nyquist and at `f_match`.
    /// Uses |H(w)|^2 = (B0 p0 + B1 p1 + B2 p2) / (A0 p0 + A1 p1 + A2 p2) with p1 = sin^2(w/2),
    /// p0 = 1 - p1, p2 = 4 p0 p1, and B0 = (b0+b1+b2)^2, B1 = (b0-b1+b2)^2, B2 = -4 b0 b2 (same for A).
    fn matched(fs: f64, n: [f64; 3], d: [f64; 2], f_match: f64) -> Self {
        let t = 1.0 / fs;
        let wn = d[1].sqrt();
        let zeta = d[0] / (2.0 * wn);
        let r = (-zeta * wn * t).exp();
        let a1 = if zeta <= 1.0 {
            -2.0 * r * (wn * t * (1.0 - zeta * zeta).sqrt()).cos()
        } else {
            -2.0 * r * (wn * t * (zeta * zeta - 1.0).sqrt()).cosh()
        };
        let a2 = r * r;
        let analog = |w: f64| {
            let num = (n[2] - n[0] * w * w).powi(2) + (n[1] * w).powi(2);
            let den = (d[1] - w * w).powi(2) + (d[0] * w).powi(2);
            num / den
        };
        let big_a = [(1.0 + a1 + a2).powi(2), (1.0 - a1 + a2).powi(2), -4.0 * a2];
        let phis = |f: f64| {
            let p1 = (PI * f / fs).sin().powi(2);
            let p0 = 1.0 - p1;
            [p0, p1, 4.0 * p0 * p1]
        };
        let b0_ = analog(0.0) * big_a[0];
        let b1_ = analog(PI * fs) * big_a[1];
        let fm = f_match.min(0.45 * fs);
        let p = phis(fm);
        let den_m = big_a[0] * p[0] + big_a[1] * p[1] + big_a[2] * p[2];
        let b2_ = (analog(2.0 * PI * fm) * den_m - b0_ * p[0] - b1_ * p[1]) / p[2];
        let (s0, s1) = (b0_.sqrt(), b1_.sqrt());
        let w = 0.5 * (s0 + s1);
        let b0 = 0.5 * (w + (w * w + b2_).max(0.0).sqrt());
        let b1 = 0.5 * (s0 - s1);
        let b2 = -b2_ / (4.0 * b0);
        Self { b0, b1, b2, a1, a2 }
    }

    /// Magnitude response in dB at `f` (for tests and displays).
    pub fn magnitude_db(&self, fs: f64, f: f64) -> f64 {
        let w = 2.0 * PI * f / fs;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
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
