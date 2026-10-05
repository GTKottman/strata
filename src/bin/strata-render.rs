//! Offline: run a stereo WAV through the default Strata chain and print loudness before and after.
//! Usage: strata-render IN.wav OUT.wav [limiter-gain-dB] [ceiling-dBTP]

use strata::dsp::meter::LoudnessMeter;
use strata::dsp::{Chain, Settings};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: strata-render IN.wav OUT.wav [limiter-gain-dB] [ceiling-dBTP]");
        std::process::exit(2);
    }
    let mut reader = hound::WavReader::open(&args[1])?;
    let spec = reader.spec();
    if spec.channels != 2 {
        return Err("stereo input only".into());
    }
    let fs = spec.sample_rate as f64;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()?
        }
    };
    let mut left: Vec<f32> = samples.iter().step_by(2).copied().collect();
    let mut right: Vec<f32> = samples.iter().skip(1).step_by(2).copied().collect();

    let mut settings = Settings::default();
    if let Some(g) = args.get(3) {
        settings.limiter.gain_db = g.parse()?;
    }
    if let Some(c) = args.get(4) {
        settings.limiter.ceiling_db = c.parse()?;
    }

    let mut before = LoudnessMeter::new(fs);
    left.iter().zip(&right).for_each(|(l, r)| before.process(*l as f64, *r as f64));

    let mut chain = Chain::new(fs);
    let lat = chain.latency();
    // Pad with the latency so the tail comes out, then drop the leading delay.
    left.extend(std::iter::repeat(0.0).take(lat));
    right.extend(std::iter::repeat(0.0).take(lat));
    for (l, r) in left.chunks_mut(512).zip(right.chunks_mut(512)) {
        chain.process(l, r, &settings);
    }
    let (left, right) = (&left[lat..], &right[lat..]);

    let mut after = LoudnessMeter::new(fs);
    left.iter().zip(right).for_each(|(l, r)| after.process(*l as f64, *r as f64));

    let out_spec = hound::WavSpec { channels: 2, sample_rate: spec.sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut writer = hound::WavWriter::create(&args[2], out_spec)?;
    for (l, r) in left.iter().zip(right) {
        writer.write_sample(*l)?;
        writer.write_sample(*r)?;
    }
    writer.finalize()?;

    let tp = |m: &LoudnessMeter| 20.0 * m.true_peak_max.log10();
    println!("in : {:+.1} LUFS integrated, {:+.2} dBTP", before.integrated(), tp(&before));
    println!("out: {:+.1} LUFS integrated, {:+.2} dBTP", after.integrated(), tp(&after));
    Ok(())
}
