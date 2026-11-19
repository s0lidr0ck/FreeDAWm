# The synths' factory presets

Burley (the subtractive synth), Cicada (the wavetable synth) and Steeple (the FM synth) each come
with a suite of factory presets: Burley 68, Cicada 66, Steeple 68.

**Designed by measurement; tuned by ear later.** Nothing in this suite was listened to. Every
preset was written from what its controls do and then measured by the preset lab
(`crates/dsp/src/plugins/preset_lab.rs`): rendered over low, middle and high chords, soft and
hard, held and short, with the macros as saved, all down and all up, and the mod wheel off and
full. Expect the presets to be retuned by ear before or after release.

## How the menu is organised

Groups by **type** (Basics, Bass, Lead, Pad, Keys, Pluck, and per synth Brass & Strings, Bells,
Mallets, Brass, Percussion, Texture, Effects), with the **tier** shown as a small heading inside
each group's submenu, in this order:

| Tier | What it is |
|---|---|
| Simple | Honest starting points a beginner can read: Init, Init for Patching (the four macros named and wired to one thing each, at zero), the plain waves, a one-oscillator bass, a basic pad and pluck. Few knobs off their defaults. |
| Expected | The bread and butter people look for by name: basses, leads, pads, keys, organs, clavs, brass, strings, bells, mallets, plucks, noise hits, sweeps and risers. |
| Special | What these synths do that a plain synth does not: named macros that move several things at once (each set to a useful position), velocity, key, mod wheel and per-note random routed musically, Env 2 doing more than the filter, X-Mod and Warp used with taste. |
| Wild | Out-there sound design that still plays: audio-rate chaos held in range, self-evolving drones, glitches, talkers, clangs, risers, falls, impacts, rhythmic gates, broken lo-fi keys. **Macro 1 always takes it from tame to wild.** |

Type groups browse best for finding a sound by job ("a bass"), and the tier headings let a
beginner stay in Simple and Expected. The panel's preset display steps through the presets in
menu order (group, then tier). Hovering a preset in a menu shows its description. The ▶ beside a
preset in the track's Presets menu plays it with its patch without loading it, and the browser's
Presets tab loads a preset with its patch too.

The first presets of each synth (Burley's 19, Cicada's 16, Steeple's 16, made before the suite)
keep their sound exactly: their reference fingerprints are pinned. They were sorted into the
tiers and groups, and are quieter than the suite (about -16 to -34 dBFS on the chord against the
suite's -13); a few of Steeple's touch -0.1 dBFS at full velocity in the high register.

## Measured (the `preset_report` example)

`cargo run --release -p soundcraft-dsp --example preset_report` prints, per preset: loudness of a
held C3 G3 C4 E4 chord at velocity 100 (the loudest 400 ms, RMS), its peak and the worst peak over
every state, the quietest held state, DC, stereo width, attack (to half the loudest), the
release tail of a short note (to -60 dB), the spectral centroid of a held middle C early and late
and how far it moves (octaves), aliasing at C6 and C7 (a 48 kHz render against the same at
192 kHz), the lowest strong partial against the played note (cents) and the render cost against
Init. `--calibrate` prints the level each preset would need for -13 dBFS with every peak under
-1.6 dBFS; `--markdown` prints the table below.

The suite's tests (`crates/dsp/src/plugins/preset_suite.rs`) keep it honest: every preset reads,
names are unique, four tiers of at least eight each, the menu in group and tier order with Init
first; every named macro has a target and changes the sound; the suite's held chord within -17 to
-9 dBFS (or a percussive preset within 4 dB of the peak line), peaks under -1 dBFS in every state
(the first presets under full scale), no held state silent, little DC as saved; pads swell (at
least 100 ms) and ring (at least 400 ms after the key), plucks start within 20 ms, melodic
presets play the key (within 35 cents; bells, percussion, textures, effects and wild presets may
be inharmonic on purpose); wild presets stay bounded for ten seconds with every macro and the
wheel at full; aliasing at C6 and C7 under -30 dB; presets named or described as bright, dark or
moving measurably are. The engine test `every_factory_preset_round_trips_through_the_commands`
loads each through `mix.instrument` and `mix.module_preset`.

Known: audio-rate patches (X-Mod, Warp, FM feedback and 1:1 ratios) can lean the wave to one side
with the macros or the wheel up, leaving some DC (as much as -12 to -20 dBFS in the most extreme
states); none of the synths has a DC blocker on its output yet.

## Every preset

