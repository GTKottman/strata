//! Strata: mastering with adjustment layers.

pub mod cylinder;
pub mod dsp;
mod editor;
pub mod orchestra;

use dsp::meter::LoudnessMeter;
use dsp::{pack_order, unpack_order, Chain, Settings, DEFAULT_ORDER};
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Measurements shared with the editor. Loudness in LUFS, peaks in dBTP, reductions in dB.
pub struct Meters {
    pub in_integrated: AtomicF32,
    pub in_short: AtomicF32,
    pub in_true_peak: AtomicF32,
    pub out_integrated: AtomicF32,
    pub out_short: AtomicF32,
    pub out_momentary: AtomicF32,
    pub out_true_peak: AtomicF32,
    pub correlation: AtomicF32,
    pub glue_gr: AtomicF32,
    pub limiter_gr: AtomicF32,
    pub reset: AtomicBool,
    /// Engine pass statistics since the last meter reset: seconds, playing %, input mean-square dB,
    /// first and last song position (beats), initialize() calls, reset() calls.
    pub engine_stats: [AtomicF32; 7],
    /// Result of "Measure latest export".
    pub file_report: Mutex<String>,
    /// Event log for diagnostics (initialize, reset, transport, snapshots), newest last.
    pub log: Mutex<std::collections::VecDeque<String>>,
    /// Text summary of the last balance plan, for the editor.
    pub plan_summary: Mutex<String>,
}

impl Meters {
    /// Never blocks: if the editor holds the lock, the line is dropped.
    pub fn push_log(&self, line: String) {
        if let Ok(mut l) = self.log.try_lock() {
            if l.len() >= 200 {
                l.pop_front();
            }
            l.push_back(line);
        }
    }
}

impl Default for Meters {
    fn default() -> Self {
        let n = || AtomicF32::new(f32::NEG_INFINITY);
        Self {
            in_integrated: n(),
            in_short: n(),
            in_true_peak: n(),
            out_integrated: n(),
            out_short: n(),
            out_momentary: n(),
            out_true_peak: n(),
            correlation: AtomicF32::new(0.0),
            glue_gr: AtomicF32::new(0.0),
            limiter_gr: AtomicF32::new(0.0),
            reset: AtomicBool::new(false),
            engine_stats: std::array::from_fn(|_| AtomicF32::new(0.0)),
            file_report: Mutex::new(String::new()),
            log: Mutex::new(std::collections::VecDeque::new()),
            plan_summary: Mutex::new(String::new()),
        }
    }
}

pub struct Strata {
    params: Arc<StrataParams>,
    meters: Arc<Meters>,
    chain: Chain,
    in_meter: LoudnessMeter,
    out_meter: LoudnessMeter,
    corr: [f64; 3],
    corr_coef: f64,
    st: [f64; 5], // frames, playing frames, input sum of squares, pos min, pos max
    inits: u32,
    resets: u32,
    fs: f64,
    /// Sample rate the meters were built for (they survive re-initialization at the same rate).
    meter_fs: f64,
    /// Frames processed since the plugin was created (log clock).
    clock: f64,
    was_playing: bool,
    next_snapshot: f64,
    last_block: usize,
}

impl Strata {
    fn out_meter_fs(&self) -> f64 {
        self.fs
    }
}

fn db_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min, max })
        .with_unit(" dB")
        .with_step_size(0.1)
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

fn opacity_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

fn freq_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-1.0) })
        .with_unit(" Hz")
        .with_value_to_string(formatters::v2s_f32_rounded(0))
}

fn ms_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-1.0) })
        .with_unit(" ms")
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

#[derive(Params)]
pub struct StrataParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,
    /// Layer order, packed by `dsp::pack_order`.
    #[persist = "layer-order"]
    pub layer_order: Arc<AtomicU32>,

    #[id = "tone_on"]
    pub tone_on: BoolParam,
    #[id = "tone_opacity"]
    pub tone_opacity: FloatParam,
    #[id = "tone_lowcut"]
    pub tone_low_cut: FloatParam,
    #[id = "tone_low"]
    pub tone_low: FloatParam,
    #[id = "tone_mid"]
    pub tone_mid: FloatParam,
    #[id = "tone_presence"]
    pub tone_presence: FloatParam,
    #[id = "tone_air"]
    pub tone_air: FloatParam,

    #[id = "glue_on"]
    pub glue_on: BoolParam,
    #[id = "glue_opacity"]
    pub glue_opacity: FloatParam,
    #[id = "glue_thresh"]
    pub glue_threshold: FloatParam,
    #[id = "glue_ratio"]
    pub glue_ratio: FloatParam,
    #[id = "glue_attack"]
    pub glue_attack: FloatParam,
    #[id = "glue_release"]
    pub glue_release: FloatParam,
    #[id = "glue_makeup"]
    pub glue_makeup: FloatParam,

    #[id = "warm_on"]
    pub warmth_on: BoolParam,
    #[id = "warm_opacity"]
    pub warmth_opacity: FloatParam,
    #[id = "warm_drive"]
    pub warmth_drive: FloatParam,

    #[id = "width_on"]
    pub width_on: BoolParam,
    #[id = "width_opacity"]
    pub width_opacity: FloatParam,
    #[id = "width_amount"]
    pub width_amount: FloatParam,
    #[id = "width_mono"]
    pub width_mono_below: FloatParam,

    #[id = "lim_on"]
    pub limiter_on: BoolParam,
    #[id = "lim_gain"]
    pub limiter_gain: FloatParam,
    #[id = "lim_ceiling"]
    pub limiter_ceiling: FloatParam,
    #[id = "lim_release"]
    pub limiter_release: FloatParam,

    /// Engine: follow the balance plan in every Cylinder.
    #[id = "bal_on"]
    pub balance_on: BoolParam,
    /// Engine: how much of the static (whole-song) level balance to apply.
    #[id = "bal_static"]
    pub balance_static: FloatParam,
    /// Engine: how much of the moment-by-moment balance to apply.
    #[id = "bal_moments"]
    pub balance_moments: FloatParam,

    /// Loudness target used by the editor's "gain to target" readout.
    #[id = "target"]
    pub target_lufs: FloatParam,
}

impl Default for StrataParams {
    fn default() -> Self {
        let d = Settings::default();
        Self {
            editor_state: EguiState::from_size(1040, 760),
            layer_order: Arc::new(AtomicU32::new(pack_order(DEFAULT_ORDER))),

            tone_on: BoolParam::new("Tone On", d.tone.on),
            tone_opacity: opacity_param("Tone Opacity", d.tone.opacity as f32),
            tone_low_cut: freq_param("Low Cut", d.tone.low_cut_hz as f32, 10.0, 150.0),
            tone_low: db_param("Low (110 Hz)", 0.0, -6.0, 6.0),
            tone_mid: db_param("Body (400 Hz)", 0.0, -6.0, 6.0),
            tone_presence: db_param("Presence (3 kHz)", 0.0, -6.0, 6.0),
            tone_air: db_param("Air (10 kHz)", 0.0, -6.0, 6.0),

            glue_on: BoolParam::new("Glue On", d.glue.on),
            glue_opacity: opacity_param("Glue Opacity", d.glue.opacity as f32),
            glue_threshold: db_param("Threshold", d.glue.threshold_db as f32, -40.0, 0.0),
            glue_ratio: FloatParam::new("Ratio", d.glue.ratio as f32, FloatRange::Linear { min: 1.0, max: 8.0 })
                .with_step_size(0.1)
                .with_value_to_string(Arc::new(|v| format!("{v:.1}:1"))),
            glue_attack: ms_param("Attack", d.glue.attack_ms as f32, 0.1, 100.0),
            glue_release: ms_param("Release", d.glue.release_ms as f32, 20.0, 1000.0),
            glue_makeup: db_param("Makeup", 0.0, 0.0, 12.0),

            warmth_on: BoolParam::new("Warmth On", d.warmth.on),
            warmth_opacity: opacity_param("Warmth Opacity", d.warmth.opacity as f32),
            warmth_drive: db_param("Drive", d.warmth.drive_db as f32, 0.0, 24.0),

            width_on: BoolParam::new("Width On", d.width.on),
            width_opacity: opacity_param("Width Opacity", d.width.opacity as f32),
            width_amount: FloatParam::new("Width", 1.0, FloatRange::Linear { min: 0.0, max: 2.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            width_mono_below: freq_param("Mono Below", d.width.mono_below_hz as f32, 20.0, 300.0),

            limiter_on: BoolParam::new("Limiter On", d.limiter.on),
            limiter_gain: db_param("Input Gain", 0.0, 0.0, 24.0),
            limiter_ceiling: db_param("Ceiling (dBTP)", d.limiter.ceiling_db as f32, -3.0, 0.0),
            limiter_release: ms_param("Limiter Release", d.limiter.release_ms as f32, 10.0, 500.0),

            balance_on: BoolParam::new("Balance On", true),
            balance_static: opacity_param("Static Balance", 1.0),
            balance_moments: opacity_param("Moments", 1.0),

            target_lufs: FloatParam::new("Target", -14.0, FloatRange::Linear { min: -24.0, max: -6.0 })
                .with_unit(" LUFS")
                .with_step_size(0.5)
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
        }
    }
}

impl StrataParams {
    fn settings(&self) -> Settings {
        let f = |p: &FloatParam| p.value() as f64;
        Settings {
            order: unpack_order(self.layer_order.load(Ordering::Relaxed)),
            tone: dsp::ToneSettings {
                on: self.tone_on.value(),
                opacity: f(&self.tone_opacity),
                low_cut_hz: f(&self.tone_low_cut),
                low_db: f(&self.tone_low),
                mid_db: f(&self.tone_mid),
                presence_db: f(&self.tone_presence),
                air_db: f(&self.tone_air),
            },
            glue: dsp::GlueSettings {
                on: self.glue_on.value(),
                opacity: f(&self.glue_opacity),
                threshold_db: f(&self.glue_threshold),
                ratio: f(&self.glue_ratio),
                attack_ms: f(&self.glue_attack),
                release_ms: f(&self.glue_release),
                makeup_db: f(&self.glue_makeup),
            },
            warmth: dsp::WarmthSettings { on: self.warmth_on.value(), opacity: f(&self.warmth_opacity), drive_db: f(&self.warmth_drive) },
            width: dsp::WidthSettings {
                on: self.width_on.value(),
                opacity: f(&self.width_opacity),
                width: f(&self.width_amount),
                mono_below_hz: f(&self.width_mono_below),
            },
            limiter: dsp::LimiterSettings {
                on: self.limiter_on.value(),
                gain_db: f(&self.limiter_gain),
                ceiling_db: f(&self.limiter_ceiling),
                release_ms: f(&self.limiter_release),
            },
        }
    }
}

/// Run the balance plan over everything the Cylinders learned, install it, write the report.
pub fn balance_now() -> String {
    let cyls = cylinders_sorted();
    if cyls.is_empty() {
        return "No Cylinders found. Put a Strata Cylinder on each channel.".into();
    }
    let inputs: Vec<orchestra::TrackInput> = cyls
        .iter()
        .map(|c| {
            let n = c.learned_beats.load(Ordering::Relaxed) as usize;
            let read = |v: &Vec<AtomicF32>| v[..n].iter().map(|a| a.load(Ordering::Relaxed)).collect::<Vec<f32>>();
            let role = orchestra::Role::from_index(c.role.load(Ordering::Relaxed));
            orchestra::TrackInput {
                id: c.id,
                label: c.label(),
                role: (role != orchestra::Role::Auto).then_some(role),
                energy: read(&c.energy),
                onsets: read(&c.onsets),
                low: read(&c.low),
                mid: read(&c.mid),
                high: read(&c.high),
            }
        })
        .collect();
    if inputs.iter().all(|t| t.energy.is_empty()) {
        return "Nothing learned yet: start learning, play the song, then balance.".into();
    }
    let bpb = cylinder::BEATS_PER_BAR.load(Ordering::Relaxed) as usize;
    let plan = orchestra::plan(&inputs, bpb);
    for (c, r) in cyls.iter().zip(&plan.tracks) {
        c.set_plan(r.static_db as f32, &r.curve_db);
    }
    let mut text = String::new();
    for r in &plan.tracks {
        let name = if r.label.is_empty() { format!("#{}", r.id) } else { r.label.clone() };
        text += &format!(
            "{name:<14} {:<8}{} {:>6.1} dB loud  static {:+5.1}\n",
            r.role.name(),
            if r.role_guessed { "?" } else { " " },
            r.loudness_db,
            r.static_db
        );
    }
    text += &format!("{} moments over {} bars\n", plan.moments.len(), plan.beats.div_ceil(plan.beats_per_bar));
    for m in plan.moments.iter().take(12) {
        text += &format!(
            "bars {:>3}-{:<3} up: {}  back: {}\n",
            m.from_bar,
            m.to_bar,
            if m.featured.is_empty() { "-".into() } else { m.featured.join(", ") },
            if m.sitting_back.is_empty() { "-".into() } else { m.sitting_back.join(", ") }
        );
    }
    match write_report(&plan) {
        Ok(path) => text += &format!("report: {path}"),
        Err(e) => text += &format!("report not written: {e}"),
    }
    text
}

/// Find the newest .wav under Documents\\Image-Line\\FL Studio\\Projects (two levels deep) and measure it.
pub fn measure_latest_export() -> String {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    let root = std::path::Path::new(&home).join("Documents").join("Image-Line").join("FL Studio").join("Projects");
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    let mut visit = |dir: &std::path::Path| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("wav")) == Some(true) {
                    if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                        if newest.as_ref().is_none_or(|(t, _)| m > *t) {
                            newest = Some((m, p));
                        }
                    }
                }
            }
        }
    };
    visit(&root);
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                visit(&e.path());
            }
        }
    }
    let Some((_, path)) = newest else { return format!("No .wav found under {}", root.display()) };
    match measure_wav(&path) {
        Ok(s) => format!("{}\n{s}", path.display()),
        Err(e) => format!("{}: {e}", path.display()),
    }
}

/// Integrated loudness, loudest short-term, true and sample peak, and length of a WAV file.
pub fn measure_wav(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let mut m = LoudnessMeter::new(spec.sample_rate as f64);
    let mut frame = Vec::with_capacity(ch);
    let mut max_short = f64::NEG_INFINITY;
    let mut n = 0usize;
    let mut push = |v: f64, frame: &mut Vec<f64>, m: &mut LoudnessMeter| {
        frame.push(v);
        if frame.len() == ch {
            let (l, r) = (frame[0], if ch > 1 { frame[1] } else { frame[0] });
            m.process(l, r);
            frame.clear();
            n += 1;
            if n % 4410 == 0 {
                max_short = max_short.max(m.short_term);
            }
        }
    };
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for s in reader.samples::<f32>() {
                push(s? as f64, &mut frame, &mut m);
            }
        }
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f64;
            for s in reader.samples::<i32>() {
                push(s? as f64 * scale, &mut frame, &mut m);
            }
        }
    }
    let db = |x: f64| if x > 0.0 { 20.0 * x.log10() } else { f64::NEG_INFINITY };
    Ok(format!(
        "{:.1} s  integrated {:.1} LUFS  max short-term {:.1} LUFS  true peak {:.2} dBTP  sample peak {:.2} dBFS",
        n as f64 / spec.sample_rate as f64,
        m.integrated(),
        max_short,
        db(m.true_peak_max),
        db(m.sample_peak_max)
    ))
}

fn cylinders_sorted() -> Vec<Arc<cylinder::CylShared>> {
    let mut c = cylinder::cylinders();
    c.sort_by_key(|c| c.id);
    c
}

fn write_report(plan: &orchestra::Plan) -> std::io::Result<String> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    let dir = std::path::Path::new(&home).join("Documents").join("Strata");
    std::fs::create_dir_all(&dir)?;
    let mut value = serde_json::to_value(plan).map_err(std::io::Error::other)?;
    // Curves per bar (mean dB) so the report stays readable.
    if let Some(tracks) = value.get_mut("tracks").and_then(|t| t.as_array_mut()) {
        for (t, r) in tracks.iter_mut().zip(&plan.tracks) {
            let per_bar: Vec<f32> = r
                .curve_db
                .chunks(plan.beats_per_bar.max(1))
                .map(|c| (c.iter().sum::<f32>() / c.len() as f32 * 10.0).round() / 10.0)
                .collect();
            t["curve_db_per_bar"] = serde_json::json!(per_bar);
        }
    }
    let path = dir.join("report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?)?;
    Ok(path.display().to_string())
}

impl Default for Strata {
    fn default() -> Self {
        Self {
            params: Arc::new(StrataParams::default()),
            meters: Arc::new(Meters::default()),
            chain: Chain::new(44100.0),
            in_meter: LoudnessMeter::new(44100.0),
            out_meter: LoudnessMeter::new(44100.0),
            corr: [0.0; 3],
            corr_coef: 0.0,
            st: [0.0, 0.0, 0.0, f64::INFINITY, f64::NEG_INFINITY],
            inits: 0,
            resets: 0,
            fs: 44100.0,
            meter_fs: 0.0,
            clock: 0.0,
            was_playing: false,
            next_snapshot: 0.0,
            last_block: 0,
        }
    }
}

fn to_db(linear: f64) -> f32 {
    if linear <= 0.0 { f32::NEG_INFINITY } else { (20.0 * linear.log10()) as f32 }
}

impl Plugin for Strata {
    const NAME: &'static str = "Strata";
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
        editor::create(self.params.clone(), self.meters.clone())
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let fs = config.sample_rate as f64;
        self.fs = fs;
        self.chain = Chain::new(fs);
        // Hosts re-initialize around offline renders; keep the measurements unless the rate changed.
        let rebuilt = fs != self.meter_fs;
        if rebuilt {
            self.in_meter = LoudnessMeter::new(fs);
            self.out_meter = LoudnessMeter::new(fs);
            self.meter_fs = fs;
        }
        self.meters.push_log(format!(
            "{:8.1}s init mode={:?} fs={} max_block={} meters {}",
            self.clock / fs,
            config.process_mode,
            config.sample_rate,
            config.max_buffer_size,
            if rebuilt { "rebuilt" } else { "kept" }
        ));
        self.corr_coef = (-1.0 / (0.3 * fs)).exp();
        context.set_latency_samples(self.chain.latency() as u32);
        self.inits += 1;
        true
    }

    fn reset(&mut self) {
        self.corr = [0.0; 3];
        self.resets += 1;
        self.meters.push_log(format!("{:8.1}s reset", self.clock / self.fs.max(1.0)));
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        if self.meters.reset.swap(false, Ordering::Relaxed) {
            cylinder::STATS_GEN.fetch_add(1, Ordering::Relaxed);
            self.in_meter.reset();
            self.out_meter.reset();
            self.st = [0.0, 0.0, 0.0, f64::INFINITY, f64::NEG_INFINITY];
            self.inits = 0;
            self.resets = 0;
            self.meters.push_log(format!("{:8.1}s meters reset", self.clock / self.fs));
        }
        let t = context.transport();
        let n = buffer.samples() as f64;
        let now = self.clock / self.fs;
        self.clock += n;
        if t.playing != self.was_playing {
            self.was_playing = t.playing;
            self.meters.push_log(format!(
                "{now:8.1}s {} at beat {:.2} ({} bpm, block {})",
                if t.playing { "play" } else { "stop" },
                t.pos_beats().unwrap_or(-1.0),
                t.tempo.unwrap_or(-1.0),
                buffer.samples()
            ));
        }
        // Only new maximum block sizes: FL renders in alternating 417/418-sample blocks, which would flood the log.
        if buffer.samples() > self.last_block && t.playing {
            self.meters.push_log(format!("{now:8.1}s block size up to {}", buffer.samples()));
            self.last_block = buffer.samples();
        }
        self.st[0] += n;
        if t.playing {
            self.st[1] += n;
            if let Some(p) = t.pos_beats() {
                self.st[3] = self.st[3].min(p);
                self.st[4] = self.st[4].max(p);
            }
        }
        cylinder::BALANCE_ON.store(self.params.balance_on.value(), Ordering::Relaxed);
        cylinder::STATIC_AMOUNT.store(self.params.balance_static.value(), Ordering::Relaxed);
        cylinder::MOMENT_AMOUNT.store(self.params.balance_moments.value(), Ordering::Relaxed);
        let settings = self.params.settings();
        let channels = buffer.as_slice();
        let (left, right) = channels.split_at_mut(1);
        let (left, right) = (&mut *left[0], &mut *right[0]);

        let metering = t.playing;
        for (l, r) in left.iter().zip(right.iter()) {
            if metering {
                self.in_meter.process(*l as f64, *r as f64);
            }
            self.st[2] += 0.5 * (*l as f64 * *l as f64 + *r as f64 * *r as f64);
        }
        let fs = self.out_meter_fs();
        let es = &self.meters.engine_stats;
        let frames = self.st[0].max(1.0);
        let vals = [
            self.st[0] / fs,
            100.0 * self.st[1] / frames,
            if self.st[2] > 0.0 { 10.0 * (self.st[2] / frames).log10() } else { -200.0 },
            if self.st[3].is_finite() { self.st[3] } else { -1.0 },
            if self.st[4].is_finite() { self.st[4] } else { -1.0 },
            self.inits as f64,
            self.resets as f64,
        ];
        for (a, v) in es.iter().zip(vals) {
            a.store(v as f32, Ordering::Relaxed);
        }
        let report = self.chain.process(left, right, &settings);
        let a = self.corr_coef;
        for (l, r) in left.iter().zip(right.iter()) {
            let (l, r) = (*l as f64, *r as f64);
            if !metering {
                continue;
            }
            self.out_meter.process(l, r);
            self.corr[0] = a * self.corr[0] + (1.0 - a) * l * r;
            self.corr[1] = a * self.corr[1] + (1.0 - a) * l * l;
            self.corr[2] = a * self.corr[2] + (1.0 - a) * r * r;
        }

        let m = &self.meters;
        let st = Ordering::Relaxed;
        m.in_integrated.store(self.in_meter.integrated() as f32, st);
        m.in_short.store(self.in_meter.short_term as f32, st);
        m.in_true_peak.store(to_db(self.in_meter.true_peak_max), st);
        m.out_integrated.store(self.out_meter.integrated() as f32, st);
        m.out_short.store(self.out_meter.short_term as f32, st);
        m.out_momentary.store(self.out_meter.momentary as f32, st);
        m.out_true_peak.store(to_db(self.out_meter.true_peak_max), st);
        let denom = (self.corr[1] * self.corr[2]).sqrt();
        m.correlation.store(if denom > 1e-12 { (self.corr[0] / denom) as f32 } else { 0.0 }, st);
        m.glue_gr.store(report.glue_gr_db as f32, st);
        m.limiter_gr.store(report.limiter_gr_db as f32, st);
        if metering {
            self.next_snapshot -= n;
            if self.next_snapshot <= 0.0 {
                self.next_snapshot = 10.0 * self.fs;
                self.meters.push_log(format!(
                    "{now:8.1}s beat {:6.1}  in {:+.1}  out {:+.1} LUFS  out TP {:+.2}",
                    t.pos_beats().unwrap_or(-1.0),
                    self.in_meter.integrated(),
                    self.out_meter.integrated(),
                    to_db(self.out_meter.true_peak_max)
                ));
            }
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for Strata {
    const CLAP_ID: &'static str = "com.gtkottman.strata";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Mastering with adjustment layers and loudness meters");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mastering];
}

impl Vst3Plugin for Strata {
    const VST3_CLASS_ID: [u8; 16] = *b"StrataMasterLyr1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Mastering];
}

nih_export_clap!(Strata, cylinder::StrataCylinder);
nih_export_vst3!(Strata, cylinder::StrataCylinder);
