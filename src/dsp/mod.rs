//! The Strata mastering chain: four reorderable adjustment layers (Tone, Glue, Warmth, Width),
//! each with on/off and opacity (dry/wet), followed by a fixed true-peak limiter output layer.
//! Everything here is host independent so the offline renderer and the tests use the same code.

pub mod filter;
pub mod meter;

use filter::{Biquad, Coefs};
use meter::TruePeak;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LayerKind {
    Tone = 0,
    Glue = 1,
    Warmth = 2,
    Width = 3,
}

impl LayerKind {
    pub const ALL: [LayerKind; 4] = [LayerKind::Tone, LayerKind::Glue, LayerKind::Warmth, LayerKind::Width];

    pub fn name(self) -> &'static str {
        match self {
            LayerKind::Tone => "Tone",
            LayerKind::Glue => "Glue",
            LayerKind::Warmth => "Warmth",
            LayerKind::Width => "Width",
        }
    }
}

pub const DEFAULT_ORDER: [LayerKind; 4] = LayerKind::ALL;

/// Order packed into one integer (4 bits per slot) so it can live in an atomic.
pub fn pack_order(order: [LayerKind; 4]) -> u32 {
    order.iter().enumerate().fold(0, |acc, (i, k)| acc | ((*k as u32) << (4 * i)))
}

/// Unpacks an order; anything that is not a permutation of the four layers gives the default.
pub fn unpack_order(packed: u32) -> [LayerKind; 4] {
    let mut out = DEFAULT_ORDER;
    let mut seen = [false; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let v = ((packed >> (4 * i)) & 0xF) as usize;
        if v > 3 || seen[v] {
            return DEFAULT_ORDER;
        }
        seen[v] = true;
        *slot = LayerKind::ALL[v];
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneSettings {
    pub on: bool,
    pub opacity: f64,
    pub low_cut_hz: f64,
    pub low_db: f64,
    pub mid_db: f64,
    pub presence_db: f64,
    pub air_db: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlueSettings {
    pub on: bool,
    pub opacity: f64,
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub makeup_db: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarmthSettings {
    pub on: bool,
    pub opacity: f64,
    pub drive_db: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WidthSettings {
    pub on: bool,
    pub opacity: f64,
    /// Side gain: 0 = mono, 1 = unchanged, 2 = double.
    pub width: f64,
    /// Side content below this is removed (bass to mono). 20 Hz or less = off.
    pub mono_below_hz: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterSettings {
    pub on: bool,
    pub gain_db: f64,
    pub ceiling_db: f64,
    pub release_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub order: [LayerKind; 4],
    pub tone: ToneSettings,
    pub glue: GlueSettings,
    pub warmth: WarmthSettings,
    pub width: WidthSettings,
    pub limiter: LimiterSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            order: DEFAULT_ORDER,
            tone: ToneSettings { on: true, opacity: 1.0, low_cut_hz: 25.0, low_db: 0.0, mid_db: 0.0, presence_db: 0.0, air_db: 0.0 },
            glue: GlueSettings { on: true, opacity: 1.0, threshold_db: -18.0, ratio: 2.0, attack_ms: 30.0, release_ms: 200.0, makeup_db: 0.0 },
            warmth: WarmthSettings { on: false, opacity: 0.5, drive_db: 6.0 },
            width: WidthSettings { on: true, opacity: 1.0, width: 1.0, mono_below_hz: 120.0 },
            limiter: LimiterSettings { on: true, gain_db: 0.0, ceiling_db: -1.0, release_ms: 80.0 },
        }
    }
}

#[inline]
fn db_to_gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Linear ramp used for layer opacity so toggles and opacity moves never click.
#[derive(Clone, Copy, Debug)]
struct Ramp {
    cur: f64,
    step: f64,
}

impl Ramp {
    fn new(fs: f64, init: f64) -> Self {
        Self { cur: init, step: 1.0 / (0.02 * fs) }
    }
    #[inline]
    fn next(&mut self, target: f64) -> f64 {
        let d = target - self.cur;
        self.cur += d.clamp(-self.step, self.step);
        self.cur
    }
}

/// One-pole smoother (20 ms) for gains.
#[derive(Clone, Copy, Debug)]
struct Smooth {
    cur: f64,
    a: f64,
}

impl Smooth {
    fn new(fs: f64, init: f64) -> Self {
        Self { cur: init, a: 1.0 - (-1.0 / (0.02 * fs)).exp() }
    }
    #[inline]
    fn next(&mut self, target: f64) -> f64 {
        self.cur += (target - self.cur) * self.a;
        self.cur
    }
}

struct Tone {
    fs: f64,
    f: [[Biquad; 5]; 2],
    cached: Option<ToneSettings>,
}

impl Tone {
    fn update(&mut self, s: &ToneSettings) {
        let key = ToneSettings { on: true, opacity: 1.0, ..*s };
        if self.cached == Some(key) {
            return;
        }
        self.cached = Some(key);
        let fs = self.fs;
        let hp = if s.low_cut_hz <= 10.5 { Coefs::identity() } else { Coefs::highpass(fs, s.low_cut_hz, 0.707) };
        let coefs = [
            hp,
            Coefs::low_shelf(fs, 110.0, 0.707, s.low_db),
            Coefs::peak(fs, 400.0, 0.8, s.mid_db),
            Coefs::peak(fs, 3000.0, 0.9, s.presence_db),
            Coefs::high_shelf(fs, 10000.0, 0.707, s.air_db),
        ];
        for ch in &mut self.f {
            for (f, c) in ch.iter_mut().zip(coefs) {
                f.c = c;
            }
        }
    }

    #[inline]
    fn process(&mut self, x: [f64; 2]) -> [f64; 2] {
        let mut out = x;
        for (ch, v) in out.iter_mut().enumerate() {
            for f in &mut self.f[ch] {
                *v = f.process(*v);
            }
        }
        out
    }
}

struct Glue {
    fs: f64,
    env: f64,
    gr_db: f64,
    att: f64,
    rel: f64,
    makeup: Smooth,
}

impl Glue {
    fn update(&mut self, s: &GlueSettings) {
        self.att = (-1.0 / (s.attack_ms.max(0.01) * 0.001 * self.fs)).exp();
        self.rel = (-1.0 / (s.release_ms.max(1.0) * 0.001 * self.fs)).exp();
    }

    /// Static curve with a 6 dB soft knee. Returns gain change in dB (<= 0).
    #[inline]
    fn curve(level_db: f64, thr: f64, ratio: f64) -> f64 {
        const KNEE: f64 = 6.0;
        let over = level_db - thr;
        let slope = 1.0 / ratio.max(1.0) - 1.0;
        if 2.0 * over < -KNEE {
            0.0
        } else if 2.0 * over.abs() <= KNEE {
            slope * (over + KNEE / 2.0).powi(2) / (2.0 * KNEE)
        } else {
            slope * over
        }
    }

    #[inline]
    fn process(&mut self, x: [f64; 2], s: &GlueSettings) -> [f64; 2] {
        // Peak envelope (instant rise, release-time fall), then attack smoothing on the gain.
        let peak = x[0].abs().max(x[1].abs());
        self.env = peak.max(self.env * self.rel);
        let level_db = 20.0 * (self.env + 1e-12).log10();
        let target = Self::curve(level_db, s.threshold_db, s.ratio);
        self.gr_db = if target < self.gr_db { target + (self.gr_db - target) * self.att } else { target };
        let g = db_to_gain(self.gr_db) * db_to_gain(self.makeup.next(s.makeup_db));
        [x[0] * g, x[1] * g]
    }
}

#[inline]
fn warmth(x: [f64; 2], drive_db: f64) -> [f64; 2] {
    let g = db_to_gain(drive_db);
    // Unity gain for small signals, smooth saturation towards 1/g for loud ones.
    [(x[0] * g).tanh() / g, (x[1] * g).tanh() / g]
}

struct Width {
    fs: f64,
    side_hp: Biquad,
    cached_hz: f64,
    width: Smooth,
}

impl Width {
    fn update(&mut self, s: &WidthSettings) {
        if s.mono_below_hz != self.cached_hz {
            self.cached_hz = s.mono_below_hz;
            self.side_hp.c = if s.mono_below_hz <= 20.0 {
                Coefs::identity()
            } else {
                Coefs::highpass(self.fs, s.mono_below_hz, 0.707)
            };
        }
    }

    #[inline]
    fn process(&mut self, x: [f64; 2], s: &WidthSettings) -> [f64; 2] {
        let m = 0.5 * (x[0] + x[1]);
        let side = self.side_hp.process(0.5 * (x[0] - x[1])) * self.width.next(s.width);
        [m + side, m - side]
    }
}

/// Lookahead true-peak limiter. Gain path: target -> instant-attack/exponential-release ->
/// sliding minimum over `hold` samples -> moving average over `look` samples, while the audio is
/// delayed by `delay` samples, so the gain is already down when a peak reaches the output.
pub struct Limiter {
    look: usize,
    delay: usize,
    tp: [TruePeak; 2],
    audio: Vec<[f64; 2]>,
    audio_pos: usize,
    targets: Vec<f64>,
    targets_pos: usize,
    avg_buf: Vec<f64>,
    avg_pos: usize,
    avg_sum: f64,
    rel_state: f64,
    rel: f64,
    gain: Smooth,
    fs: f64,
}

impl Limiter {
    fn new(fs: f64) -> Self {
        let look = ((fs * 0.0015).round() as usize).max(4);
        let hold = look + 12;
        let delay = look + 12;
        Self {
            look,
            delay,
            tp: [TruePeak::default(); 2],
            audio: vec![[0.0; 2]; delay],
            audio_pos: 0,
            targets: vec![1.0; hold + 1],
            targets_pos: 0,
            avg_buf: vec![1.0; look],
            avg_pos: 0,
            avg_sum: look as f64,
            rel_state: 1.0,
            rel: 0.0,
            gain: Smooth::new(fs, 1.0),
            fs,
        }
    }

    pub fn latency(&self) -> usize {
        self.delay
    }

    fn update(&mut self, s: &LimiterSettings) {
        self.rel = (-1.0 / (s.release_ms.max(1.0) * 0.001 * self.fs)).exp();
    }

    /// Returns the delayed (input-gained) sample and the limiter gain for it.
    #[inline]
    fn process(&mut self, x: [f64; 2], s: &LimiterSettings) -> ([f64; 2], f64) {
        let g_in = self.gain.next(db_to_gain(s.gain_db));
        let x = [x[0] * g_in, x[1] * g_in];
        let ceiling = db_to_gain(s.ceiling_db);

        let peak = self.tp[0].push(x[0]).max(self.tp[1].push(x[1])).max(x[0].abs()).max(x[1].abs());
        let target = if peak > ceiling { ceiling / peak } else { 1.0 };

        self.targets[self.targets_pos] = target;
        self.targets_pos = (self.targets_pos + 1) % self.targets.len();
        let held = self.targets.iter().copied().fold(1.0, f64::min);

        self.rel_state = if held < self.rel_state { held } else { held + (self.rel_state - held) * self.rel };

        self.avg_sum += self.rel_state - self.avg_buf[self.avg_pos];
        self.avg_buf[self.avg_pos] = self.rel_state;
        self.avg_pos = (self.avg_pos + 1) % self.look;
        if self.avg_pos == 0 {
            self.avg_sum = self.avg_buf.iter().sum(); // stop float drift
        }
        let g = (self.avg_sum / self.look as f64).min(1.0);

        let delayed = self.audio[self.audio_pos];
        self.audio[self.audio_pos] = x;
        self.audio_pos = (self.audio_pos + 1) % self.delay;
        (delayed, g)
    }
}

/// Per-block report from the chain.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockReport {
    /// Largest Glue gain reduction in the block, dB (positive number).
    pub glue_gr_db: f64,
    /// Largest limiter gain reduction in the block, dB (positive number).
    pub limiter_gr_db: f64,
}

pub struct Chain {
    tone: Tone,
    glue: Glue,
    width: Width,
    limiter: Limiter,
    mix: [Ramp; 4],
    limiter_mix: Ramp,
}

impl Chain {
    pub fn new(fs: f64) -> Self {
        Self {
            tone: Tone { fs, f: [[Biquad::default(); 5]; 2], cached: None },
            glue: Glue { fs, env: 0.0, gr_db: 0.0, att: 0.0, rel: 0.0, makeup: Smooth::new(fs, 1.0) },
            width: Width { fs, side_hp: Biquad::default(), cached_hz: -1.0, width: Smooth::new(fs, 1.0) },
            limiter: Limiter::new(fs),
            mix: [Ramp::new(fs, 0.0); 4],
            limiter_mix: Ramp::new(fs, 1.0),
        }
    }

    /// Latency in samples (the limiter lookahead, also applied when the limiter is off).
    pub fn latency(&self) -> usize {
        self.limiter.latency()
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], s: &Settings) -> BlockReport {
        self.tone.update(&s.tone);
        self.glue.update(&s.glue);
        self.width.update(&s.width);
        self.limiter.update(&s.limiter);

        let mut report = BlockReport::default();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut x = [*l as f64, *r as f64];
            for kind in s.order {
                let (on, opacity) = match kind {
                    LayerKind::Tone => (s.tone.on, s.tone.opacity),
                    LayerKind::Glue => (s.glue.on, s.glue.opacity),
                    LayerKind::Warmth => (s.warmth.on, s.warmth.opacity),
                    LayerKind::Width => (s.width.on, s.width.opacity),
                };
                let target = if on { opacity.clamp(0.0, 1.0) } else { 0.0 };
                let m = self.mix[kind as usize].next(target);
                if m <= 0.0 {
                    continue;
                }
                let wet = match kind {
                    LayerKind::Tone => self.tone.process(x),
                    LayerKind::Glue => {
                        let y = self.glue.process(x, &s.glue);
                        report.glue_gr_db = report.glue_gr_db.max(-self.glue.gr_db);
                        y
                    }
                    LayerKind::Warmth => warmth(x, s.warmth.drive_db),
                    LayerKind::Width => self.width.process(x, &s.width),
                };
                x = [x[0] + (wet[0] - x[0]) * m, x[1] + (wet[1] - x[1]) * m];
            }

            // The limiter always runs (keeps latency constant); "off" crossfades to its delayed input.
            let (delayed, g) = self.limiter.process(x, &s.limiter);
            let lm = self.limiter_mix.next(if s.limiter.on { 1.0 } else { 0.0 });
            // Limited path: gain reduction plus a sample-level safety clip at the ceiling.
            let c = db_to_gain(s.limiter.ceiling_db);
            let limited = [(delayed[0] * g).clamp(-c, c), (delayed[1] * g).clamp(-c, c)];
            if lm > 0.0 {
                report.limiter_gr_db = report.limiter_gr_db.max(-20.0 * g.log10());
            }
            let out = [delayed[0] + (limited[0] - delayed[0]) * lm, delayed[1] + (limited[1] - delayed[1]) * lm];
            *l = out[0] as f32;
            *r = out[1] as f32;
        }
        report
    }
}

#[cfg(test)]
mod tests;
