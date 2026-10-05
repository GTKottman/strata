# Strata

A mastering plugin built like a stack of adjustment layers. Four layers you can reorder, each with
on/off and **opacity** (how much of the layer is blended over what is below it), and a true-peak
limiter that always sits last. Loudness meters (EBU R128 / ITU-R BS.1770-4) show input and output,
with a loudness target and a one-click "gain to target".

VST3 and CLAP, 64-bit Windows (built here by cross-compiling from Linux). Linux builds work too.

## Layers

| Layer | What it does | Controls |
|---|---|---|
| Tone | Mastering EQ | Low cut, Low shelf 110 Hz, Body 400 Hz, Presence 3 kHz, Air shelf 10 kHz (±6 dB) |
| Glue | Stereo-linked bus compressor, 6 dB soft knee | Threshold, Ratio, Attack, Release, Makeup |
| Warmth | Soft tanh saturation (unity gain for quiet signals) | Drive |
| Width | Mid/side width with bass to mono | Width 0–200 %, Mono below |
| Limiter (out) | Lookahead true-peak limiter, 1.5 ms lookahead | Input gain, Ceiling (dBTP), Release |

Use ▲/▼ on a layer to move it. Opacity changes and on/off are ramped, so they never click.
Latency is reported to the host (1.5 ms + 12 samples).

## Meters

Output: integrated, short-term and momentary loudness, true-peak max, correlation, Glue and limiter
gain reduction. Input: integrated, short-term, true-peak max. Play the whole song, read the
integrated loudness, then **Apply to limiter input gain** to reach the target. Reset meters before
each full pass.

## Install (Windows)

Download `Strata-windows.zip` from the latest release and unzip it.

- VST3: copy the `Strata.vst3` folder to `C:\Program Files\Common Files\VST3\`
- CLAP: copy `Strata.clap` to `C:\Program Files\Common Files\CLAP\`

In FL Studio: Options > Manage plugins > Find more plugins (or Start scan), then add **Strata** to the
Master mixer track.

## Build

```sh
cargo test --lib                # DSP tests: EBU 3341 loudness, true peak, limiter ceiling, bypass
./build-windows.sh              # needs: rustup target add x86_64-pc-windows-msvc; cargo install cargo-xwin
cargo run --release --bin strata-render -- in.wav out.wav [limiter-gain-dB] [ceiling-dBTP]
```

`strata-render` runs the default chain offline and prints loudness before and after.

## License

GPL-3.0-or-later (VST3 support comes through nih-plug's GPLv3 VST3 bindings).
