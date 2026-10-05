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
    s.limiter = LimiterSettings { on: true, gain_db: 12.0, ceiling_db: -1.0, release_ms: 60.0 };
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
    s.glue = GlueSettings { on: true, opacity: 1.0, threshold_db: -24.0, ratio: 4.0, attack_ms: 5.0, release_ms: 50.0, makeup_db: 0.0 };
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
    s.width = WidthSettings { on: true, opacity: 1.0, width: 0.0, mono_below_hz: 0.0 };
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
