# Tuning Manners

Manners (`fs_manners`) is the automatic balance equaliser: Tame turns down what covers its neighbours,
Recover lifts what is covered. Unlike the other FireStation engines it is written here in Rust
and has no C++ original, so there is no fidelity test to say when it is right. It is tuned by
ear and against a reference processor instead, with `crates/dsp/examples/manners_tune.rs`.

The model's constants are one struct, `Tuning` in
`crates/dsp/src/plugins/fs/manners/manners_dsp.rs`. The plugin runs `Tuning::default()`; tuning means
changing those defaults.

## Against a reference processor

1. Make the probe (90 s, 48 kHz, 24-bit stereo, synthesised by the tool):

   ```sh
   cargo run --release -p soundcraft-dsp --example manners_tune -- gen probe.wav
   ```

   It prints what each segment asks: balanced noise, tilts, narrow resonances at three
   frequencies, a broad bump, holes, missing highs and lows, a tone, two segments that change
   half way (for the time constants), the same resonance quiet and loud, a harmonic tone, bursts
   and unrelated left and right.

2. Play it through the reference once per setting and bounce each pass. Same sample rate, the
   whole file from its first sample, nothing else on the track, no normalising, no dither. Useful
   passes, everything not named at zero:

   | Pass | Tells us |
   |---|---|
   | Tame 100 % | how hard and how wide Tame cuts |
   | Recover 100 % | the same for Recover, and where it stops lifting |
   | Tame 50 %, Recover 50 % | how the two share the work, and how linear the knobs are |
   | Tame 50 %, Recover 50 %, Bias at each end | what Bias moves |
   | Brighten at each end, then Boost at each end | the tilts |

   Put the settings in the file names.

3. Compare (the knobs each pass was made at go after `@`):

   ```sh
   cargo run --release -p soundcraft-dsp --example manners_tune -- compare probe.wav \
       ref_tame100.wav@tame=100 ref_recover100.wav@recover=100 ref_t50_r50.wav@tame=50,recover=50
   ```

   Per pass and per segment it prints the gain the reference applied in each third octave once
   settled, the gain Manners applies at the same knobs, and the difference; for the two changing
   segments, how long each takes to get 63 % of the way in and out; and the RMS error over every
   moment and band. A reference bounced late (latency not compensated) is lined up first.

4. Fit:

   ```sh
   cargo run --release -p soundcraft-dsp --example manners_tune -- fit probe.wav \
       ref_tame100.wav@tame=100 ref_recover100.wav@recover=100 ref_t50_r50.wav@tame=50,recover=50
   ```

   Coordinate descent over the `Tuning` fields (about three minutes per pass given). It prints
   the fitted `Tuning`; copy the values into `Tuning::default`, then run
   `cargo test -p soundcraft-dsp --lib manners` (the tests hold the behaviour that must survive a
   retune: a pure delay at rest, no flutter on steady noise, the floor left alone, level kept).

The fit only moves constants. Where the table shows a difference no constant can close (a
different shape of cut, behaviour that depends on level), the model itself needs changing; the
per-segment rows are there to show which.

## By ear

```sh
cargo run --release -p soundcraft-dsp --example manners_tune -- render mix.wav out.wav tame=40 recover=30
```

Any `Tuning` field can be given the same way (`slope_up_db=14`), so a constant can be auditioned
without rebuilding.
