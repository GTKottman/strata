//! ITU-R BS.1770-4 / EBU R128 loudness (momentary, short-term, integrated) and true peak.

use super::filter::{Biquad, Coefs};

/// BS.1770-4 Annex 2 true-peak interpolation filter: 4 phases x 12 taps.
const TP_TAPS: [[f64; 12]; 4] = [
    [
        0.0017089843750, 0.0109863281250, -0.0196533203125, 0.0332031250000, -0.0594482421875, 0.1373291015625,
        0.9721679687500, -0.1022949218750, 0.0476074218750, -0.0266113281250, 0.0148925781250, -0.0083007812500,
    ],
    [
        -0.0291748046875, 0.0292968750000, -0.0517578125000, 0.0891113281250, -0.1665039062500, 0.4650878906250,
        0.7797851562500, -0.2003173828125, 0.1015625000000, -0.0582275390625, 0.0330810546875, -0.0189208984375,
    ],
    [
        -0.0189208984375, 0.0330810546875, -0.0582275390625, 0.1015625000000, -0.2003173828125, 0.7797851562500,
        0.4650878906250, -0.1665039062500, 0.0891113281250, -0.0517578125000, 0.0292968750000, -0.0291748046875,
    ],
    [
        -0.0083007812500, 0.0148925781250, -0.0266113281250, 0.0476074218750, -0.1022949218750, 0.9721679687500,
        0.1373291015625, -0.0594482421875, 0.0332031250000, -0.0196533203125, 0.0109863281250, 0.0017089843750,
    ],
];

/// 4x oversampled peak estimator. `push` returns the largest absolute value among the four
/// interpolated points around the newest sample (delayed by ~6 samples).
#[derive(Clone, Copy, Debug, Default)]
pub struct TruePeak {
    hist: [f64; 12],
    pos: usize,
}

impl TruePeak {
    #[inline]
    pub fn push(&mut self, x: f64) -> f64 {
        self.pos = (self.pos + 11) % 12;
        self.hist[self.pos] = x;
        let mut peak = 0.0f64;
        for phase in &TP_TAPS {
            let mut acc = 0.0;
            for (k, h) in phase.iter().enumerate() {
                acc += h * self.hist[(self.pos + k) % 12];
            }
            peak = peak.max(acc.abs());
        }
        peak
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

const HIST_MIN: f64 = -70.0;
const HIST_BINS: usize = 900; // -70 .. +20 LUFS in 0.1 LU bins
const SHORT_STEPS: usize = 30; // 3 s of 100 ms steps

pub fn lufs(energy: f64) -> f64 {
    if energy <= 0.0 {
        f64::NEG_INFINITY
    } else {
        -0.691 + 10.0 * energy.log10()
    }
}

/// Stereo loudness meter. All memory is allocated in `new`.
pub struct LoudnessMeter {
    k: [[Biquad; 2]; 2],
    step_len: usize,
    step_pos: usize,
    step_acc: f64,
    steps: [f64; SHORT_STEPS],
    step_idx: usize,
    steps_filled: usize,
    hist_count: Vec<u64>,
    hist_energy: Vec<f64>,
    tp: [TruePeak; 2],
    pub momentary: f64,
    pub short_term: f64,
    pub momentary_max: f64,
    /// Linear, maximum since reset.
    pub true_peak_max: f64,
    pub sample_peak_max: f64,
}

impl LoudnessMeter {
    pub fn new(fs: f64) -> Self {
        // K-weighting as RBJ designs that reproduce the BS.1770 48 kHz coefficients at any rate.
        let stage1 = Coefs::high_shelf(fs, 1500.0, std::f64::consts::FRAC_1_SQRT_2, 4.0);
        let stage2 = Coefs::k_highpass(fs, 38.135, 0.5003);
        Self {
            k: [[Biquad::new(stage1), Biquad::new(stage2)]; 2],
            step_len: ((fs * 0.1).round() as usize).max(1),
            step_pos: 0,
            step_acc: 0.0,
            steps: [0.0; SHORT_STEPS],
            step_idx: 0,
            steps_filled: 0,
            hist_count: vec![0; HIST_BINS],
            hist_energy: vec![0.0; HIST_BINS],
            tp: [TruePeak::default(); 2],
            momentary: f64::NEG_INFINITY,
            short_term: f64::NEG_INFINITY,
            momentary_max: f64::NEG_INFINITY,
            true_peak_max: 0.0,
            sample_peak_max: 0.0,
        }
    }

    pub fn reset(&mut self) {
        for ch in &mut self.k {
            for f in ch {
                f.reset();
            }
        }
        self.step_pos = 0;
        self.step_acc = 0.0;
        self.steps = [0.0; SHORT_STEPS];
        self.step_idx = 0;
        self.steps_filled = 0;
        self.hist_count.iter_mut().for_each(|c| *c = 0);
        self.hist_energy.iter_mut().for_each(|e| *e = 0.0);
        self.tp = [TruePeak::default(); 2];
        self.momentary = f64::NEG_INFINITY;
        self.short_term = f64::NEG_INFINITY;
        self.momentary_max = f64::NEG_INFINITY;
        self.true_peak_max = 0.0;
        self.sample_peak_max = 0.0;
    }

    #[inline]
    pub fn process(&mut self, l: f64, r: f64) {
        let tp = self.tp[0].push(l).max(self.tp[1].push(r));
        self.true_peak_max = self.true_peak_max.max(tp).max(l.abs()).max(r.abs());
        self.sample_peak_max = self.sample_peak_max.max(l.abs()).max(r.abs());

        let mut e = 0.0;
        for (ch, x) in [l, r].into_iter().enumerate() {
            let [s1, s2] = &mut self.k[ch];
            let y = s2.process(s1.process(x));
            e += y * y;
        }
        self.step_acc += e;
        self.step_pos += 1;
        if self.step_pos == self.step_len {
            self.finish_step();
        }
    }

    fn finish_step(&mut self) {
        self.steps[self.step_idx] = self.step_acc / self.step_len as f64;
        self.step_idx = (self.step_idx + 1) % SHORT_STEPS;
        self.steps_filled = (self.steps_filled + 1).min(SHORT_STEPS);
        self.step_acc = 0.0;
        self.step_pos = 0;

        let recent = |n: usize| -> f64 {
            let n = n.min(self.steps_filled);
            let mut sum = 0.0;
            for i in 1..=n {
                sum += self.steps[(self.step_idx + SHORT_STEPS - i) % SHORT_STEPS];
            }
            sum / n as f64
        };
        if self.steps_filled >= 4 {
            let block = recent(4);
            self.momentary = lufs(block);
            self.momentary_max = self.momentary_max.max(self.momentary);
            // Gating block (400 ms, 75 % overlap) into the histogram, absolute gate at -70 LUFS.
            if self.momentary > HIST_MIN {
                let bin = (((self.momentary - HIST_MIN) * 10.0) as usize).min(HIST_BINS - 1);
                self.hist_count[bin] += 1;
                self.hist_energy[bin] += block;
            }
        }
        if self.steps_filled >= SHORT_STEPS {
            self.short_term = lufs(recent(SHORT_STEPS));
        }
    }

    /// Gated integrated loudness since reset (relative gate -10 LU).
    pub fn integrated(&self) -> f64 {
        let (n, e) = self.sum_from(0);
        if n == 0 {
            return f64::NEG_INFINITY;
        }
        let rel_gate = lufs(e / n as f64) - 10.0;
        let from = (((rel_gate - HIST_MIN) * 10.0).ceil().max(0.0) as usize).min(HIST_BINS);
        let (n2, e2) = self.sum_from(from);
        if n2 == 0 {
            f64::NEG_INFINITY
        } else {
            lufs(e2 / n2 as f64)
        }
    }

    fn sum_from(&self, bin: usize) -> (u64, f64) {
        let n = self.hist_count[bin..].iter().sum();
        let e = self.hist_energy[bin..].iter().sum();
        (n, e)
    }
}
