//! 4x linear-phase oversampling for the nonlinear stages (Warmth's saturation, the clipper).
//! Saturation makes harmonics above Nyquist that fold back as inharmonic tones (aliasing); running
//! the nonlinearity at 4x the rate and filtering before coming back down removes them.
//!
//! Up: zero-stuff and low-pass (polyphase); down: the same low-pass, keeping every 4th sample. The
//! kernel has 4k+1 taps, so the round trip is a whole number of base-rate samples (`LATENCY`).

use std::f64::consts::PI;

pub const FACTOR: usize = 4;
pub const TAPS: usize = 257;
/// Round-trip delay in base-rate samples.
pub const LATENCY: usize = (TAPS - 1) / FACTOR;
/// Taps per polyphase branch (branch 0 has the most).
const PHASE_TAPS: usize = TAPS.div_ceil(FACTOR);

/// Zeroth-order modified Bessel function (power series; plenty for the Kaiser window).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Windowed-sinc low-pass at the base rate's Nyquist (1/8 of the 4x rate), Kaiser beta 8 (about
/// 80 dB stop band), normalised to unity DC gain. Flat to about 0.46 fs, stopped from about 0.54 fs.
fn kernel() -> [f64; TAPS] {
    let mut h = [0.0; TAPS];
    let mid = (TAPS - 1) as f64 / 2.0;
    let fc = 0.5 / FACTOR as f64; // cycles per 4x-rate sample
    let beta = 8.0;
    for (i, v) in h.iter_mut().enumerate() {
        let t = i as f64 - mid;
        let sinc = if t == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * t).sin() / (PI * t) };
        let r = t / mid;
        *v = sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
    }
    let sum: f64 = h.iter().sum();
    h.iter_mut().for_each(|v| *v /= sum);
    h
}

/// One channel of oversampling around a nonlinearity.
pub struct Oversampler {
    /// Down-sampling kernel (symmetric, so it can be applied to the history oldest-first).
    h: [f64; TAPS],
    /// Up-sampling branches, reversed so each is a dot product with the input history oldest-first.
    phases: [[f64; PHASE_TAPS]; FACTOR],
    /// Base-rate input history, written twice so a contiguous window always exists.
    input: [f64; 2 * PHASE_TAPS],
    in_pos: usize,
    /// 4x-rate processed history, same layout.
    high: Vec<f64>,
    high_pos: usize,
}

impl Default for Oversampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler {
    pub fn new() -> Self {
        let h = kernel();
        let mut phases = [[0.0; PHASE_TAPS]; FACTOR];
        for (k, branch) in phases.iter_mut().enumerate() {
            for j in 0..PHASE_TAPS {
                let i = k + FACTOR * j;
                // Zero-stuffing loses 3/4 of the energy: the up kernel carries a gain of FACTOR.
                let tap = if i < TAPS { h[i] * FACTOR as f64 } else { 0.0 };
                branch[PHASE_TAPS - 1 - j] = tap;
            }
        }
        Self { h, phases, input: [0.0; 2 * PHASE_TAPS], in_pos: 0, high: vec![0.0; 2 * TAPS], high_pos: 0 }
    }

    /// Feeds one base-rate sample through `f` at 4x and returns the result `LATENCY` samples late.
    #[inline]
    pub fn process(&mut self, x: f64, mut f: impl FnMut(f64) -> f64) -> f64 {
        self.input[self.in_pos] = x;
        self.input[self.in_pos + PHASE_TAPS] = x;
        self.in_pos = (self.in_pos + 1) % PHASE_TAPS;
        let mut out = 0.0;
        for k in 0..FACTOR {
            let window = &self.input[self.in_pos..self.in_pos + PHASE_TAPS];
            let s: f64 = self.phases[k].iter().zip(window).map(|(a, b)| a * b).sum();
            let v = f(s);
            self.high[self.high_pos] = v;
            self.high[self.high_pos + TAPS] = v;
            self.high_pos = (self.high_pos + 1) % TAPS;
            if k == 0 {
                // Only every 4th output is kept, and only that one is computed.
                let window = &self.high[self.high_pos..self.high_pos + TAPS];
                out = self.h.iter().zip(window).map(|(a, b)| a * b).sum();
            }
        }
        out
    }
}

/// A plain delay line (the dry path next to a delayed wet path).
pub struct Delay {
    buf: Vec<f64>,
    pos: usize,
}

impl Delay {
    pub fn new(samples: usize) -> Self {
        Self { buf: vec![0.0; samples], pos: 0 }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        if self.buf.is_empty() {
            return x;
        }
        let y = self.buf[self.pos];
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}
