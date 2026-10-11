# Strata

A mastering plugin built like a stack of adjustment layers. Four layers you can reorder, each with
on/off and **opacity** (how much of the layer is blended over what is below it), and a true-peak
limiter that always sits last. Loudness meters (EBU R128 / ITU-R BS.1770-4) show input and output,
with a loudness target and a one-click "gain to target".

VST3 and CLAP, 64-bit Windows (built here by cross-compiling from Linux). Linux builds work too.

## Layers

| Layer | What it does | Controls |
|---|---|---|
| Tone | Mastering EQ; Presence and Air are matched to the analog curve up to Nyquist (no cramping) | Low cut, Low shelf 110 Hz, Body 400 Hz, Presence 3 kHz, Air shelf 10 kHz (±6 dB) |
| Glue | Stereo-linked bus compressor, 6 dB soft knee, high-passed detector so kick and bass don't drive it | Threshold, Ratio, Attack, Release, Makeup, Sidechain HPF (100 Hz; 20 = off) |
| Warmth | Soft tanh saturation, 4x oversampled (no aliasing), plus asymmetric grit on the bass band | Drive, Bass grit |
| Width | Mid/side width with bass to mono (off by default) | Width 0–200 %, Mono below, Linear phase |
| Limiter (out) | Lookahead true-peak limiter, 1.5 ms lookahead, program-dependent release, optional clipper before it | Input gain, Ceiling (dBTP), Release, Auto release, Clipper, Clip above ceiling |

Use Up/Down on a layer to move it. Opacity changes and on/off are ramped, so they never click.
Latency is reported to the host: 1.5 ms + 140 samples (lookahead plus the two 4x oversamplers),
plus 42.5 ms while Width's linear phase is on.

- **Auto release**: a lone peak recovers at about half the release time, sustained limiting at about
  three times it, so a dense loud passage doesn't pump.
- **Clipper**: a 4x oversampled soft clipper before the limiter. Peaks are clipped to "Clip above
  ceiling" dB over the ceiling and the limiter takes them the rest of the way. It is distortion: use
  it on dirty genres, leave it off where the kick and sub must stay clean.
- **Linear phase** (Width): bass to mono without shifting the side channel's phase.

## Engine + Cylinders (auto-balance)

The DLL holds two plugins. **Strata** on the Master is the Engine; **Strata Cylinder** goes on every
instrument's mixer track (first insert slot). They find each other inside the host process.

1. Put a Cylinder on each channel. Give it a name and a role (Drums, Bass, Lead, Keys, Pad, Texture),
   or leave Auto and it guesses from what it hears.
2. In the Engine's BALANCE card press **Start learning**, play the whole song from the start, then
   **Stop learning + Balance**.
3. Each Cylinder now plays back its plan, locked to the song position:
   - **Static balance**: whole-song level towards a role template (drums 0, bass -2, lead -1,
     keys -5, pad -9, texture -15 dB relative to the mix), at most +-9 dB.
   - **Moments**, per beat; only Lead and Keys parts can be featured (Pad and Texture are always beds): one part moving alone (a solo or near-solo) +3 dB,
     two moving +1.5 dB each, crowded sections (4+) keep only the busiest forward and pull the rest
     back 1.5 dB; held drones/pads/texture sit back 1.5 dB while something moves. Drums and bass are
     left alone. Smoothed over 3 beats, read a quarter beat ahead.
4. Scale both with the Static balance and Moments sliders, switch the plan off with the BALANCE
   toggle, or per channel with "Follow Engine". Plans are saved with the project.

Each balance writes `Documents\Strata\report.json`: every channel's role, loudness, static offset and
per-bar curve, plus the list of moments (bars, who is featured, who sits back).

Cylinders adjust their own gain; FL's faders still apply after them, so set faders to 0 dB if you
want the Engine to own the balance.

## Meters

Output: integrated, short-term and momentary loudness, true-peak max, correlation, Glue and limiter
gain reduction, and the largest limiter reduction of the pass (a warning shows past 6 dB: get the
loudness from Glue instead). **Compare 1:1** plays the input at the output's loudness, so switching
it on and off judges the processing and not the level; the meters keep measuring the processed output. Input: integrated, short-term, true-peak max. Play the whole song, read the
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
