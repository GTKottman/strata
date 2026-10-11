//! Strata Cylinder: goes on every channel. It learns what its channel does per beat (level, attacks,
//! three bands) while the Engine is learning, and plays back the gain the Engine planned for it,
//! locked to the song position.
//!
//! Cylinders and the Engine live in the same DLL, so in the host's process they find each other
//! through the static registry below (FL loads plugins in-process unless a plugin is bridged).

use crate::dsp::filter::{Biquad, Coefs};
use crate::orchestra::Role;
use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Color32, RichText};
use nih_plug_egui::{create_egui_editor, widgets::ParamSlider, EguiState};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

pub const MAX_BEATS: usize = 4096;

// ---- shared between all instances in the process ----

/// Bumped by the Engine when a new learn pass starts; Cylinders clear their memory when it changes.
pub static LEARN_GEN: AtomicU32 = AtomicU32::new(0);
pub static LEARNING: AtomicBool = AtomicBool::new(false);
/// Engine-controlled amounts (0..1) for the static balance and the moments.
pub static STATIC_AMOUNT: AtomicF32 = AtomicF32::new(1.0);
pub static MOMENT_AMOUNT: AtomicF32 = AtomicF32::new(1.0);
pub static BALANCE_ON: AtomicBool = AtomicBool::new(true);
/// Engine-controlled amount (0..1) of the per-channel width plan.
pub static WIDTH_AMOUNT: AtomicF32 = AtomicF32::new(1.0);
/// The Engine's genre (index into `Genre::ALL`), used when it plans.
pub static GENRE: AtomicU32 = AtomicU32::new(0);
/// Beats per bar as last seen by any instance (for the report).
pub static BEATS_PER_BAR: AtomicU32 = AtomicU32::new(4);

/// Bumped by the Engine's "Reset meters": every Cylinder restarts its pass statistics.
pub static STATS_GEN: AtomicU32 = AtomicU32::new(0);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static REGISTRY: Mutex<Vec<Weak<CylShared>>> = Mutex::new(Vec::new());

pub fn cylinders() -> Vec<Arc<CylShared>> {
    let mut reg = REGISTRY.lock().unwrap();
    reg.retain(|w| w.strong_count() > 0);
    reg.iter().filter_map(Weak::upgrade).collect()
}

/// What a Cylinder keeps in the project.
#[derive(Clone, Serialize, Deserialize)]
pub struct CylState {
    pub label: String,
    pub static_db: f32,
    pub curve: Vec<f32>,
    /// Planned side gain (1 = as it is). Older projects have none: as it is.
    #[serde(default = "one")]
    pub width: f32,
    /// Planned "side to mono below" frequency, Hz (0 = off).
    #[serde(default)]
    pub mono_below_hz: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for CylState {
    fn default() -> Self {
        Self { label: String::new(), static_db: 0.0, curve: Vec::new(), width: 1.0, mono_below_hz: 0.0 }
    }
}

pub struct CylShared {
    pub id: u32,
    pub energy: Vec<AtomicF32>,
    pub onsets: Vec<AtomicF32>,
    pub low: Vec<AtomicF32>,
    pub mid: Vec<AtomicF32>,
    pub high: Vec<AtomicF32>,
    pub side: Vec<AtomicF32>,
    pub learned_beats: AtomicU32,
    pub curve: Vec<AtomicF32>,
    pub curve_len: AtomicU32,
    pub static_db: AtomicF32,
    pub width: AtomicF32,
    pub mono_below_hz: AtomicF32,
    pub live_gain_db: AtomicF32,
    /// Statistics since the last meter reset (diagnostics).
    pub stats: [AtomicF32; STAT_COUNT],
    pub role: AtomicU32,
    pub state: Arc<Mutex<CylState>>,
}

impl CylShared {
    fn new(state: Arc<Mutex<CylState>>) -> Self {
        let arr = || (0..MAX_BEATS).map(|_| AtomicF32::new(0.0)).collect::<Vec<_>>();
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            energy: arr(),
            onsets: arr(),
            low: arr(),
            mid: arr(),
            high: arr(),
            side: arr(),
            learned_beats: AtomicU32::new(0),
            curve: arr(),
            curve_len: AtomicU32::new(0),
            static_db: AtomicF32::new(0.0),
            width: AtomicF32::new(1.0),
            mono_below_hz: AtomicF32::new(0.0),
            live_gain_db: AtomicF32::new(0.0),
            stats: std::array::from_fn(|_| AtomicF32::new(0.0)),
            role: AtomicU32::new(0),
            state,
        }
    }

    pub fn label(&self) -> String {
        self.state.lock().map(|s| s.label.clone()).unwrap_or_default()
    }

    /// Install a plan result (called by the Engine, off the audio thread).
    pub fn set_plan(&self, static_db: f32, curve: &[f32], width: f32, mono_below_hz: f32) {
        let n = curve.len().min(MAX_BEATS);
        for (slot, v) in self.curve.iter().zip(&curve[..n]) {
            slot.store(*v, Ordering::Relaxed);
        }
        self.curve_len.store(n as u32, Ordering::Relaxed);
        self.static_db.store(static_db, Ordering::Relaxed);
        self.width.store(width, Ordering::Relaxed);
        self.mono_below_hz.store(mono_below_hz, Ordering::Relaxed);
        if let Ok(mut s) = self.state.lock() {
            s.static_db = static_db;
            s.curve = curve[..n].to_vec();
            s.width = width;
            s.mono_below_hz = mono_below_hz;
        }
    }

    pub fn clear_plan(&self) {
        self.set_plan(0.0, &[], 1.0, 0.0);
    }

    fn load_state(&self) {
        let s = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        self.set_plan(s.static_db, &s.curve, s.width, s.mono_below_hz);
    }

    fn clear_learning(&self) {
        for v in [&self.energy, &self.onsets, &self.low, &self.mid, &self.high, &self.side] {
            v.iter().for_each(|a| a.store(0.0, Ordering::Relaxed));
        }
        self.learned_beats.store(0, Ordering::Relaxed);
    }
}

// ---- pass statistics ----

pub const STAT_COUNT: usize = 9;
pub const STAT_NAMES: [&str; STAT_COUNT] = ["sec", "play%", "in dB", "out dB", "gain", "pos0", "pos1", "bpm", "rate"];

/// Accumulated on the audio thread, published to `CylShared::stats` once per block.
#[derive(Default)]
struct PassStats {
    frames: f64,
    playing: f64,
    in_sq: f64,
    out_sq: f64,
    gain_db: f64,
    pos_min: f64,
    pos_max: f64,
    tempo: f64,
}

impl PassStats {
    fn publish(&self, fs: f64, out: &[AtomicF32; STAT_COUNT]) {
        let n = self.frames.max(1.0);
        let db = |x: f64| if x > 0.0 { 10.0 * (x / n).log10() } else { -200.0 };
        let v = [
            self.frames / fs,
            100.0 * self.playing / n,
            db(self.in_sq),
            db(self.out_sq),
            self.gain_db / n,
            if self.pos_min.is_finite() { self.pos_min } else { -1.0 },
            if self.pos_max.is_finite() { self.pos_max } else { -1.0 },
            self.tempo,
            fs,
        ];
        for (a, x) in out.iter().zip(v) {
            a.store(x as f32, Ordering::Relaxed);
        }
    }
}

// ---- feature extraction ----

/// Per-sample analysis: three bands and an attack detector. `push` returns true on an attack.
pub struct Features {
    lp1: f64,
    lp2: f64,
    a1: f64,
    a2: f64,
    fast: f64,
    slow: f64,
    fast_a: f64,
    slow_a: f64,
    hold: usize,
    hold_len: usize,
    pub acc: [f64; 4], // energy, low, mid, high
    pub side_acc: f64,
    pub onsets: u32,
    pub count: u32,
}

impl Features {
    pub fn new(fs: f64) -> Self {
        let c = |hz: f64| 1.0 - (-2.0 * std::f64::consts::PI * hz / fs).exp();
        let t = |ms: f64| 1.0 - (-1.0 / (ms * 0.001 * fs)).exp();
        Self {
            lp1: 0.0,
            lp2: 0.0,
            a1: c(250.0),
            a2: c(2500.0),
            fast: 0.0,
            slow: 0.0,
            fast_a: t(5.0),
            slow_a: t(50.0),
            hold: 0,
            hold_len: (0.06 * fs) as usize,
            acc: [0.0; 4],
            side_acc: 0.0,
            onsets: 0,
            count: 0,
        }
    }

    /// The side signal ((L - R) / 2) of the sample just pushed.
    #[inline]
    pub fn push_side(&mut self, s: f64) {
        self.side_acc += s * s;
    }

    #[inline]
    pub fn push(&mut self, x: f64) -> bool {
        self.lp1 += (x - self.lp1) * self.a1;
        self.lp2 += (x - self.lp2) * self.a2;
        let (low, mid, high) = (self.lp1, self.lp2 - self.lp1, x - self.lp2);
        self.acc[0] += x * x;
        self.acc[1] += low * low;
        self.acc[2] += mid * mid;
        self.acc[3] += high * high;
        self.count += 1;

        let r = x.abs();
        self.fast += (r - self.fast) * self.fast_a;
        self.slow += (r - self.slow) * self.slow_a;
        if self.hold > 0 {
            // Still inside the last attack: its rise belongs to the same note.
            self.hold -= 1;
            self.slow = self.slow.max(self.fast);
        }
        // An attack: the 5 ms envelope jumps 4 dB over the 50 ms one, above -60 dBFS.
        if self.hold == 0 && self.fast > 1.6 * self.slow + 1e-6 && self.fast > 1e-3 {
            self.hold = self.hold_len;
            self.onsets += 1;
            // The note is now the reference: the next attack has to jump 4 dB over it.
            self.slow = self.fast;
            return true;
        }
        false
    }

    /// Energy, onsets, low, mid, high, side for the beat that just ended.
    pub fn take(&mut self) -> Option<[f32; 6]> {
        if self.count == 0 {
            return None;
        }
        let n = self.count as f64;
        let out = [
            (self.acc[0] / n) as f32,
            self.onsets as f32,
            (self.acc[1] / n) as f32,
            (self.acc[2] / n) as f32,
            (self.acc[3] / n) as f32,
            (self.side_acc / n) as f32,
        ];
        self.acc = [0.0; 4];
        self.side_acc = 0.0;
        self.onsets = 0;
        self.count = 0;
        Some(out)
    }
}

// ---- width ----

/// Mid/side width with the side high-passed below a frequency (bass to mono). Pure, so it is tested.
pub struct WidthStage {
    fs: f64,
    hp: Biquad,
    hz: f64,
    w: f64,
    w_a: f64,
}

impl WidthStage {
    pub fn new(fs: f64) -> Self {
        Self { fs, hp: Biquad::new(Coefs::identity()), hz: 0.0, w: 1.0, w_a: 1.0 - (-1.0 / (0.05 * fs)).exp() }
    }

    /// `width` 0 = mono, 1 = as it is, 2 = double side; `mono_below_hz` <= 20 = off.
    #[inline]
    pub fn process(&mut self, l: f64, r: f64, width: f64, mono_below_hz: f64) -> (f64, f64) {
        if (mono_below_hz - self.hz).abs() > 0.5 {
            self.hz = mono_below_hz;
            self.hp.c = if mono_below_hz <= 20.0 { Coefs::identity() } else { Coefs::highpass(self.fs, mono_below_hz, 0.707) };
        }
        self.w += (width - self.w) * self.w_a;
        let m = 0.5 * (l + r);
        let s = self.hp.process(0.5 * (l - r)) * self.w;
        (m + s, m - s)
    }
}

// ---- the plugin ----

#[derive(Params)]
pub struct CylParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,
    #[persist = "cylinder"]
    pub state: Arc<Mutex<CylState>>,
    #[id = "role"]
    pub role: EnumParam<Role>,
    #[id = "trim"]
    pub trim: FloatParam,
    #[id = "follow"]
    pub follow: BoolParam,
}

impl Default for CylParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(380, 300),
            state: Arc::new(Mutex::new(CylState::default())),
            role: EnumParam::new("Role", Role::Auto),
            trim: FloatParam::new("Trim", 0.0, FloatRange::Linear { min: -24.0, max: 12.0 })
                .with_unit(" dB")
                .with_step_size(0.1)
                .with_smoother(SmoothingStyle::Linear(30.0)),
            follow: BoolParam::new("Follow Engine", true),
        }
    }
}

pub struct StrataCylinder {
    params: Arc<CylParams>,
    shared: Arc<CylShared>,
    features: Features,
    fs: f64,
    seen_gen: u32,
    seen_stats: u32,
    stats: PassStats,
    cur_beat: i64,
    gain_db: f64,
    gain_a: f64,
    width: WidthStage,
}

impl Default for StrataCylinder {
    fn default() -> Self {
        let params = Arc::new(CylParams::default());
        let shared = Arc::new(CylShared::new(params.state.clone()));
        REGISTRY.lock().unwrap().push(Arc::downgrade(&shared));
        Self {
            params,
            shared,
            features: Features::new(44100.0),
            fs: 44100.0,
            seen_gen: LEARN_GEN.load(Ordering::Relaxed),
            cur_beat: -1,
            gain_db: 0.0,
            gain_a: 0.0,
            seen_stats: u32::MAX,
            stats: PassStats::default(),
            width: WidthStage::new(44100.0),
        }
    }
}

impl Plugin for StrataCylinder {
    const NAME: &'static str = "Strata Cylinder";
    const VENDOR: &'static str = "GTKottman";
    const URL: &'static str = "https://github.com/GTKottman/strata";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let params = self.params.clone();
        let shared = self.shared.clone();
        create_egui_editor(
            params.editor_state.clone(),
            (),
            |ctx, _| ctx.set_visuals(egui::Visuals::dark()),
            move |ctx, setter, _| {
                egui::CentralPanel::default().frame(egui::Frame::default().fill(Color32::from_rgb(24, 25, 29)).inner_margin(12.0)).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("STRATA").strong().color(Color32::from_rgb(232, 160, 64)));
                        ui.label(RichText::new(format!("Cylinder #{}", shared.id)).strong());
                    });
                    ui.add_space(6.0);
                    egui::Grid::new("cyl").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Name");
                        let mut name = shared.label();
                        if ui.add(egui::TextEdit::singleline(&mut name).desired_width(200.0)).changed() {
                            if let Ok(mut s) = shared.state.lock() {
                                s.label = name;
                            }
                        }
                        ui.end_row();
                        ui.label("Role");
                        egui::ComboBox::from_id_salt("role").selected_text(params.role.value().name()).show_ui(ui, |ui| {
                            for r in Role::ALL {
                                if ui.selectable_label(params.role.value() == r, r.name()).clicked() {
                                    setter.begin_set_parameter(&params.role);
                                    setter.set_parameter(&params.role, r);
                                    setter.end_set_parameter(&params.role);
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Trim");
                        ui.add(ParamSlider::for_param(&params.trim, setter).with_width(200.0));
                        ui.end_row();
                        ui.label("Follow Engine");
                        let mut f = params.follow.value();
                        if ui.checkbox(&mut f, "").changed() {
                            setter.begin_set_parameter(&params.follow);
                            setter.set_parameter(&params.follow, f);
                            setter.end_set_parameter(&params.follow);
                        }
                        ui.end_row();
                    });
                    ui.add_space(8.0);
                    let learned = shared.learned_beats.load(Ordering::Relaxed);
                    let planned = shared.curve_len.load(Ordering::Relaxed);
                    ui.label(RichText::new(format!("Learned beats: {learned}")).monospace());
                    ui.label(RichText::new(format!("Plan: {} beats, static {:+.1} dB", planned, shared.static_db.load(Ordering::Relaxed))).monospace());
                    let (w, hz) = (shared.width.load(Ordering::Relaxed), shared.mono_below_hz.load(Ordering::Relaxed));
                    ui.label(RichText::new(format!("Width: {:.0}%{}", w * 100.0, if hz > 20.0 { format!(", mono below {hz:.0} Hz") } else { String::new() })).monospace());
                    ui.label(RichText::new(format!("Gain now: {:+.1} dB", shared.live_gain_db.load(Ordering::Relaxed))).monospace().size(18.0));
                });
                ctx.request_repaint();
            },
        )
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        self.fs = config.sample_rate as f64;
        self.features = Features::new(self.fs);
        self.gain_a = 1.0 - (-1.0 / (0.06 * self.fs)).exp();
        self.width = WidthStage::new(self.fs);
        self.shared.load_state();
        true
    }

    fn reset(&mut self) {
        self.cur_beat = -1;
        self.features.take();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let sh = &self.shared;
        sh.role.store(self.params.role.value().index(), Ordering::Relaxed);
        let gen = LEARN_GEN.load(Ordering::Relaxed);
        if gen != self.seen_gen {
            self.seen_gen = gen;
            sh.clear_learning();
            self.cur_beat = -1;
            self.features.take();
        }

        let sgen = STATS_GEN.load(Ordering::Relaxed);
        if sgen != self.seen_stats {
            self.seen_stats = sgen;
            self.stats = PassStats { pos_min: f64::INFINITY, pos_max: f64::NEG_INFINITY, ..Default::default() };
        }

        let t = context.transport();
        if let Some(n) = t.time_sig_numerator {
            if n > 0 {
                BEATS_PER_BAR.store(n as u32, Ordering::Relaxed);
            }
        }
        let pos = if t.playing { t.pos_beats() } else { None };
        let beats_per_sample = t.tempo.unwrap_or(120.0) / 60.0 / self.fs;
        let learning = LEARNING.load(Ordering::Relaxed);
        let follow = self.params.follow.value() && BALANCE_ON.load(Ordering::Relaxed);
        let st_amt = STATIC_AMOUNT.load(Ordering::Relaxed) as f64;
        let mo_amt = MOMENT_AMOUNT.load(Ordering::Relaxed) as f64;
        let curve_len = sh.curve_len.load(Ordering::Relaxed) as i64;
        let static_db = sh.static_db.load(Ordering::Relaxed) as f64;
        // Width follows the plan like the gain does; with no plan (or not following) the channel is untouched.
        let planned = follow && (curve_len > 0 || static_db != 0.0);
        let w_amt = WIDTH_AMOUNT.load(Ordering::Relaxed) as f64;
        let (width, mono_hz) = if planned {
            (1.0 + (sh.width.load(Ordering::Relaxed) as f64 - 1.0) * w_amt, if w_amt > 0.0 { sh.mono_below_hz.load(Ordering::Relaxed) as f64 } else { 0.0 })
        } else {
            (1.0, 0.0)
        };

        if pos.is_none() && self.cur_beat >= 0 {
            self.cur_beat = -1;
            self.features.take();
        }

        for (i, mut frame) in buffer.iter_samples().enumerate() {
            let trim = self.params.trim.smoothed.next() as f64;
            let (l, r) = unsafe { (*frame.get_unchecked_mut(0) as f64, *frame.get_unchecked_mut(1) as f64) };
            self.features.push(0.5 * (l + r));
            self.features.push_side(0.5 * (l - r));

            let mut target = trim;
            if let Some(p0) = pos {
                let p = p0 + i as f64 * beats_per_sample;
                let beat = p.floor() as i64;
                if beat != self.cur_beat {
                    if let Some(f) = self.features.take() {
                        if learning && self.cur_beat >= 0 && (self.cur_beat as usize) < MAX_BEATS {
                            let b = self.cur_beat as usize;
                            for (arr, v) in [&sh.energy, &sh.onsets, &sh.low, &sh.mid, &sh.high, &sh.side].into_iter().zip(f) {
                                arr[b].store(v, Ordering::Relaxed);
                            }
                            sh.learned_beats.fetch_max(b as u32 + 1, Ordering::Relaxed);
                        }
                    }
                    self.cur_beat = beat;
                }
                if follow && curve_len > 0 {
                    // Read a quarter beat ahead so a boost has arrived when the note does.
                    let idx = (p + 0.25).floor() as i64;
                    let moment = if idx >= 0 && idx < curve_len { sh.curve[idx as usize].load(Ordering::Relaxed) as f64 } else { 0.0 };
                    target += static_db * st_amt + moment * mo_amt;
                }
            } else if follow && curve_len > 0 {
                target += static_db * st_amt;
            }

            self.gain_db += (target - self.gain_db) * self.gain_a;
            let g = 10f64.powf(self.gain_db / 20.0);
            let (wl, wr) = if planned || self.width.w != 1.0 { self.width.process(l, r, width, mono_hz) } else { (l, r) };
            unsafe {
                *frame.get_unchecked_mut(0) = (wl * g) as f32;
                *frame.get_unchecked_mut(1) = (wr * g) as f32;
            }

            let st = &mut self.stats;
            st.frames += 1.0;
            st.in_sq += 0.5 * (l * l + r * r);
            st.out_sq += 0.5 * (wl * wl + wr * wr) * g * g;
            st.gain_db += self.gain_db;
            if let Some(p0) = pos {
                let p = p0 + i as f64 * beats_per_sample;
                st.playing += 1.0;
                st.pos_min = st.pos_min.min(p);
                st.pos_max = st.pos_max.max(p);
            }
        }
        self.stats.tempo = t.tempo.unwrap_or(-1.0);
        self.stats.publish(self.fs, &sh.stats);
        sh.live_gain_db.store(self.gain_db as f32, Ordering::Relaxed);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for StrataCylinder {
    const CLAP_ID: &'static str = "com.gtkottman.strata.cylinder";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Per-channel listener and gain stage for the Strata Engine");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Utility];
}

impl Vst3Plugin for StrataCylinder {
    const VST3_CLASS_ID: [u8; 16] = *b"StrataCylinder01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_bursts_but_not_a_drone() {
        let fs = 48000.0;
        let mut f = Features::new(fs);
        // 1 s drone: no attacks after the first.
        for i in 0..48000 {
            f.push(0.3 * (2.0 * std::f64::consts::PI * 110.0 * i as f64 / fs).sin());
        }
        let d = f.take().unwrap()[1];
        assert!(d <= 1.0, "drone onsets {d}");
        // 1 s of 8 plucks (decaying bursts every 125 ms).
        for i in 0..48000 {
            let t = (i % 6000) as f64 / fs;
            f.push(0.5 * (-t * 40.0).exp() * (2.0 * std::f64::consts::PI * 880.0 * i as f64 / fs).sin());
        }
        let onsets = f.take().unwrap()[1];
        assert!((7.0..=8.0).contains(&onsets), "onsets {onsets}");
    }

    #[test]
    fn width_stage_mono_wide_and_bass_to_mono() {
        let fs = 48000.0;
        let side_energy = |width: f64, hz: f64, freq: f64| {
            let mut w = WidthStage::new(fs);
            let (mut se, mut me) = (0.0, 0.0);
            for i in 0..48000 {
                let t = i as f64 / fs;
                // Left and right differ: a source panned left with some stereo spread.
                let x = (2.0 * std::f64::consts::PI * freq * t).sin();
                let (l, r) = w.process(0.8 * x, 0.3 * x, width, hz);
                if i > 24000 {
                    se += (0.5 * (l - r)).powi(2);
                    me += (0.5 * (l + r)).powi(2);
                }
            }
            se / me
        };
        let base = side_energy(1.0, 0.0, 1000.0);
        assert!((side_energy(0.0, 0.0, 1000.0)) < 1e-6, "mono");
        assert!((side_energy(1.5, 0.0, 1000.0) / base - 2.25).abs() < 0.05, "1.5x side = 2.25x energy");
        assert!(side_energy(1.0, 200.0, 60.0) < 0.1 * side_energy(1.0, 0.0, 60.0), "60 Hz side removed below 200 Hz");
        assert!(side_energy(1.0, 200.0, 2000.0) > 0.9 * side_energy(1.0, 0.0, 2000.0), "highs keep their width");
    }

    #[test]
    fn engine_balances_registered_cylinders_and_writes_report() {
        let dir = std::env::temp_dir().join(format!("strata-test-{}", std::process::id()));
        std::env::set_var("USERPROFILE", &dir);
        let a = StrataCylinder::default();
        let b = StrataCylinder::default();
        // a: a lead that plays alone for 6 beats; b: a drone under it the whole time.
        for i in 0..6 {
            a.shared.energy[i].store(0.1, Ordering::Relaxed);
            a.shared.onsets[i].store(2.0, Ordering::Relaxed);
            a.shared.mid[i].store(0.08, Ordering::Relaxed);
            b.shared.energy[i].store(0.1, Ordering::Relaxed);
            b.shared.low[i].store(0.02, Ordering::Relaxed);
            b.shared.mid[i].store(0.08, Ordering::Relaxed);
        }
        for c in [&a, &b] {
            c.shared.learned_beats.store(6, Ordering::Relaxed);
        }
        a.shared.role.store(Role::Lead.index(), Ordering::Relaxed);
        b.shared.role.store(Role::Pad.index(), Ordering::Relaxed);
        a.shared.side[0].store(0.01, Ordering::Relaxed);
        let summary = crate::balance_now();
        assert!(summary.contains("report:"), "{summary}");
        assert_eq!(a.shared.curve_len.load(Ordering::Relaxed), 6);
        assert!(a.shared.curve[2].load(Ordering::Relaxed) > 2.0);
        assert!(b.shared.curve[2].load(Ordering::Relaxed) < -1.0);
        assert_eq!(a.shared.state.lock().unwrap().curve.len(), 6, "plan saved with the project");
        let report = std::fs::read_to_string(dir.join("Documents/Strata/report.json")).unwrap();
        assert!(report.contains("curve_db_per_bar") && report.contains("moments") && report.contains("\"width\""));
        let _ = std::fs::remove_dir_all(dir);
    }
}
