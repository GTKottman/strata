use super::meter::{LoudnessMeter, TruePeak};
use super::*;

const FS: f64 = 48000.0;

fn sine(freq: f64, amp: f64, phase: f64, n: usize) -> Vec<f32> {
    (0..n).map(|i| (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / FS + phase).sin()) as f32).collect()
}

/// Deterministic noise in -1..1.
fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
        })
        .collect()
}

fn measure(l: &[f32], r: &[f32]) -> LoudnessMeter {
    let mut m = LoudnessMeter::new(FS);
    for (a, b) in l.iter().zip(r) {
        m.process(*a as f64, *b as f64);
    }
    m
}

#[test]
fn ebu_3341_case_1_sine_minus_23() {
    // EBU Tech 3341 test 1: stereo 1 kHz sine at -23 dBFS reads -23.0 LUFS (+-0.1).
    let x = sine(1000.0, db_to_gain(-23.0), 0.0, 20 * 48000);
    let m = measure(&x, &x);
    assert!((m.integrated() + 23.0).abs() < 0.1, "integrated {}", m.integrated());
    assert!((m.momentary + 23.0).abs() < 0.1, "momentary {}", m.momentary);
    assert!((m.short_term + 23.0).abs() < 0.1, "short-term {}", m.short_term);
}

#[test]
fn integrated_gating_ignores_silence() {
    // 10 s at -20 dBFS then 10 s of silence: the silence is gated out.
    let mut x = sine(1000.0, db_to_gain(-20.0), 0.0, 10 * 48000);
    x.extend(std::iter::repeat(0.0).take(10 * 48000));
    let m = measure(&x, &x);
    assert!((m.integrated() + 20.0).abs() < 0.15, "integrated {}", m.integrated());
}

#[test]
fn true_peak_finds_intersample_peak() {
    // fs/4 sine with 45 degree phase: every sample is +-0.707, the waveform peaks at 1.0.
    let x = sine(FS / 4.0, 1.0, std::f64::consts::FRAC_PI_4, 4800);
    let mut tp = TruePeak::default();
    let peak = x.iter().map(|v| tp.push(*v as f64)).fold(0.0, f64::max);
    let sample_peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    assert!((sample_peak - 0.7071).abs() < 0.001);
    assert!((20.0 * peak.log10()).abs() < 0.3, "true peak {} dB", 20.0 * peak.log10());
}

#[test]
fn order_pack_roundtrip_and_invalid() {
    let order = [LayerKind::Width, LayerKind::Tone, LayerKind::Warmth, LayerKind::Glue];
    assert_eq!(unpack_order(pack_order(order)), order);
    assert_eq!(unpack_order(0), DEFAULT_ORDER); // all slots Tone: not a permutation
    assert_eq!(unpack_order(pack_order(DEFAULT_ORDER)), DEFAULT_ORDER);
}

fn all_off() -> Settings {
    let mut s = Settings::default();
    s.tone.on = false;
    s.glue.on = false;
    s.warmth.on = false;
    s.width.on = false;
    s.limiter.on = false;
    s
}

#[test]
fn everything_off_is_a_pure_delay() {
    let mut l = noise(20000, 1);
    let mut r = noise(20000, 2);
    let (l0, r0) = (l.clone(), r.clone());
    let mut chain = Chain::new(FS);
    let lat = chain.latency();
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        chain.process(a, b, &all_off());
    }
    // After the limiter's 20 ms bypass ramp, output = input delayed by the latency.
    for i in 2000..20000 {
        assert!((l[i] - l0[i - lat]).abs() < 1e-6, "left {i}");
        assert!((r[i] - r0[i - lat]).abs() < 1e-6, "right {i}");
    }
}

#[test]
fn limiter_holds_the_true_peak_ceiling() {
    let mut l = noise(5 * 48000, 3);
    let mut r = noise(5 * 48000, 4);
    let mut s = all_off();
    s.limiter = LimiterSettings { on: true, gain_db: 12.0, ceiling_db: -1.0, release_ms: 60.0, auto_release: false, clip_on: false, clip_db: 3.0 };
    let mut chain = Chain::new(FS);
    let mut max_gr: f64 = 0.0;
    for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        max_gr = max_gr.max(chain.process(a, b, &s).limiter_gr_db);
    }
    let m = measure(&l[4800..], &r[4800..]);
    let tp_db = 20.0 * m.true_peak_max.log10();
    assert!(tp_db <= -0.9, "true peak {tp_db} dBTP");
    assert!(max_gr > 6.0, "limiter barely worked: {max_gr} dB");
}

#[test]
fn glue_reduces_by_ratio_above_threshold() {
    // Steady sine peaking 12 dB over the threshold, ratio 4: expect ~9 dB of reduction.
    let mut s = all_off();
    s.glue = GlueSettings { on: true, opacity: 1.0, threshold_db: -24.0, ratio: 4.0, attack_ms: 5.0, release_ms: 50.0, makeup_db: 0.0, sc_hpf_hz: 0.0 };
    let mut l = sine(1000.0, db_to_gain(-12.0), 0.0, 48000);
    let mut r = l.clone();
    let mut chain = Chain::new(FS);
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        chain.process(a, b, &s);
    }
    let tail_peak = l[40000..].iter().fold(0.0f32, |a, v| a.max(v.abs())) as f64;
    let reduction = -12.0 - 20.0 * tail_peak.log10();
    assert!((reduction - 9.0).abs() < 1.0, "reduction {reduction} dB");
}

#[test]
fn width_zero_makes_mono_and_mono_bass_keeps_highs_wide() {
    let mut s = all_off();
    s.width = WidthSettings { on: true, opacity: 1.0, width: 0.0, mono_below_hz: 0.0, linear_phase: false };
    let mut l = noise(48000, 5);
    let mut r = noise(48000, 6);
    let mut chain = Chain::new(FS);
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        chain.process(a, b, &s);
    }
    // The width smoother starts at 1.0; give it 0.2 s.
    for i in 10000..48000 {
        assert!((l[i] - r[i]).abs() < 1e-4, "not mono at {i}");
    }
}

#[test]
fn measure_wav_reads_ebu_reference_tone() {
    let path = std::env::temp_dir().join(format!("strata-ref-{}.wav", std::process::id()));
    let spec = hound::WavSpec { channels: 2, sample_rate: 48000, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for v in sine(1000.0, db_to_gain(-23.0), 0.0, 10 * 48000) {
        let s = (v as f64 * 8388607.0).round() as i32;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
    let report = crate::measure_wav(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(report.contains("integrated -23.0 LUFS"), "{report}");
    assert!(report.starts_with("10.0 s"), "{report}");
}

/// Energy of `x` at `freq` (single-bin DFT over a whole number of cycles).
fn tone_energy(x: &[f64], freq: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let a = 2.0 * std::f64::consts::PI * freq * i as f64 / FS;
        re += v * a.cos();
        im += v * a.sin();
    }
    (re * re + im * im) / (x.len() as f64).powi(2)
}

/// Warmth's wet output with the oversampling delay removed (so it lines up with the input).
fn run_warmth(x: &[f32], s: &WarmthSettings) -> Vec<f64> {
    let mut w = Warmth::new(FS);
    let lat = oversample::LATENCY;
    let mut y: Vec<f64> = x.iter().chain(std::iter::repeat(&0.0).take(lat)).map(|v| w.process([*v as f64, *v as f64], s).1[0]).collect();
    y.drain(..lat);
    y
}

/// Sum of sines across the audible band (nothing near Nyquist, where the oversampling filter rolls off).
fn band_test_signal(n: usize) -> Vec<f32> {
    let parts = [(50.0, 0.2), (440.0, 0.2), (3000.0, 0.15), (11000.0, 0.1), (19000.0, 0.05)];
    (0..n)
        .map(|i| parts.iter().map(|(f, a)| a * (2.0 * std::f64::consts::PI * f * i as f64 / FS).sin()).sum::<f64>() as f32)
        .collect()
}

#[test]
fn bass_grit_off_is_transparent() {
    let x = band_test_signal(48000);
    let s = WarmthSettings { on: true, opacity: 1.0, drive_db: 0.0, grit_db: 0.0 };
    let y = run_warmth(&x, &s);
    // (Skip both ends: the linear-phase filter rings around the signal's abrupt start and stop.)
    let err = x.iter().zip(&y).take(47000).skip(1000).map(|(a, b)| (*a as f64 - b).abs()).fold(0.0, f64::max);
    assert!(err < 1e-4, "max error {err}");
}

#[test]
fn bass_grit_adds_harmonics_to_bass_and_leaves_highs_alone() {
    let s = WarmthSettings { on: true, opacity: 1.0, drive_db: 0.0, grit_db: 12.0 };
    // 60 Hz bass at -6 dBFS: grit puts energy at 120 Hz (even) and 180 Hz (odd).
    let bass = sine(60.0, 0.5, 0.0, 48000);
    let y = run_warmth(&bass, &s);
    let tail = &y[24000..];
    let f1 = tone_energy(tail, 60.0);
    assert!(tone_energy(tail, 120.0) > 1e-3 * f1, "even harmonic {:.2e}", tone_energy(tail, 120.0) / f1);
    assert!(tone_energy(tail, 180.0) > 1e-3 * f1, "odd harmonic {:.2e}", tone_energy(tail, 180.0) / f1);
    // 2 kHz lead: within 0.3 dB of the input, no added harmonics to speak of.
    let lead = sine(2000.0, 0.5, 0.0, 48000);
    let y = run_warmth(&lead, &s);
    let (xi, yi): (Vec<f64>, &[f64]) = (lead[24000..].iter().map(|v| *v as f64).collect(), &y[24000..]);
    let ratio = tone_energy(yi, 2000.0) / tone_energy(&xi, 2000.0);
    assert!((10.0 * ratio.log10()).abs() < 0.3, "2 kHz changed {:.2} dB", 10.0 * ratio.log10());
    assert!(tone_energy(yi, 4000.0) < 1e-4 * tone_energy(yi, 2000.0));
}

#[test]
fn oversampler_round_trip_is_a_whole_sample_delay() {
    let mut os = oversample::Oversampler::new();
    let x = band_test_signal(20000);
    let y: Vec<f64> = x.iter().map(|v| os.process(*v as f64, |u| u)).collect();
    let lat = oversample::LATENCY;
    let err = (1000..20000).map(|i| (y[i] - x[i - lat] as f64).abs()).fold(0.0, f64::max);
    assert!(err < 1e-3, "max error {err}");
}

/// Energy in everything below `below` Hz except the listed tones, relative to the whole signal.
fn energy_outside(x: &[f64], below: f64, keep: &[f64]) -> f64 {
    let n = x.len();
    let total: f64 = x.iter().map(|v| v * v).sum::<f64>() / n as f64;
    let mut other = 0.0;
    let step = FS / n as f64;
    let mut f = step;
    while f < below {
        if keep.iter().all(|k| (f - k).abs() > 3.0 * step) {
            other += 2.0 * tone_energy(x, f);
        }
        f += step;
    }
    other / total
}

#[test]
fn warmth_oversampling_removes_aliasing() {
    // A 9 kHz tone driven hard at 48 kHz: its harmonics 27, 45, 63 kHz... fold back to 21, 3, 15 kHz
    // when saturated at the base rate. Oversampled, almost nothing lands outside 9 kHz.
    let n = 4800; // 0.1 s, whole cycles of 9 kHz and of the 10 Hz analysis grid
    let x = sine(9000.0, 0.9, 0.0, n + 2000);
    let s = WarmthSettings { on: true, opacity: 1.0, drive_db: 18.0, grit_db: 0.0 };
    let y = run_warmth(&x, &s);
    let os = energy_outside(&y[1000..1000 + n], 24000.0, &[9000.0]);
    let naive: Vec<f64> = x[1000..1000 + n].iter().map(|v| saturate(*v as f64, 18.0)).collect();
    let base = energy_outside(&naive, 24000.0, &[9000.0]);
    println!("aliasing: {:.1} dB oversampled, {:.1} dB at the base rate", 10.0 * os.log10(), 10.0 * base.log10());
    assert!(base > 1e-3, "test signal should alias at the base rate: {:.1} dB", 10.0 * base.log10());
    assert!(os < base * 1e-3, "aliasing {:.1} dB oversampled vs {:.1} dB at the base rate", 10.0 * os.log10(), 10.0 * base.log10());
}

#[test]
fn glue_sidechain_hpf_ignores_the_sub() {
    // A loud 40 Hz sub with a quiet 1 kHz part: without the detector high-pass the sub drives the
    // compressor; with it at 100 Hz the compressor barely moves.
    let n = 48000;
    let sub = sine(40.0, db_to_gain(-6.0), 0.0, n);
    let top = sine(1000.0, db_to_gain(-30.0), 0.0, n);
    let x: Vec<f32> = sub.iter().zip(&top).map(|(a, b)| a + b).collect();
    let gr = |hz: f64| {
        let mut s = all_off();
        s.glue = GlueSettings { on: true, opacity: 1.0, threshold_db: -20.0, ratio: 4.0, attack_ms: 10.0, release_ms: 100.0, makeup_db: 0.0, sc_hpf_hz: hz };
        let (mut l, mut r) = (x.clone(), x.clone());
        let mut chain = Chain::new(FS);
        let mut g: f64 = 0.0;
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            g = g.max(chain.process(a, b, &s).glue_gr_db);
        }
        g
    };
    let (off, on) = (gr(0.0), gr(100.0));
    assert!(off > 6.0, "sub should drive the compressor without the high-pass: {off:.1} dB");
    assert!(on < 1.0, "with the high-pass the sub should not: {on:.1} dB");
}

#[test]
fn linear_phase_mono_bass_removes_side_lows_and_keeps_side_highs_exact() {
    let mut s = all_off();
    s.width = WidthSettings { on: true, opacity: 1.0, width: 1.0, mono_below_hz: 120.0, linear_phase: true };
    let n = 4 * 48000;
    // Side-only signal: 40 Hz and 1 kHz, opposite polarity in the two channels.
    let side: Vec<f32> = sine(40.0, 0.3, 0.0, n).iter().zip(sine(1000.0, 0.3, 0.0, n)).map(|(a, b)| a + b).collect();
    let mut l = side.clone();
    let mut r: Vec<f32> = side.iter().map(|v| -v).collect();
    let mut chain = Chain::new(FS);
    let lat = chain.latency_with(&s);
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        chain.process(a, b, &s);
    }
    let out: Vec<f64> = l[2 * 48000..3 * 48000].iter().map(|v| *v as f64).collect();
    let input: Vec<f64> = side[2 * 48000 - lat..3 * 48000 - lat].iter().map(|v| *v as f64).collect();
    let low = tone_energy(&out, 40.0) / tone_energy(&input, 40.0);
    assert!(10.0 * low.log10() < -40.0, "40 Hz side left at {:.1} dB", 10.0 * low.log10());
    // 1 kHz passes with no phase change: sample for sample against the delayed input's 1 kHz part.
    let hi = sine(1000.0, 0.3, 0.0, n);
    let err = (2 * 48000..3 * 48000).map(|i| (l[i] as f64 - hi[i - lat] as f64).abs()).fold(0.0, f64::max);
    assert!(err < 3e-3, "1 kHz side changed by {err}");
}

#[test]
fn matched_eq_does_not_cramp_near_nyquist() {
    // Air +6 dB at 10 kHz and Presence +6 dB at 3 kHz at 44.1 kHz, against the analog response.
    let fs = 44100.0;
    let analog_shelf = |f: f64| {
        let (a, w0, q) = (10f64.powf(6.0 / 40.0), 10000.0, 0.707);
        let x = f / w0;
        let num = (a * (1.0 - a * x * x)).powi(2) + (a * a.sqrt() * x / q).powi(2);
        let den = (a - x * x).powi(2) + (a.sqrt() * x / q).powi(2);
        10.0 * (num / den).log10()
    };
    let analog_peak = |f: f64| {
        let (a, w0, q) = (10f64.powf(6.0 / 40.0), 3000.0, 0.9);
        let x = f / w0;
        let num = (1.0 - x * x).powi(2) + (a * x / q).powi(2);
        let den = (1.0 - x * x).powi(2) + (x / (a * q)).powi(2);
        10.0 * (num / den).log10()
    };
    let worst = |c: Coefs, analog: &dyn Fn(f64) -> f64| (1..=200).map(|i| i as f64 * 100.0).map(|f| (c.magnitude_db(fs, f) - analog(f)).abs()).fold(0.0, f64::max);
    let shelf_m = worst(Coefs::matched_high_shelf(fs, 10000.0, 0.707, 6.0), &analog_shelf);
    let shelf_b = worst(Coefs::high_shelf(fs, 10000.0, 0.707, 6.0), &analog_shelf);
    let peak_m = worst(Coefs::matched_peak(fs, 3000.0, 0.9, 6.0), &analog_peak);
    let peak_b = worst(Coefs::peak(fs, 3000.0, 0.9, 6.0), &analog_peak);
    println!("air shelf: matched {shelf_m:.2} dB, bilinear {shelf_b:.2} dB; presence: matched {peak_m:.2} dB, bilinear {peak_b:.2} dB");
    assert!(shelf_m < 0.25 && peak_m < 0.25, "matched error shelf {shelf_m:.2} peak {peak_m:.2}");
    assert!(shelf_b > shelf_m && peak_b > peak_m);
    // 0 dB is exactly flat.
    let flat = Coefs::matched_high_shelf(fs, 10000.0, 0.707, 0.0);
    assert!((1..=200).all(|i| flat.magnitude_db(fs, i as f64 * 100.0).abs() < 1e-9));
}

/// Limiter gain reduction over a signal, with these limiter settings.
fn limiter_run(x: &[f32], lim: LimiterSettings) -> (Vec<f32>, f64) {
    let mut s = all_off();
    s.limiter = lim;
    let (mut l, mut r) = (x.to_vec(), x.to_vec());
    let mut chain = Chain::new(FS);
    let mut gr: f64 = 0.0;
    for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        gr = gr.max(chain.process(a, b, &s).limiter_gr_db);
    }
    (l, gr)
}

#[test]
fn clipper_takes_load_off_the_limiter_and_still_holds_the_ceiling() {
    // Quiet bed with sharp clicks: the clicks are what make a limiter work hard.
    let n = 3 * 48000;
    let mut x: Vec<f32> = noise(n, 9).iter().map(|v| v * 0.1).collect();
    for k in (2400..n).step_by(4800) {
        x[k] = 0.9;
        x[k + 1] = -0.8;
    }
    let base = LimiterSettings { on: true, gain_db: 6.0, ceiling_db: -1.0, release_ms: 80.0, auto_release: false, clip_on: false, clip_db: 2.0 };
    let (_, gr_plain) = limiter_run(&x, base);
    let (out, gr_clip) = limiter_run(&x, LimiterSettings { clip_on: true, ..base });
    assert!(gr_clip < gr_plain - 2.0, "limiter {gr_plain:.1} dB alone, {gr_clip:.1} dB after the clipper");
    let m = measure(&out[4800..], &out[4800..]);
    assert!(20.0 * m.true_peak_max.log10() <= -0.9, "true peak {:.2}", 20.0 * m.true_peak_max.log10());
}

#[test]
fn auto_release_recovers_fast_after_a_lone_peak_and_slow_after_dense_limiting() {
    let lim = LimiterSettings { on: true, gain_db: 0.0, ceiling_db: -1.0, release_ms: 100.0, auto_release: false, clip_on: false, clip_db: 3.0 };
    let quiet = 0.1f32;
    // Gain of the limited signal at `at` (output over the delayed input).
    let gain_at = |x: &[f32], l: LimiterSettings, at: usize| {
        let mut s = all_off();
        s.limiter = l;
        let (mut a, mut b) = (x.to_vec(), x.to_vec());
        let mut chain = Chain::new(FS);
        let lat = chain.latency_with(&s);
        for (p, q) in a.chunks_mut(256).zip(b.chunks_mut(256)) {
            chain.process(p, q, &s);
        }
        20.0 * (a[at + lat] / x[at]).abs().log10()
    };
    // Lone 2 ms burst 9 dB over the ceiling, then a quiet DC-free square of level 0.1.
    let n = 48000;
    let bed = |i: usize| if (i / 24) % 2 == 0 { quiet } else { -quiet };
    let mut lone: Vec<f32> = (0..n).map(bed).collect();
    (10000..10096).for_each(|i| lone[i] = if bed(i) > 0.0 { 2.5 } else { -2.5 });
    // Same, but after half a second of dense limiting.
    let mut dense: Vec<f32> = (0..n).map(bed).collect();
    (5000..10096).for_each(|i| dense[i] = if bed(i) > 0.0 { 2.5 } else { -2.5 });
    let probe = 10096 + 4800; // 100 ms after the loud part ends
    let auto = LimiterSettings { auto_release: true, ..lim };
    let (fixed_lone, auto_lone) = (gain_at(&lone, lim, probe), gain_at(&lone, auto, probe));
    let (fixed_dense, auto_dense) = (gain_at(&dense, lim, probe), gain_at(&dense, auto, probe));
    println!("100 ms after: lone fixed {fixed_lone:.2} auto {auto_lone:.2} dB; dense fixed {fixed_dense:.2} auto {auto_dense:.2} dB");
    assert!(auto_lone > fixed_lone, "auto should recover faster after a lone peak");
    assert!(auto_dense < fixed_dense, "auto should hold longer after dense limiting");
}

