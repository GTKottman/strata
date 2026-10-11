//! The Strata mastering chain: four reorderable adjustment layers (Tone, Glue, Warmth, Width),
//! each with on/off and opacity (dry/wet), followed by a fixed true-peak limiter output layer.
//! Everything here is host independent so the offline renderer and the tests use the same code.

pub mod filter;
pub mod meter;
pub mod oversample;

use filter::{Biquad, Coefs};
use meter::TruePeak;
use oversample::{Delay, Oversampler};

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
    /// High-pass on what the compressor listens to (not on the audio), so the kick and bass don't
    /// drive it. 20 Hz or less = off.
    pub sc_hpf_hz: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarmthSettings {
    pub on: bool,
    pub opacity: f64,
    pub drive_db: f64,
    /// Saturation of the band below `GRIT_SPLIT_HZ` only (0 = off): low, harsh bass.
    pub grit_db: f64,
}

/// Where the bass grit splits the signal.
pub const GRIT_SPLIT_HZ: f64 = 180.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WidthSettings {
    pub on: bool,
    pub opacity: f64,
    /// Side gain: 0 = mono, 1 = unchanged, 2 = double.
    pub width: f64,
    /// Side content below this is removed (bass to mono). 20 Hz or less = off.
    pub mono_below_hz: f64,
    /// Linear-phase bass-to-mono: no phase shift on the side channel, costs `WIDTH_FIR_SECONDS / 2`
    /// of latency (reported to the host).
    pub linear_phase: bool,
}

/// Length of the linear-phase Width filter.
pub const WIDTH_FIR_SECONDS: f64 = 0.085;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterSettings {
    pub on: bool,
    pub gain_db: f64,
    pub ceiling_db: f64,
    pub release_ms: f64,
    /// Program-dependent release: short peaks recover at about half the release time, sustained
    /// limiting at about three times it, so dense loud passages don't pump or distort the bass.
    pub auto_release: bool,
    /// Soft clipper before the limiter (4x oversampled): shaves short spiky peaks so the limiter
    /// has less to do. Suits dirty genres; keep it off where the kick and sub must stay clean.
    pub clip_on: bool,
    /// Where the clipper starts, in dB above the ceiling: peaks are clipped to this, then the
    /// limiter takes them the rest of the way. Lower = more clipping, less limiting.
    pub clip_db: f64,
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
            glue: GlueSettings { on: true, opacity: 1.0, threshold_db: -18.0, ratio: 2.0, attack_ms: 30.0, release_ms: 200.0, makeup_db: 0.0, sc_hpf_hz: 100.0 },
            warmth: WarmthSettings { on: false, opacity: 0.5, drive_db: 6.0, grit_db: 0.0 },
            // Bass to mono is off unless a song needs it (mastering engineers use it as a fix, not a default).
            width: WidthSettings { on: true, opacity: 1.0, width: 1.0, mono_below_hz: 20.0, linear_phase: false },
            limiter: LimiterSettings { on: true, gain_db: 0.0, ceiling_db: -1.0, release_ms: 80.0, auto_release: true, clip_on: false, clip_db: 3.0 },
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
            // Presence and Air sit close enough to Nyquist to cramp with the bilinear design.
            Coefs::matched_peak(fs, 3000.0, 0.9, s.presence_db),
            Coefs::matched_high_shelf(fs, 10000.0, 0.707, s.air_db),
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
    /// Detector high-pass per channel.
    sc: [Biquad; 2],
    sc_hz: f64,
    env: f64,
    gr_db: f64,
    att: f64,
    rel: f64,
    makeup: Smooth,
}

impl Glue {
    fn update(&mut self, s: &GlueSettings) {
        if s.sc_hpf_hz != self.sc_hz {
            self.sc_hz = s.sc_hpf_hz;
            let c = if s.sc_hpf_hz <= 20.0 { Coefs::identity() } else { Coefs::highpass(self.fs, s.sc_hpf_hz, 0.707) };
            self.sc.iter_mut().for_each(|f| f.c = c);
        }
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
        // Peak envelope of the high-passed detector signal (instant rise, release-time fall), then
        // attack smoothing on the gain. The gain itself goes on the full-range audio.
        let peak = self.sc[0].process(x[0]).abs().max(self.sc[1].process(x[1]).abs());
        self.env = peak.max(self.env * self.rel);
        let level_db = 20.0 * (self.env + 1e-12).log10();
        let target = Self::curve(level_db, s.threshold_db, s.ratio);
        self.gr_db = if target < self.gr_db { target + (self.gr_db - target) * self.att } else { target };
        let g = db_to_gain(self.gr_db) * db_to_gain(self.makeup.next(s.makeup_db));
        [x[0] * g, x[1] * g]
    }
}

#[inline]
fn saturate(x: f64, drive_db: f64) -> f64 {
    if drive_db <= 0.0 {
        return x;
    }
    let g = db_to_gain(drive_db);
    // Unity gain for small signals, smooth saturation towards 1/g for loud ones.
    (x * g).tanh() / g
}

/// Asymmetric saturation for the bass band: positive half clips harder, which adds even harmonics
/// (the growl of an overdriven bass amp) as well as odd ones. Unity slope at zero, no DC for small signals.
#[inline]
fn grit(x: f64, drive_db: f64) -> f64 {
    if drive_db <= 0.0 {
        return x;
    }
    let g = db_to_gain(drive_db);
    let y = x * g;
    let s = if y >= 0.0 { y.tanh() } else { (1.4 * y).tanh() / 1.4 };
    s / g
}

/// Warmth layer: full-band saturation plus bass grit on the low band (Linkwitz-Riley style split:
/// low = two 2nd-order low-passes in series, high = input - low, so with no grit it sums back exactly).
/// The full-band saturation runs 4x oversampled, so its harmonics above Nyquist don't fold back
/// as inharmonic tones; the layer is `oversample::LATENCY` samples late, dry path included.
struct Warmth {
    lp: [[Biquad; 2]; 2],
    dc: [Biquad; 2],
    os: [Oversampler; 2],
    dry: [Delay; 2],
}

impl Warmth {
    fn new(fs: f64) -> Self {
        let c = Coefs::lowpass(fs, GRIT_SPLIT_HZ, 0.707);
        let hp = Coefs::highpass(fs, 12.0, 0.707);
        Self {
            lp: [[Biquad::new(c), Biquad::new(c)]; 2],
            dc: [Biquad::new(hp); 2],
            os: [Oversampler::new(), Oversampler::new()],
            dry: [Delay::new(oversample::LATENCY), Delay::new(oversample::LATENCY)],
        }
    }

    /// Returns (dry, wet), both delayed by `oversample::LATENCY`.
    #[inline]
    fn process(&mut self, x: [f64; 2], s: &WarmthSettings) -> ([f64; 2], [f64; 2]) {
        let mut out = [0.0; 2];
        let mut dry = [0.0; 2];
        for ch in 0..2 {
            dry[ch] = self.dry[ch].process(x[ch]);
            let mut v = x[ch];
            let [a, b] = &mut self.lp[ch];
            let low = b.process(a.process(v));
            if s.grit_db > 0.0 {
                // Asymmetric clipping makes a little DC: remove it from the gritty band only.
                let low_g = self.dc[ch].process(grit(low, s.grit_db));
                v = (v - low) + low_g;
            }
            // (The filters run even with no grit, so turning it on does not click.)
            let drive = s.drive_db;
            out[ch] = self.os[ch].process(v, |u| saturate(u, drive));
        }
        (dry, out)
    }
}

/// Width layer: side gain plus bass to mono. Minimum-phase mode high-passes the side channel with a
/// biquad (no latency, but it shifts the side's phase near the crossover). Linear-phase mode takes
/// the side's low band out with a windowed-sinc FIR instead and delays mid and dry to match, so the
/// image above the crossover is untouched (latency `fir_delay`).
struct Width {
    fs: f64,
    side_hp: Biquad,
    cached_hz: f64,
    width: Smooth,
    fir: Vec<f64>,
    fir_hz: f64,
    /// Side history, written twice so a contiguous window always exists.
    side_hist: Vec<f64>,
    side_pos: usize,
    mid_delay: Delay,
    dry: [Delay; 2],
}

impl Width {
    fn new(fs: f64) -> Self {
        let n = (fs * WIDTH_FIR_SECONDS).round() as usize | 1;
        let d = (n - 1) / 2;
        Self {
            fs,
            side_hp: Biquad::default(),
            cached_hz: -1.0,
            width: Smooth::new(fs, 1.0),
            fir: vec![0.0; n],
            fir_hz: -1.0,
            side_hist: vec![0.0; 2 * n],
            side_pos: 0,
            mid_delay: Delay::new(d),
            dry: [Delay::new(d), Delay::new(d)],
        }
    }

    /// Latency of the linear-phase mode in samples.
    fn fir_delay(&self) -> usize {
        (self.fir.len() - 1) / 2
    }

    fn update(&mut self, s: &WidthSettings) {
        if s.mono_below_hz != self.cached_hz {
            self.cached_hz = s.mono_below_hz;
            self.side_hp.c = if s.mono_below_hz <= 20.0 {
                Coefs::identity()
            } else {
                Coefs::highpass(self.fs, s.mono_below_hz, 0.707)
            };
        }
        if s.linear_phase && s.mono_below_hz > 20.0 && (s.mono_below_hz - self.fir_hz).abs() > 0.01 {
            self.fir_hz = s.mono_below_hz;
            // Blackman-windowed sinc low-pass at the crossover, unity DC gain. The high band is the
            // delayed side minus this, so low + high sum back exactly.
            let n = self.fir.len();
            let mid = (n - 1) as f64 / 2.0;
            let fc = s.mono_below_hz / self.fs;
            for (i, h) in self.fir.iter_mut().enumerate() {
                let t = i as f64 - mid;
                let sinc = if t == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t) };
                let w = 2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64;
                *h = sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos());
            }
            let sum: f64 = self.fir.iter().sum();
            self.fir.iter_mut().for_each(|h| *h /= sum);
        }
    }

    /// Returns (dry, wet); both are `fir_delay` samples late in linear-phase mode.
    #[inline]
    fn process(&mut self, x: [f64; 2], s: &WidthSettings) -> ([f64; 2], [f64; 2]) {
        let m = 0.5 * (x[0] + x[1]);
        let side = 0.5 * (x[0] - x[1]);
        let w = self.width.next(s.width);
        // The delay lines always run, so switching mode never plays stale audio.
        let n = self.fir.len();
        self.side_hist[self.side_pos] = side;
        self.side_hist[self.side_pos + n] = side;
        self.side_pos = (self.side_pos + 1) % n;
        let m_late = self.mid_delay.process(m);
        let dry_late = [self.dry[0].process(x[0]), self.dry[1].process(x[1])];
        if s.linear_phase {
            let window = &self.side_hist[self.side_pos..self.side_pos + n];
            let side_late = window[self.fir_delay()];
            let low = if s.mono_below_hz > 20.0 { self.fir.iter().zip(window).map(|(a, b)| a * b).sum() } else { 0.0 };
            let sh = (side_late - low) * w;
            (dry_late, [m_late + sh, m_late - sh])
        } else {
            let sh = self.side_hp.process(side) * w;
            (x, [m + sh, m - sh])
        }
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
    /// Auto release: fast and slow gain reduction (dB) and their coefficients.
    fast_db: f64,
    slow_db: f64,
    rel_fast: f64,
    rel_slow: f64,
    att_slow: f64,
    gain: Smooth,
    fs: f64,
    /// Clipper before the limiter (oversampled), a plain delay of the same length beside it, and
    /// the crossfade between them.
    clip_os: [Oversampler; 2],
    clip_dry: [Delay; 2],
    clip_mix: Ramp,
}

/// Soft clipper: linear up to 80% of `t`, then a tanh curve that never passes `t` (slope 1 at the knee).
#[inline]
fn soft_clip(x: f64, t: f64) -> f64 {
    let k = 0.8 * t;
    let a = x.abs();
    if a <= k {
        x
    } else {
        (k + (t - k) * ((a - k) / (t - k)).tanh()).copysign(x)
    }
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
            fast_db: 0.0,
            slow_db: 0.0,
            rel_fast: 0.0,
            rel_slow: 0.0,
            att_slow: (-1.0 / (0.08 * fs)).exp(),
            gain: Smooth::new(fs, 1.0),
            fs,
            clip_os: [Oversampler::new(), Oversampler::new()],
            clip_dry: [Delay::new(oversample::LATENCY), Delay::new(oversample::LATENCY)],
            clip_mix: Ramp::new(fs, 0.0),
        }
    }

    /// Lookahead plus the clipper's oversampling (always in the path, so latency never changes).
    pub fn latency(&self) -> usize {
        self.delay + oversample::LATENCY
    }

    fn update(&mut self, s: &LimiterSettings) {
        let coef = |ms: f64| (-1.0 / (ms.max(1.0) * 0.001 * self.fs)).exp();
        self.rel = coef(s.release_ms);
        self.rel_fast = coef(0.5 * s.release_ms);
        self.rel_slow = coef(3.0 * s.release_ms);
    }

    /// Returns the delayed (input-gained) sample and the limiter gain for it.
    #[inline]
    fn process(&mut self, x: [f64; 2], s: &LimiterSettings) -> ([f64; 2], f64) {
        let g_in = self.gain.next(db_to_gain(s.gain_db));
        let x = [x[0] * g_in, x[1] * g_in];
        let ceiling = db_to_gain(s.ceiling_db);

        // Clipper: both paths always run; "off" crossfades to the plain delay.
        let t = ceiling * db_to_gain(s.clip_db.max(0.0));
        let cm = self.clip_mix.next(if s.clip_on { 1.0 } else { 0.0 });
        let mut y = [0.0; 2];
        for ch in 0..2 {
            let clipped = self.clip_os[ch].process(x[ch], |u| soft_clip(u, t));
            let plain = self.clip_dry[ch].process(x[ch]);
            y[ch] = plain + (clipped - plain) * cm;
        }
        let x = y;

        let peak = self.tp[0].push(x[0]).max(self.tp[1].push(x[1])).max(x[0].abs()).max(x[1].abs());
        let target = if peak > ceiling { ceiling / peak } else { 1.0 };

        self.targets[self.targets_pos] = target;
        self.targets_pos = (self.targets_pos + 1) % self.targets.len();
        let held = self.targets.iter().copied().fold(1.0, f64::min);

        self.rel_state = if s.auto_release {
            // Program-dependent release: the fast state catches every peak and lets go quickly; the
            // slow state follows sustained limiting and lets go slowly. The deeper of the two wins,
            // so a lone transient recovers fast and a dense loud passage doesn't pump.
            let r = -20.0 * held.max(1e-9).log10();
            self.fast_db = if r >= self.fast_db { r } else { r + (self.fast_db - r) * self.rel_fast };
            let c = if r > self.slow_db { self.att_slow } else { self.rel_slow };
            self.slow_db = r + (self.slow_db - r) * c;
            db_to_gain(-self.fast_db.max(self.slow_db))
        } else if held < self.rel_state {
            held
        } else {
            held + (self.rel_state - held) * self.rel
        };

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
    warmth: Warmth,
    width: Width,
    limiter: Limiter,
    mix: [Ramp; 4],
    limiter_mix: Ramp,
}

impl Chain {
    pub fn new(fs: f64) -> Self {
        Self {
            tone: Tone { fs, f: [[Biquad::default(); 5]; 2], cached: None },
            glue: Glue { fs, sc: [Biquad::default(); 2], sc_hz: -1.0, env: 0.0, gr_db: 0.0, att: 0.0, rel: 0.0, makeup: Smooth::new(fs, 1.0) },
            warmth: Warmth::new(fs),
            width: Width::new(fs),
            limiter: Limiter::new(fs),
            mix: [Ramp::new(fs, 0.0); 4],
            limiter_mix: Ramp::new(fs, 1.0),
        }
    }

    /// Latency in samples for these settings: the limiter (lookahead + clipper oversampling) and
    /// Warmth's oversampling always count, also when they are off; the linear-phase Width adds its
    /// filter delay only while that mode is on.
    pub fn latency_with(&self, s: &Settings) -> usize {
        self.limiter.latency() + oversample::LATENCY + if s.width.linear_phase { self.width.fir_delay() } else { 0 }
    }

    /// Latency with the default settings.
    pub fn latency(&self) -> usize {
        self.latency_with(&Settings::default())
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
                // Layers with latency run even when off, so the delay (and the host's compensation) holds.
                let delays = kind == LayerKind::Warmth || (kind == LayerKind::Width && s.width.linear_phase);
                if m <= 0.0 && !delays {
                    continue;
                }
                let (dry, wet) = match kind {
                    LayerKind::Tone => (x, self.tone.process(x)),
                    LayerKind::Glue => {
                        let y = self.glue.process(x, &s.glue);
                        report.glue_gr_db = report.glue_gr_db.max(-self.glue.gr_db);
                        (x, y)
                    }
                    LayerKind::Warmth => self.warmth.process(x, &s.warmth),
                    LayerKind::Width => self.width.process(x, &s.width),
                };
                x = [dry[0] + (wet[0] - dry[0]) * m, dry[1] + (wet[1] - dry[1]) * m];
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
