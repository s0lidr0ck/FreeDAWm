# The synths' factory presets

Burley (the subtractive synth), Cicada (the wavetable synth) and Steeple (the FM synth) each come
with a suite of factory presets: Burley 81, Cicada 79, Steeple 81. Thirteen of each are the
**showcase set** (the **Patched** group, below): presets whose whole point is the patching.

**Designed by measurement; tuned by ear later.** Nothing in this suite was listened to. Every
preset was written from what its controls do and then measured by the preset lab
(`crates/dsp/src/plugins/preset_lab.rs`): rendered over low, middle and high chords, soft and
hard, held and short, with the macros as saved, all down and all up, and the mod wheel off and
full. Expect the presets to be retuned by ear before or after release.

## How the menu is organised

Groups by **type** (Basics, Bass, Lead, Pad, Keys, Pluck, and per synth Brass & Strings, Bells,
Mallets, Brass, Percussion, Texture, Effects), plus **Patched** (the showcase set, second in
every synth's menu, right after Basics), with the **tier** shown as a small heading inside each
group's submenu, in this order:

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

## The showcase set: Patched

Before this set, only a handful of presets showed what the modulation matrix can do. Counted on
2026-11-19 (`preset_report --patching`, and `--slots` for what each cable does when its source
moves, scored on the same clearly-heard line as the macros, below):

| Synth | Presets | With any cable (beyond the macros) | With 3 or more cables | From 3 or more sources | Cables | Cables clearly heard | Presets with 3 cables clearly heard | ...from 3 sources |
|---|---|---|---|---|---|---|---|---|
| Burley | 68 | 27 | 12 | 6 | 61 | 41 | 7 | 3 |
| Cicada | 66 | 24 | 10 | 5 | 60 | 33 | 3 | 0 |
| Steeple | 68 | 24 | 13 | 7 | 57 | 31 | 4 | 1 |

Cables by source then: Burley Amp Env 1, Env 2 10, LFO 6, Velocity 9, Wheel 16, Key 4, Random 15;
Cicada Env 2 13, LFO 9, Velocity 7, Wheel 14, Key 4, Random 13; Steeple Env 2 15, LFO 3, Velocity
10, Wheel 12, Key 4, Random 13. By destination they went mostly to the obvious places: Burley's
cutoff (18 of 61), Cicada's position and Warp (39 of 60), Steeple's modulator levels (24 of 57).
The key and the LFO on anything but the filter were barely used.

The **Patched** group adds thirteen presets per synth (nine in Special: two basses, two leads,
keys or a pluck, pads, brass or strings; four in Wild: evolving and wild textures, Macro 1 taking
each from tame to wild). In every one several sources do something audible at once, and the
patches are the kind a player finds by playing: velocity on more than loudness (the vibrato's
depth, the cross-modulation, how far Env 2 sweeps, the drawbars, the feedback), the mod wheel
bringing in wobble, tremolo, sync wobble or an octave climb, the key making low notes snap harder
or high notes thinner, per-note random voicing every note a little differently, Env 2 on unusual
places (the noise, the sub, the resonance, the vibrato's depth so it blooms in late, the level
for a brass swell, the Warp for an FM key click), the LFO on the oscillator mix, the stereo
spread, the table position, a fixed operator's frequency. Several of them modulate modulation:
velocity or Env 2 on the LFO's depth, velocity on Env 2's depth, the wheel on the LFO's depth and
rate. Every preset has four named macros, and each macro moves several things together, often a
cable's depth (Wobble sets the LFO's depth before the wheel adds its own; Bloom how deep the late
vibrato goes). Each description says in plain words what to play to hear it: play harder, push
the wheel, hold a note, play high or low.

**What holds them** (`crates/dsp/src/plugins/preset_suite.rs`): every bar the rest of the suite
meets (the held chord's level window or a percussive preset's peak line, peaks under -1 dBFS in
every macro and wheel state, every named macro clearly heard, little DC, aliasing at C6 and C7
under -30 dB, melodic Special presets playing the key), and
`every_showcase_preset_has_three_sources_clearly_heard`: twelve to sixteen per synth, all in the
Patched group and only they, Special or Wild, four named macros each, and at least three cables
that are each clearly heard when their source moves, from at least three different sources. A
cable is measured by `preset_lab::slot_effect`: the preset against itself with that cable taken
out, the source at the end of the travel a player reaches (every note at velocity 127; the wheel
at full; the music two octaves up and two octaves down for the key; eight notes in turn, each a
new value, for random; Env 2, the amp envelope and the LFO move by themselves), scored by the same
`preset_lab::audible` as the macros (brightness by 25 %, a band or the level by 6 dB, a
modulation's depth or rate by a factor of two, a semitone, the width by 30 %, an envelope time by
a factor of two). The first presets' fingerprints are untouched and the rest of the suite sounds
as before.

**Measured on 2026-11-19, after:** of the showcase set's 193 cables, 192 cross the line (the one
under it, Cicada's Storm Shutter's random on the position, scores 0.98; that preset has four
others); every preset has at least four cables heard from at least four different sources (28 of
the 39 from five). Median cable score 2.1 (Burley), 2.3 (Cicada), 3.4 (Steeple); every macro at
least 1.08, medians 3.2, 2.8 and 3.7. Counting the
whole menu now (`--patching`): Burley 40 presets with a cable, 25 with three or more, 19 from
three or more sources; Cicada 37, 23, 18; Steeple 37, 26, 20. The showcase's held chords sit at
-18.8 dBFS on average for Burley (-26.0 to -12.8, its basses and plucks held by their peaks with
velocity and the macros up), -16.0 for Cicada and -14.9 for Steeple; worst peak -1.2 dBFS; worst
aliasing -34.0 dB (Burley), -35.2 (Cicada) and -54.8 (Steeple); at most 2.7 times Init's cost.

**Hearing them:** `cargo run --release -p soundcraft-dsp --example preset_audition -- <dir>
<synth id> "<name>"...` plays a showcase preset's phrase (a bass riff, a lead line, chords, a
broken chord or hits, from its description) soft, then hard, then with the wheel swept up and back
down, then with Macro 1 at 0 % and at 100 %.

Each showcase preset's cables and macros as measured (1.0 is the clearly-heard line; the full
descriptions, with what to play, are in the table of every preset below):

**Burley**

| Preset | Tier | Cables: source → knob (clearly-heard score when the source moves) | Macros (score) |
|---|---|---|---|
| Hush Puppy Bass | Special | Velocity → cutoff (3.9), Random → cutoff (12.0), Key → filter_env_amount (2.1), Wheel → lfo_cutoff (3.1), Wheel → lfo_rate (2.6) | Bite (12.0), Growl (12.0), Weight (2.6), Wobble (2.6) |
| Gravel Road Bass | Special | Velocity → unison_detune (1.3), Random → osc2_detune (1.0), Env 2 → sub_level (1.4), Wheel → resonance (1.2), Key → cutoff (3.1) | Snarl (2.1), Spread (1.9), Dark (4.6), Chew (3.7) |
| Whippoorwill Lead | Special | Env 2 → noise_level (1.1), Velocity → lfo_pitch (3.9), Key → cutoff (3.9), Wheel → osc2_semitones (1.6), Random → cutoff (1.1) | Breath (1.9), Bite (3.2), Vibrato (1.5), Air (3.1) |
| Front Porch Fiddle | Special | Env 2 → lfo_pitch (1.3), Velocity → cutoff (7.2), Random → osc2_detune (2.0), Wheel → xmod_pitch (1.1), Key → resonance (1.0) | Bow (7.4), Bloom (1.4), Rasp (1.5), Body (17.0) |
| Roadhouse Keys | Special | Velocity → cutoff (5.0), Random → osc_mix (10.1), Key → osc_mix (10.1), Wheel → lfo_amp (1.4) | Bright (3.2), Wood (12.0), Tremolo (2.1), Chime (9.2) |
| Cotton Gin Pluck | Special | Velocity → filter_env_amount (2.2), Random → cutoff (2.2), Key → filter_env_amount (1.7), Wheel → resonance (1.0) | Snap (1.7), Ring (3.4), Body (13.1), Spread (3.3) |
| Magnolia Pad | Special | LFO → osc_mix (1.0), Random → cutoff (2.0), Velocity → filter_env_amount (3.2), Wheel → lfo_cutoff (1.4), Key → unison_detune (1.4) | Bloom (2.2), Drift (1.5), Shimmer (2.8), Sweep (2.6) |
| Bayou Breath Pad | Special | Env 2 → resonance (1.7), Key → unison_spread (1.4), LFO → unison_spread (1.3), Wheel → cutoff (11.6), Random → cutoff (3.4) | Air (7.2), Whistle (4.0), Sway (3.2), Open (10.8) |
| Revival Brass | Special | Velocity → filter_env_amount (3.6), Env 2 → level (2.6), Random → osc2_detune (1.4), Wheel → lfo_pitch (2.5), Velocity → resonance (1.8) | Blare (6.1), Growl (12.0), Section (2.9), Vibrato (1.9) |
| Haint Choir | Wild | LFO → osc2_semitones (2.9), Random → pitch (1.0), Env 2 → unison_spread (1.8), Wheel → lfo_rate (4.9), LFO → cutoff (2.0) | Haunt (8.8), Wander (2.6), Snarl (1.2), Space (2.4) |
| Gator Growl | Wild | Velocity → xmod_pitch (1.2), Key → cutoff (3.9), LFO → xmod_cutoff (2.6), Wheel → resonance (1.4), Random → lfo_cutoff (1.7) | Jaws (2.4), Chew (3.2), Mud (5.0), Bite (2.0) |
| Firefly Field | Wild | Random → pitch (3.5), Random → cutoff (3.5), LFO → level (3.4), Env 2 → pitch (2.7), Wheel → lfo_rate (2.7) | Swarm (3.2), Flicker (6.2), Glow (3.6), Fall (5.7) |
| Tent Revival Riser | Wild | Env 2 → pitch (1.6), Velocity → filter_env_amount (3.0), Random → osc2_detune (2.0), Wheel → lfo_cutoff (4.0), LFO → pitch (3.6) | Rapture (4.8), Speed (3.6), Choir (2.5), Grit (1.6) |

**Cicada**

| Preset | Tier | Cables: source → knob (clearly-heard score when the source moves) | Macros (score) |
|---|---|---|---|
| Crawdad Bass | Special | Velocity → position (1.4), Env 2 → position (2.2), Key → cutoff (3.4), Wheel → lfo_position (1.3), Random → position (2.0) | Pulse (2.8), Fold (1.4), Sub (7.2), Wobble (2.2) |
| Mudcat Bass | Special | Env 2 → position (1.1), Velocity → position (2.3), Random → position (2.0), Key → cutoff (2.7), Wheel → lfo_position (1.3) | Sync (2.5), Sweep (2.0), Growl (2.2), Weight (2.6) |
| Sync Holler Lead | Special | Velocity → warp_amount (2.3), Env 2 → warp_amount (2.4), Key → position (1.6), Wheel → lfo_position (1.8), Random → position (2.0) | Tear (4.5), Fall (3.2), Shake (4.4), Air (5.4) |
| Mockingbird Lead | Special | Env 2 → lfo_pitch (1.8), Velocity → position (4.0), Random → warp_amount (1.0), Wheel → warp_amount (1.2), Key → cutoff (2.2) | Song (4.3), Bloom (1.2), Edge (1.4), Width (3.3) |
| Tin Roof Keys | Special | Velocity → position (4.7), Env 2 → warp_amount (3.3), Key → position (2.3), Wheel → lfo_amp (3.0), Random → cutoff (2.6) | Stops (3.4), Click (8.9), Tremolo (3.0), Drift (2.6) |
| Dewdrop Keys | Special | Velocity → warp_amount (1.3), Random → position (1.9), Env 2 → position (1.5), Wheel → lfo_position (1.4), Key → cutoff (2.4) | Shine (2.6), Comb (1.4), Ring (1.3), Motion (1.4) |
| Spanish Moss Pad | Special | LFO → position (1.4), Random → position (6.2), Velocity → cutoff (5.9), Wheel → lfo_rate (10.5), Wheel → lfo_cutoff (2.1), Env 2 → warp_amount (1.1) | Sway (2.0), Bloom (4.6), Glow (2.2), Width (3.1) |
| Low Country Choir | Special | Key → position (3.4), LFO → position (2.4), Random → osc_fine (1.6), Wheel → lfo_position (1.6), Velocity → cutoff (4.1) | Vowel (1.8), Drift (2.1), Choir (2.4), Breath (8.8) |
| Sunbelt Brass | Special | Velocity → env2_cutoff (3.7), Random → unison_detune (1.6), Key → cutoff (3.6), Wheel → lfo_pitch (3.7), Env 2 → cutoff (3.0) | Blare (1.5), Swell (4.0), Section (2.7), Vibrato (3.1) |
| Junebug Swarm | Wild | Random → osc_pitch (5.7), LFO → warp_amount (1.2), Env 2 → osc_pitch (1.2), Wheel → lfo_rate (3.0), Velocity → cutoff (3.5) | Swarm (4.5), Rattle (1.2), Grit (1.5), Buzz (1.1) |
| Storm Shutter | Wild | LFO → position (12.0), Velocity → warp_amount (1.8), Key → warp_amount (3.7), Random → position (1.0), Wheel → lfo_rate (2.3) | Storm (1.7), Bang (12.0), Slam (4.5), Gust (3.8) |
| Moonshine Still | Wild | Env 2 → warp_amount (1.2), LFO → osc_fine (1.2), Random → position (6.7), Wheel → cutoff (10.7), Key → cutoff (4.2) | Proof (3.7), Bubble (1.8), Heat (11.3), Hiss (5.6) |
| Holy Roller | Wild | LFO → warp_amount (1.8), Wheel → osc_pitch (12.6), Random → warp_amount (1.6), Velocity → position (5.2), Key → cutoff (12.0) | Spirit (3.7), Rock (14.1), Shout (1.3), Spread (5.1) |

**Steeple**

| Preset | Tier | Cables: source → knob (clearly-heard score when the source moves) | Macros (score) |
|---|---|---|---|
| Pecan Bass | Special | Velocity → op2_level (3.8), Key → op2_level (3.7), Env 2 → op3_level (2.6), Wheel → lfo_mod (1.3), Random → op2_detune (1.3) | Bark (3.2), Click (18.4), Growl (2.2), Wobble (1.9) |
| Rattlesnake Bass | Special | Velocity → op2_feedback (10.9), Env 2 → op2_fine (1.7), Key → op2_feedback (7.0), Wheel → op2_level (12.0), Random → op2_level (15.7) | Rattle (13.9), Snap (3.5), Weight (2.2), Edge (16.7) |
| Steam Whistle Lead | Special | Env 2 → lfo_pitch (1.8), Velocity → op2_feedback (1.4), Key → op2_level (5.5), Wheel → op2_level (17.8), Random → op1_detune (1.3) | Reed (18.8), Bloom (1.2), Steam (15.2), Flutter (1.4) |
| Riverboat Calliope | Special | Env 2 → op4_level (2.3), Velocity → op2_level (2.3), Key → op3_level (1.5), Random → op4_level (3.9), Wheel → lfo_amp (2.5) | Pipes (1.5), Chiff (6.1), Tremble (2.9), Reed (2.4) |
| Sanctified Tine | Special | Velocity → op2_level (8.2), Velocity → op4_level (4.8), Key → op2_level (3.5), Random → op4_level (4.3), Wheel → lfo_amp (2.1) | Bark (3.7), Tine (6.8), Tremolo (2.4), Warm (6.2) |
| Hammered Dulcimer | Special | Velocity → op2_level (2.1), Random → op3_detune (1.1), Env 2 → op4_level (3.2), Wheel → lfo_amp (1.9), Key → op2_level (1.1) | Strike (2.5), Shimmer (3.9), Course (2.2), Sway (1.1) |
| Cumberland Pad | Special | LFO → op4_level (3.4), Random → op2_detune (1.0), Velocity → op4_level (4.9), Wheel → op4_level (6.3) | Breath (2.6), Glow (10.0), Drift (11.8), Air (16.3) |
| Blue Ridge Strings | Special | Env 2 → op2_level (1.0), Env 2 → op4_level (13.1), Velocity → op4_feedback (15.8), LFO → op3_detune (1.0), Wheel → lfo_pitch (3.7) | Bow (5.1), Rosin (2.5), Section (1.6), Vibrato (3.2) |
| Second Line Brass | Special | Env 2 → op4_level (5.5), Velocity → op4_feedback (6.7), Random → op2_detune (4.2), Wheel → lfo_pitch (3.0), Key → op4_level (12.0) | Blare (12.7), Growl (3.1), Spread (1.6), Vibrato (2.6) |
| Twister Bells | Wild | Random → op2_fine (1.0), LFO → op2_level (1.9), Env 2 → pitch (1.3), Wheel → lfo_rate (2.9), Velocity → op4_level (5.0) | Storm (1.2), Swing (2.0), Clang (4.1), Fall (12.7) |
| Junkyard Choir | Wild | Random → op4_freq (15.5), LFO → op4_freq (6.8), Velocity → op4_level (5.5), Wheel → op4_level (3.0), Key → op2_level (1.7) | Scrap (4.3), Bend (2.3), Clank (5.0), Rust (5.1) |
| Radio Preacher | Wild | LFO → op2_fine (1.5), Random → op2_feedback (11.0), Velocity → op2_level (11.9), Wheel → op3_level (14.8), Key → op3_level (5.8) | Sermon (15.2), Talk (1.2), Static (13.7), Distance (9.2) |
| Hail on the Tin | Wild | Random → pitch (10.3), Random → op2_level (2.5), Env 2 → op2_level (2.6), Key → op4_level (1.4), Wheel → lfo_amp (3.4) | Storm (6.6), Rattle (2.8), Ring (8.9), Pitch (11.1) |

## Measured (the `preset_report` example)

`cargo run --release -p soundcraft-dsp --example preset_report` prints, per preset: loudness of a
held C3 G3 C4 E4 chord at velocity 100 (the loudest 400 ms, RMS), its peak and the worst peak over
every state, the quietest held state, DC, stereo width, attack (to half the loudest), the
release tail of a short note (to -60 dB), the spectral centroid of a held middle C early and late
and how far it moves (octaves), aliasing at C6 and C7 (a 48 kHz render against the same at
192 kHz), the lowest strong partial against the played note (cents) and the render cost against
Init. `--calibrate` prints the level each preset would need for -13 dBFS with every peak under
-1.6 dBFS; `--markdown` prints the table below; `--patching` counts the presets with cables by
source and destination; `--slots` scores what each cable does when its source moves; `--macros`
each macro. A filter after the synth id matches part of a name or a whole group (`patched`).

The suite's tests (`crates/dsp/src/plugins/preset_suite.rs`) keep it honest: every preset reads,
names are unique, four tiers of at least eight each, the menu in group and tier order with Init
first; every named macro has a target and is clearly heard (below); the suite's held chord within -17 to
-9 dBFS (or a percussive preset within 4 dB of the peak line), peaks under -1 dBFS in every state
(the first presets under full scale), no held state silent, little DC as saved; pads swell (at
least 100 ms) and ring (at least 400 ms after the key), plucks start within 20 ms, melodic
presets play the key (within 35 cents; bells, percussion, textures, effects and wild presets may
be inharmonic on purpose); wild presets stay bounded for ten seconds with every macro and the
wheel at full; aliasing at C6 and C7 under -30 dB; presets named or described as bright, dark or
moving measurably are. The engine test `every_factory_preset_round_trips_through_the_commands`
loads each through `mix.instrument` and `mix.module_preset`.

### Macros: clearly heard

Each named macro is turned from 0 to 100 % (the preset's other macros as saved) on a held chord
(C3 G3 C4 E4, 2.5 s), a short middle C (150 ms, for plucks) and a held middle C (for pitch), and
it must cross at least one of these lines (`preset_lab::audible`, the test
`every_named_macro_is_clearly_heard`):

| What moves | Line | Why there |
|---|---|---|
| Brightness (spectral centroid) | 25 % | about a third of an octave, several times what is first noticed |
| Level in an octave band (bands within 30 dB of the loudest), or overall | 6 dB | twice the 3 dB that reads as "a little louder" |
| A modulation's depth: tremolo, filter sweep, timbre motion (octave bands moving), vibrato | factor of 2, the deeper at least 1.5 dB / 0.1 octave / 2 dB / 8 cents | less than that is hardly heard as movement |
| A modulation's rate | factor of 2, both deep enough to hear | |
| Pitch of a held note | a semitone | |
| Stereo width (side against mid) | 30 %, the wider at least 0.15 | narrower images sound mono either way |
| Attack, or the tail after a short note | factor of 2, the longer at least 50 / 150 ms | |

The score in the table below is the largest of these as a multiple of its line (1.0 is "clearly
heard"; the reworked macros were taken to at least 1.5).

Measured on 2026-11-19 against the previous threshold (any difference at all), 29 of the 160
named macros fell under the line and 11 more were under 1.5 (Burley 9 and 5, Cicada 11 and 2,
Steeple 9 and 4): most drove one knob a little, or a knob on the flat part of what it does (a
filter on a sine or triangle, X-Mod to pitch on a saw, a modulator whose operator is silent, a
position already pushed to the end by velocity and Env 2), or promised what a macro cannot reach
(an envelope's decay). No bug was behind it: a macro turned during a held note sounds as turning
the knob itself by the same share of its travel, for every destination of the three synths
(`a_macro_turned_on_a_held_note_sounds_like_turning_the_knob`). 45 macros were reworked: wider
ranges, more targets moving the same way (Bright opening the filter and adding X-Mod, Mouth moving
the vowel and the filter, Shimmer detuning and adding a slow tremolo), levels set against
resonance so the peaks stay under -1 dBFS, and ranges set so the preset sounds as before at its
saved macro positions (the range runs both ways from there). Now every macro scores at least 1.57
(Burley's lowest 1.57, median 3.6; Cicada 1.59, 3.6; Steeple 1.63, 4.4). Five macros were
re-described to say what they now do (Steeple's Koto Garden Bend and Ring, Cowbell
Kit Ring, Init for Patching Bite; Cicada's Init for Patching Tone). Four presets are 1 to 2 dB
quieter (Burley's Creek Haze Pad, Sunday Keys and Dew Pluck, Steeple's Steeple Bells) so their wider macros
stay under the peak line.

A source dropped on a knob now moves it by 30 % of its travel (it was 25 %; two semitones on a
pitch; an envelope, velocity or the wheel turn a knob near its top down instead of past its end),
and a macro dragged on drives half the travel, up or (near the top) down. Surprise keeps the level
within 6 dB (it was 3) and uses at least half of each limit.

Known: audio-rate patches (X-Mod, Warp, FM feedback and 1:1 ratios) can lean the wave to one side
with the macros or the wheel up, leaving some DC (as much as -12 to -20 dBFS in the most extreme
states); none of the synths has a DC blocker on its output yet.

### The numbers (full report, 2026-11-19)

| Synth | Presets (Simple / Expected / Special / Wild) | Suite's held chord, mean (range) | Worst peak, any state | Worst aliasing (C6, C7) | Costliest against Init |
|---|---|---|---|---|---|
| Burley | 68 (11 / 32 / 15 / 10) | -15.4 dBFS (-25.5 to -12.8) | -1.3 dBFS | -33.7 dB | 4.4 x |
| Cicada | 66 (9 / 33 / 14 / 10) | -15.1 dBFS (-22.2 to -12.8) | -1.3 dBFS | -33.0 dB | 2.4 x |
| Steeple | 68 (8 / 38 / 13 / 9) | -15.1 dBFS (-23.5 to -12.5) | -1.4 dBFS | -34.4 dB | 2.4 x |

These are the presets before the showcase set; the Patched group's own numbers are in its
section above. The suite's presets that sit under -16 dBFS are percussive (plucks, hits, impacts, bells) or
lift with their macros up: their level is held by the peak line, not by the chord. The first
presets (unchanged): Burley -22.0 dBFS on average, Cicada -20.3, Steeple -16.7 (six of Steeple's
peak between -1.4 and -0.1 dBFS).

## Every preset

| Synth | Tier | Group | Preset | What it is for | Macros (clearly-heard score) |
| Burley | Simple | Basics | Init | The synth as it starts, at the suite's level: two saws a little apart through a soft low-pass. Start here. | – |
| Burley | Simple | Basics | Init for Patching | Init with the four macros named and wired to one thing each, all at zero so it sounds like Init: Tone opens the filter, Motion brings in the LFO's sweep, Grit the resonance, Detune pulls the two saws apart. | Tone (5.3), Motion (3.1), Grit (4.0), Detune (2.4) |
| Burley | Simple | Basics | Plain Saw | One saw, the filter wide open and no envelope movement: the raw sound to shape from scratch. | – |
| Burley | Simple | Basics | Plain Square | One square, the filter wide open: hollow, woody, the odd harmonics only. | – |
| Burley | Simple | Basics | Plain Sine | One sine: the pure tone, for sub lines, whistles and test notes. | – |
| Burley | Simple | Basics | Plain Triangle | One triangle: soft and flute-like, a few quiet odd harmonics over the note. | – |
| Burley | Special | Patched | Hush Puppy Bass | A round square-and-saw bass that answers your hands. Play harder and it bites (velocity opens the filter); every note's tone lands a little differently, like fingers on strings (random on the cutoff); low notes thump with more envelope than high ones (key); push the wheel for a wobble that also speeds up. Bite sharpens the filter, Growl the cross-modulation, Weight the sub (and less of the saw), Wobble the wobble's depth before the wheel. | Bite (12.0), Growl (12.0), Weight (2.6), Wobble (2.7) |
| Burley | Special | Patched | Gravel Road Bass | A thick three-voice bass that changes under your fingers: hard notes spread the voices wider apart (velocity), each note's second saw lands a little differently out of tune (random), every note starts with a sub-octave thump that falls away (Env 2 on the sub), and the wheel winds up the resonance. Snarl adds a cross-modulated snarl, Spread the stereo, Dark closes the filter, Chew adds a slow chewing sweep. | Snarl (2.1), Spread (2.0), Dark (4.6), Chew (3.7) |
| Burley | Special | Patched | Whippoorwill Lead | A singing lead with a breath at the start of each note (Env 2 on the noise). Play harder for more vibrato (velocity sets its depth); higher notes come out brighter (key); push the wheel and the second oscillator climbs a full octave above the first. Breath sets the chiff, Bite the resonance and sweep, Vibrato the vibrato under every note, Air the noise that stays. | Breath (1.9), Bite (3.2), Vibrato (1.5), Air (3.1) |
| Burley | Special | Patched | Front Porch Fiddle | A bowed lead: hold a note and the vibrato blooms in after a moment (Env 2 slowly raises the vibrato's depth), the bow pressing harder the harder you play (velocity on the filter's swell), each note bowed a little differently (random on the detune), and the wheel adds a rosin rasp of cross-modulation. Bow sets the bite, Bloom how deep the late vibrato goes, Rasp the grit, Body the sub-octave under it. | Bow (7.4), Bloom (1.4), Rasp (1.5), Body (17.0) |
| Burley | Special | Patched | Roadhouse Keys | An upright-ish piano tone made of patches: play harder and it gets brighter and louder; every key is voiced a touch differently, like an old roadhouse upright (random on how much of the octave above it has); the low notes are woodier and the high notes chime (key on the mix); push the wheel for a tremolo. Bright opens the filter, Wood adds a sub-octave body, Tremolo the wheel's depth before the wheel, Chime the octave on top. | Bright (3.2), Wood (12.0), Tremolo (2.1), Chime (9.2) |
| Burley | Special | Patched | Cotton Gin Pluck | A pluck that is never played the same way twice: play harder for a longer, brighter sweep (velocity on the filter envelope); every note's filter lands a little differently (random); low notes snap harder than high ones (key on the filter envelope); push the wheel and the pluck rings out like a resonant string. Snap sets the sweep, Ring the resonance, Body adds the sub-octave, Spread the stereo. | Snap (1.7), Ring (3.4), Body (13.1), Spread (3.3) |
| Burley | Special | Patched | Magnolia Pad | A slow pad where several things move at once: the LFO morphs the two oscillators into each other (LFO on the mix), each note of a chord sits at its own brightness (random on the cutoff), harder chords bloom brighter (velocity on the filter envelope), and the wheel brings in a filter sweep. Bloom sets the swell, Drift the LFO's morph, Shimmer the stereo and detune, Sweep the filter sweep before the wheel. | Bloom (2.2), Drift (1.5), Shimmer (2.8), Sweep (2.6) |
| Burley | Special | Patched | Bayou Breath Pad | A breathy pad whose noise and resonance move with you: every chord starts with a whistle in the filter that settles (Env 2 on the resonance), high notes spread wider than low ones (key on the spread), the LFO pans the unison's width in and out (LFO on the spread), each note of a chord sits a little brighter or darker (random on the cutoff), and the wheel opens the filter. Air sets the breath, Whistle the resonance, Sway how far the width swings, Open the filter. | Air (7.2), Whistle (4.0), Sway (3.2), Open (10.8) |
| Burley | Special | Patched | Revival Brass | Brass you play like brass: hard notes blare (velocity on the filter envelope and the resonance), every note swells in after its first breath (Env 2 dips the level at the start), the section is never quite in tune (random on the detune), and the wheel brings vibrato. Blare sets the brightness, Growl a cross-modulated rasp from the sub, Section the spread, Vibrato the vibrato before the wheel. | Blare (6.1), Growl (12.0), Section (2.9), Vibrato (1.9) |
| Burley | Wild | Patched | Haint Choir | An evolving choir of ghosts: a slow LFO drags the second oscillator up and down a fifth, each voice wanders off pitch on its own (random), Env 2 spreads the choir wider over a second or two, and the wheel speeds everything up. Haunt takes it from a calm choir to a screaming one; Wander sets the LFO's pull, Snarl the resonance, Space the width. | Haunt (8.8), Wander (2.6), Snarl (1.2), Space (2.4) |
| Burley | Wild | Patched | Gator Growl | A bass monster with every source pulling at it: velocity throws the sub at oscillator 1 at audio rate, the key moves the filter the wrong way (low notes brighter), the LFO chews the cross-modulation, and the wheel winds up the resonance. Jaws takes it from a fat bass to a full roar; Chew sets the LFO's speed, Mud the low end, Bite the resonance. | Jaws (2.3), Chew (3.2), Mud (5.0), Bite (2.0) |
| Burley | Wild | Patched | Firefly Field | A field of blinking lights: every note sparkles at its own random pitch and brightness, a fast LFO flickers the level, Env 2 makes each note fall in pitch as it fades, and the wheel slows the flicker to a crawl. Swarm takes it from a few glints to a dense cloud; Flicker sets the LFO's depth, Glow the filter, Fall how far each note drops. | Swarm (3.3), Flicker (6.2), Glow (3.6), Fall (5.6) |
| Burley | Wild | Patched | Tent Revival Riser | A riser you steer: hold a chord and Env 2 lifts the pitch and opens the filter over several seconds, harder chords open further (velocity on the filter envelope), every voice rises at its own rate (random on the detune), the LFO shakes the pitch, and the wheel stirs it into the filter too. Rapture takes it from a gentle lift to a shrieking climb; Speed sets the LFO, Choir the unison, Grit the cross-modulation. | Rapture (4.8), Speed (3.6), Choir (2.5), Grit (1.6) |
| Burley | Simple | Bass | Sub Sine | A sine with a touch of triangle: low end with no edge, for small speakers add Heft after it. | – |
| Burley | Simple | Bass | One-Osc Bass | One saw through the 24 dB low-pass with a short filter blip: the plainest bass that still has a front edge. | – |
| Burley | Expected | Bass | Round Bass | Saw and square an octave down, filtered low with a short pluck: a bass line that sits under the mix. | – |
| Burley | Expected | Bass | Acid Line | One saw, a resonant filter and a fast decaying sweep, with a little glide between notes. | – |
| Burley | Expected | Bass | Bottomland Bass | The classic ladder bass: saw and square an octave down into a driven 24 dB low-pass with a snappy envelope, one voice with a little glide. | – |
| Burley | Expected | Bass | Muddy Reese | Two saws pulled apart so they beat slowly against each other, doubled, under a dark low-pass: the rolling reese bass. | – |
| Burley | Expected | Bass | Rubber Bass | A square with the sub under it, the 24 dB low-pass bouncing shut on every note: round and elastic. | – |
| Burley | Special | Bass | Growl Bass | A saw an octave down, its pitch shaken by the sub at audio rate for a gritty edge, the filter wobbling with the mod wheel. Growl deepens the cross-modulation. | Growl (2.5) |
| Burley | Special | Bass | Levee Wobble | A wobble bass: the LFO sweeps a resonant filter, the mod wheel speeds it up. Wobble sets the sweep's depth, Rate its speed, Grit the resonance and an audio-rate edge from the sub, Sub the weight underneath. | Wobble (5.3), Rate (3.2), Grit (2.3), Sub (1.7) |
| Burley | Special | Bass | Thumbed Bass | A bass that answers the hand: velocity opens the filter and its envelope, the low notes a little darker. Tone opens it for every note, Thump deepens the envelope's snap. | Tone (8.5), Thump (3.2) |
| Burley | Simple | Lead | Plain Lead | Saw and square together, one voice at a time with a touch of glide: a lead to learn on. | – |
| Burley | Expected | Lead | Detuned Lead | Two saws a few cents apart, bright, with a quick attack and the mod wheel bringing in vibrato. | – |
| Burley | Expected | Lead | Square Lead | Two squares an octave apart: hollow and reedy, cutting through without much top. | – |
| Burley | Expected | Lead | Wide Stack Lead | Seven detuned saws spread across the sides, a bright filter and a little glide: a big trance-style lead. | – |
| Burley | Expected | Lead | Sync Lead | Oscillator 2 hard-synced a fifth and an octave up, its tone swept by the filter envelope: a biting lead. | – |
| Burley | Expected | Lead | Phasing Squares | Two squares a hair apart drifting through each other, the sound a pulse-width sweep makes; the mod wheel adds vibrato. | – |
| Burley | Expected | Lead | Supersaw Sky | Seven saws spread wide and far apart, bright and big: the huge unison lead for anthems. | – |
| Burley | Expected | Lead | Holler Lead | Saw and square with a driven filter, one voice and a long glide; the mod wheel brings in vibrato: a fat monophonic lead. | – |
| Burley | Special | Lead | Drift Lead | Two saws whose detune breathes with the amp envelope and wanders a little per note; the mod wheel brings in vibrato. Bite raises the resonance and the filter envelope. | Bite (2.9) |
| Burley | Special | Lead | Wheel Holler | An expressive lead that opens up with the wheel: the wheel lifts the filter, brings in vibrato and a cross-modulated edge. Bite adds resonance and envelope, Vibrato sets the wheel's vibrato depth, Edge the cross-modulation it brings. | Bite (5.7), Vibrato (4.6), Edge (1.8) |
| Burley | Special | Lead | Cross Talk | Oscillator 2 frequency-modulating oscillator 1 at audio rate, the modulation falling away with Env 2 for a bright, talking attack. Edge sets how much cross-modulation stays, Bright the filter, Tilt where oscillator 2 sits. | Edge (1.7), Bright (6.0), Tilt (2.0) |
| Burley | Wild | Lead | Hornet Nest | The sub shaking oscillator 1's pitch and the filter at audio rate, held in check: a buzzing, angry lead. Sting takes it from a gritty saw to a full swarm; the wheel speeds a growl in the filter. | Sting (7.6) |
| Burley | Wild | Lead | Bullfrog Talk | A talker: a resonant band-pass jumping between vowel-like places on the LFO and on each note's envelope, so a held note seems to speak. Mouth takes it from a murmur to a croaking shout; the wheel speeds the talking. | Mouth (7.5) |
| Burley | Simple | Pad | Plain Pad | Two saws apart, a slow fade in and out under a soft filter: the basic pad. | – |
| Burley | Expected | Pad | Warm Pad | Slow swell and long release, two saws detuned and filtered dark. | – |
| Burley | Expected | Pad | Glass Pad | Triangle and sine a fifth apart: a soft, clear bed for chords. | – |
| Burley | Expected | Pad | Wide Unison Pad | Four gently detuned copies of saw and triangle across the whole stereo field, slow to open and close. | – |
| Burley | Expected | Pad | Porch Light Pad | A bright, open pad: two saws and a triangle an octave up, unison spread, the filter following the keys. | – |
| Burley | Expected | Pad | Slow Sweep Pad | Saws under a resonant low-pass that a slow triangle LFO opens and closes over several seconds: the sweeping pad. | – |
| Burley | Special | Pad | Breathing Pad | Three detuned saws under a dark filter that the LFO opens and closes slowly, each note a hair off pitch. Open turns up the filter, Wobble the LFO's sweep, Grit the resonance and a growl from the sub, Space the unison's width. | Open (6.8), Wobble (2.8), Grit (12.0), Space (2.3) |
| Burley | Special | Pad | Creek Haze Pad | A pad where Env 2 slowly fades oscillator 2 in and pulls it apart, each note a little different from the last. Haze sets the per-note drift, Bloom how far Env 2 moves the mix, Air the filter, Width the stereo spread. | Haze (1.6), Bloom (7.5), Air (6.1), Width (2.8) |
| Burley | Special | Pad | Heat Haze | Seven saws shimmering as the LFO breathes their detune, the filter opening as each note is held. Shimmer sets the breathing, Warmth darkens or brightens, Width spreads the copies, Swell lets Env 2 open the filter further. | Shimmer (3.8), Warmth (5.9), Width (2.7), Swell (3.9) |
| Burley | Expected | Keys | Soft Keys | A plucky triangle with a decaying filter: an electric-piano-like part for chords. | – |
| Burley | Expected | Keys | Organ-ish | Sine and square an octave apart, no envelope movement: a steady, simple organ tone. | – |
| Burley | Expected | Keys | Chapel Organ | Sine and square an octave up over the sub, a slight tremolo from the LFO: a steady small organ. | – |
| Burley | Expected | Keys | Screen Door Clav | Two squares an octave apart through a band-pass that snaps shut: the nasal, percussive clav, brighter the harder you play. | – |
| Burley | Special | Keys | Velocity Pluck | A short saw-and-square pluck whose filter opens with how hard the key is struck and a little more up the keyboard. Bright opens it for every note. | Bright (5.6) |
| Burley | Special | Keys | Sunday Keys | Soft electric keys that brighten with velocity and up the keyboard, each note very slightly out of tune with the last, as an old instrument. Bright opens the filter, Ring adds resonance, Age widens the per-note tuning. | Bright (1.7), Ring (3.6), Age (2.3) |
| Burley | Wild | Keys | Pawnshop Keys | Broken lo-fi keys: every note lands at its own slightly wrong pitch and tone, driven and dull, the LFO wobbling the tuning like a worn tape. Wear takes it from a tired upright to a ruined one. | Wear (2.7) |
| Burley | Simple | Pluck | Pluck | A short, bright pluck that dies away: arpeggios and fast patterns. | – |
| Burley | Expected | Pluck | Bell Pluck | Sine and a square two octaves up, decaying: a small bell or mallet. | – |
| Burley | Expected | Pluck | Picked Saw | Two saws plucked through a low-pass that closes fast, a short ring after: arpeggios and guitar-like parts. | – |
| Burley | Special | Pluck | Dew Pluck | A glassy pluck whose snap follows velocity and whose filter follows the keys, each note's body slightly different. Snap sets the filter envelope, Body the second oscillator, Ring the resonance. | Snap (2.2), Body (10.9), Ring (3.3) |
| Burley | Expected | Brass & Strings | Brass Band | Two saws whose filter opens a moment after the note and settles back, as a brass section blows: stabs and swells. | – |
| Burley | Expected | Brass & Strings | Evening Brass | A slower, softer horn: saw and square opening gently, for held chords under a melody. | – |
| Burley | Expected | Brass & Strings | Parlor Strings | A string machine: three detuned saw pairs spread wide, a gentle bow in and a long release, a slow chorus from the LFO. | – |
| Burley | Expected | Brass & Strings | Sawmill Strings | Brighter, leaner strings with a slower bow: two saws through a high-ish low-pass, the release ringing on. | – |
| Burley | Special | Brass & Strings | Blaring Brass | Brass whose second saw blats sharp for a moment at the start of each note (Env 2 on its tuning, not the filter) and opens with velocity; the wheel adds vibrato. Blare opens the filter, Breath adds noise, Blat sets the starting bend. | Blare (4.4), Breath (4.0), Blat (2.2) |
| Burley | Special | Brass & Strings | Tremolo Strings | Strings the wheel sets trembling: the wheel brings in a fast tremolo and lifts the bow. Bow opens the filter, Tremolo sets the wheel's tremolo, Width the spread of the section. | Bow (5.1), Tremolo (1.6), Width (2.4) |
| Burley | Wild | Texture | Kudzu Creep | A drone that grows on its own: Env 2 swells the cross-modulation over many seconds while a slow LFO and per-note chance move the filter. Creep takes it from a calm hum to a tangle; Wander sets the LFO's reach, Snarl the resonance. | Creep (12.0), Wander (5.1), Snarl (2.6) |
| Burley | Wild | Texture | Screen Door Gate | A rhythmic gate: a square LFO chops a held chord on and off, the mod wheel speeding the chop. Chop takes it from a gentle pulse to a hard stutter with a filter that jumps with it. | Chop (2.7) |
| Burley | Wild | Texture | Silo Drone | Seven saws churning in a dark room, the LFO's speed itself rising as the amp envelope swells and with the wheel. Churn takes it from a still hum to a heaving, phasing wall. | Churn (8.9) |
| Burley | Expected | Effects | Riser | A slow filter opening over a long attack: hold a note into a drop. | – |
| Burley | Expected | Effects | Snare Crack | Noise through a band-pass with a fast decay and a little body from the oscillators: a noise hit for snares, claps and rhythm. | – |
| Burley | Expected | Effects | Storm Riser | Noise and saws under a filter that opens over four seconds as the level climbs: hold a note into the drop. | – |
| Burley | Expected | Effects | Bug Zapper | A bright saw whose pitch falls two octaves in a blink as the filter shuts: zaps, lasers, falling hits. | – |
| Burley | Expected | Effects | Dusk Sweep | A held wash whose resonant filter falls from bright to dark over three seconds: a downward sweep for transitions. | – |
| Burley | Special | Effects | Sweep Machine | A held chord the LFO sweeps through a resonant band-pass; the wheel speeds it from a crawl to a flutter. Sweep sets the depth, Speed the rate, Focus the resonance. | Sweep (2.9), Speed (2.4), Focus (3.9) |
| Burley | Wild | Effects | Tin Roof Rain | Glitchy sample-and-hold weather: a square LFO jumps the band-pass between two places while every note picks its own pitch and tone at random. Storm takes it from a patter to a downpour of jumps; the wheel speeds the LFO. | Storm (3.8) |
| Burley | Wild | Effects | Thunderhead Riser | A riser that climbs an octave and a half over four seconds while the filter and the LFO speed up with it. Climb sets how far the pitch rises and how fast it churns; hold a note into the drop. | Climb (4.8) |
| Burley | Wild | Effects | Off the Porch | An impact that falls: the pitch drops away from the strike while noise and the sub thump under it. Drop sets how far and how hard it falls, from a bump to a crash. | Drop (7.6) |
| Burley | Wild | Effects | Junkyard Clang | A metallic clang: oscillator 2 a tritone and a hair off cross-modulating oscillator 1 so the partials stop lining up, a quick ring and a long shimmer. Clang takes it from a dull knock to scrap metal. | Clang (5.6) |
| Cicada | Simple | Basics | Init | The synth as it starts, at the suite's level: the Classic table's sine through an open filter. Start here. | – |
| Cicada | Simple | Basics | Init for Patching | Init with the four macros named and wired to one thing each, all at zero so it sounds like Init: Morph moves through the table, Tone brightens the wave and rings the filter, Motion lets the LFO move the position, Warp bends the wave with FM. | Morph (17.9), Tone (14.8), Motion (8.6), Warp (14.9) |
| Cicada | Simple | Basics | Plain Sine | The Classic table at its sine, nothing moving: a pure tone. | – |
| Cicada | Simple | Basics | Plain Triangle | The Classic table at its triangle: soft, a few odd harmonics. | – |
| Cicada | Simple | Basics | Plain Saw | The Classic table at its saw: every harmonic, bright and buzzy. | – |
| Cicada | Simple | Basics | Plain Square | The Classic table at its square: hollow, the odd harmonics only. | – |
| Cicada | Special | Patched | Crawdad Bass | A pulse bass that you shape as you play: harder notes narrow the pulse for more bite (velocity on the position), each note starts narrow and nasal and opens out (Env 2 on the position), low notes are rounder than high ones (key on the filter), and the wheel brings in pulse-width wobble. Pulse sets the width, Fold a Warp fold, Sub the sub, Wobble the wobble before the wheel. | Pulse (2.8), Fold (1.3), Sub (7.2), Wobble (2.2) |
| Cicada | Special | Patched | Mudcat Bass | A sync bass that growls when you lean on it: every note's sync sweeps down from bright to round (Env 2 on the position), harder notes start brighter still (velocity on the position), each note lands a little differently (random on the position), high notes are brighter (key on the filter), and the wheel adds a sync wobble. Sync sets where the sweep ends, Sweep how far Env 2 sweeps, Growl an FM growl, Weight the sub. | Sync (2.5), Sweep (2.0), Growl (2.2), Weight (2.6) |
| Cicada | Special | Patched | Sync Holler Lead | A sync lead that tears when you dig in: play harder for more sync (velocity on the Warp), each note's sync starts high and falls (Env 2 on the Warp too), high notes lean from saw toward square (key on the position), each note starts at its own place on the table (random on the position), and the wheel brings a sync wobble. Tear sets the sync that stays, Fall how far Env 2 sweeps it, Shake a vibrato with a filter shimmer, Air the filter. | Tear (4.5), Fall (3.2), Shake (4.4), Air (5.4) |
| Cicada | Special | Patched | Mockingbird Lead | A lead that changes its song as you hold it: the vibrato grows in after a moment (Env 2 raises the vibrato's depth), harder notes are brighter along the Harmonic Sweep table (velocity on the position), each note gets its own touch of FM edge (random on the Warp), and the wheel pushes the FM further. Song sets the brightness, Bloom the late vibrato, Edge the FM that stays, Width the stereo. | Song (4.3), Bloom (1.2), Edge (1.4), Width (3.3) |
| Cicada | Special | Patched | Tin Roof Keys | Organ-ish keys from the Drawbars table that respond like a player's: harder notes pull out more drawbars (velocity on the position), each note starts with an FM bark that fades, like a percussion stop (Env 2 on the Warp), high notes thin out (key on the position, downwards), every key is a little brighter or duller (random on the filter), and the wheel spins up a tremolo. Stops sets the drawbars, Click an FM key click, Tremolo the tremolo before the wheel, Drift a per-key tuning drift. | Stops (3.4), Click (8.9), Tremolo (3.0), Drift (2.6) |
| Cicada | Special | Patched | Dewdrop Keys | Glassy keys from the Comb table: harder notes ring with an FM shimmer (velocity on the Warp), each note's comb sits in a slightly different place (random on the position), Env 2 sweeps the comb as the note sounds, and the wheel sets it moving. Shine sets the shimmer, Comb where the comb sits, Ring the resonance, Motion the motion before the wheel. | Shine (2.6), Comb (1.4), Ring (1.3), Motion (1.4) |
| Cicada | Special | Patched | Spanish Moss Pad | A pad that never sits still: the LFO sweeps the wave slowly between triangle and saw, every note of a chord starts at its own place in it (random on the position), harder chords are brighter (velocity on the filter), and the wheel speeds the sweep and adds a filter sway. Sway sets the LFO's sweep and a filter sway with it, Bloom a slow Env 2 FM shimmer that swells in, Glow the filter, Width the stereo. | Sway (2.0), Bloom (4.6), Glow (2.3), Width (3.1) |
| Cicada | Special | Patched | Low Country Choir | A choir pad from the Vowels table: low notes sing 'oo' and high notes 'ee' (key on the position), the LFO drifts the vowel slowly, every voice of a chord sits a few cents apart (random on the fine tune), and the wheel sets the vowels swaying back and forth. Vowel sets where the choir sings, Drift the LFO's reach, Choir the unison, Breath the noise. | Vowel (1.7), Drift (2.1), Choir (2.4), Breath (8.7) |
| Cicada | Special | Patched | Sunbelt Brass | Brass that swells and blares: every note's filter swells open with Env 2, and the harder you play the further it goes (velocity on Env 2's depth), the section is never quite together (random on the unison's detune), high notes are brighter (key on the filter), and the wheel brings vibrato. Blare sets the brightness, Swell how far Env 2 sweeps the filter and the wave, Section the spread, Vibrato the vibrato before the wheel. | Blare (1.5), Swell (4.0), Section (2.7), Vibrato (3.1) |
| Cicada | Wild | Patched | Junebug Swarm | A swarm of buzzing notes from the Odd to Even table: every note lands on its own pitch (random on the pitch), a fast LFO rattles the FM Warp, each note starts high and falls as it dies (Env 2 on the pitch), harder notes buzz brighter (velocity on the filter), and the wheel slows the rattle to a crawl. Swarm takes it from a few bugs to a cloud; Rattle sets the LFO's grip, Grit moves along the table, Buzz the resonance. | Swarm (4.5), Rattle (1.2), Grit (1.5), Buzz (1.1) |
| Cicada | Wild | Patched | Storm Shutter | A rhythmic texture that bangs in the wind: a square LFO bangs back and forth through the Grit table, harder notes slam the Sync Warp, each note starts somewhere else on the table (random on the position), high notes get less sync (key on the Warp), and the wheel speeds the banging. Storm takes it from a rattle to a gale; Bang sets the LFO's steps, Slam the sync, Gust the noise and a sweep of the high-pass. | Storm (1.7), Bang (12.0), Slam (4.5), Gust (3.8) |
| Cicada | Wild | Patched | Moonshine Still | A drone that cooks: Env 2 folds the Phase Bend table further and further over several seconds, the LFO bubbles the fine tune, every note starts on its own part of the table (random), and the wheel lifts the lid (wheel on the filter). Proof takes it from a warm hum to a boil; Bubble sets the LFO's depth, Heat the filter, Hiss the noise. | Proof (3.7), Bubble (1.8), Heat (11.3), Hiss (5.6) |
| Cicada | Wild | Patched | Holy Roller | A folding drone that shouts: the LFO folds the wave in and out, the wheel bends the whole thing up an octave, each note of a chord is folded its own amount (random on the Warp), harder chords push the Fold table further along, and high notes are brighter (key on the filter). Spirit takes it from a hum to a holler; Rock sets the LFO's speed, Shout the resonance, Spread the stereo. | Spirit (3.7), Rock (14.1), Shout (1.3), Spread (5.1) |
| Cicada | Simple | Bass | One-Table Bass | The Classic saw an octave down through a 24 dB low-pass that Env 2 opens briefly on each note: a plain bass. | – |
| Cicada | Expected | Bass | Sync Growl | The Sync Saw table swept up by the second envelope on each note, low-passed: a bass that opens and closes. | – |
| Cicada | Expected | Bass | Fold Bass | The Fold table, harder with velocity: soft notes round, hard ones growl. | – |
| Cicada | Expected | Bass | Growl Bass | Sync Saw's harmonics pushed up by Env 2 on each note and folded a little: a growling, vocal bass that opens and closes. | – |
| Cicada | Expected | Bass | Wobble Bass | Harmonic Sweep opened and closed by the LFO with the filter, the sub underneath: the wobble bass. | – |
| Cicada | Expected | Bass | Gravel Reese | Two Classic saws pulled apart and beating, a little narrow, under a dark 24 dB low-pass: the rolling reese. | – |
| Cicada | Expected | Bass | Pulse Bass | A narrow pulse from the Pulse Width table with the sub: a hollow, punchy bass that cuts on small speakers. | – |
| Cicada | Special | Bass | Wavefold Bass | The Classic table folded back on itself: harder keys and the start of each note fold it more, Env 2 letting go. Fold adds more for every note. | Fold (2.9) |
| Cicada | Special | Bass | Snarling Reese | A reese whose beating speeds up with the mod wheel and whose fold follows Env 2. Grit folds it, Movement sets the LFO's sweep through the table, Width spreads the copies, Sub adds weight. | Grit (1.7), Movement (2.0), Width (3.3), Sub (8.8) |
| Cicada | Special | Bass | Talking Bass | The Vowels table under a bass: velocity and Env 2 choose the vowel so each note says something. Mouth moves the vowel, Snap sets how far Env 2 opens it, Body the sub. | Mouth (6.3), Snap (3.2), Body (1.6) |
| Cicada | Wild | Bass | Bullwhip Bass | Audio-rate chaos held on a leash: Warp FM driven by Env 2 and velocity cracks at the start of each note, then settles into a growl. Crack takes it from a firm bass to a lashing, metallic snarl. | Crack (6.5) |
| Cicada | Expected | Lead | Pulse Lead | Pulse Width moved by a slow triangle LFO, five unison copies a little apart, the mod wheel narrowing the pulse. | – |
| Cicada | Expected | Lead | Vowel Lead | The Vowels table moved from a to u by the mod wheel, with vibrato from the LFO. | – |
| Cicada | Expected | Lead | Morph Lead | The Classic table from saw towards square as the key goes down harder, with a short glide. | – |
| Cicada | Expected | Lead | Vowel Holler | The Vowels table moving from an open a to an o on each note, with vibrato from the wheel: a formant lead that seems to sing. | – |
| Cicada | Expected | Lead | Wavefold Lead | The Fold table at the start of its sweep with Warp Fold on top, harder with velocity: a bright, rubbery lead. | – |
| Cicada | Expected | Lead | Sync Sweep Lead | Sync Square's slave pitch swept down by Env 2 on every note: the tearing, biting sync lead. | – |
| Cicada | Expected | Lead | Hollow Reed | Odd to Even near its hollow end, the mod wheel filling in the even harmonics: a clarinet-like reed. | – |
| Cicada | Special | Lead | Sync Lead | Warp Sync on the Classic table: the mod wheel sweeps the slave up for the classic tearing lead, the LFO moving it a little. Tear pushes it further. | Tear (3.2) |
| Cicada | Special | Lead | Wheel Opener | An expressive lead: the wheel opens the filter, adds vibrato and pushes Warp Sync up for bite. Bite sets how much sync there is before the wheel, Vibrato its depth, Air the filter, Color moves the wave from saw toward square. | Bite (12.0), Vibrato (2.6), Air (6.2), Color (6.0) |
| Cicada | Special | Lead | Pulse Talker | Pulse Width narrowed by the key and by Env 2 so a phrase changes color as it climbs; each note's width a little different. Width sets the pulse, Talk how far Env 2 moves it, Chorus the unison. | Width (4.2), Talk (2.9), Chorus (2.3) |
| Cicada | Wild | Lead | Formant Talker | A voice that talks: the Vowels table jumped between vowels by a square LFO and by chance on each note, Warp Bend thickening it. Jabber takes it from a slow murmur to a chattering rant; the wheel speeds the talking. | Jabber (1.8) |
| Cicada | Wild | Lead | Drawbar Meltdown | An organ coming apart: Warp Bend and Fold pushed by the LFO and the wheel through the Drawbars table. Melt takes it from a church to a carnival on fire. | Melt (4.1) |
| Cicada | Simple | Pad | Plain Pad | Harmonic Sweep half open, three copies a little apart, a slow fade in and out: the basic pad. | – |
| Cicada | Expected | Pad | Harmonic Swell | Harmonic Sweep opening slowly as the note is held, seven copies spread wide. | – |
| Cicada | Expected | Pad | Comb Drift | The Comb table drifting with a slow sine LFO, low-passed, wide. | – |
| Cicada | Expected | Pad | Grit Texture | Grit's fixed random spectra moving under a band-pass that follows the keys. | – |
| Cicada | Expected | Pad | Cottonwood Pad | Harmonic Sweep opening slowly as each chord is held while the LFO drifts it: an evolving, airy pad. | – |
| Cicada | Expected | Pad | Choir Pad | The Vowels table moving slowly from vowel to vowel under the LFO, wide and soft: a pad of voices. | – |
| Cicada | Expected | Pad | Morning Fog | Odd to Even fading from hollow to full over each held chord, filtered warm: a soft, slowly changing bed. | – |
| Cicada | Expected | Pad | Shimmer Pad | Phase Bend near its bright end, seven copies spread wide, the filter following the keys: a glassy, shining pad. | – |
| Cicada | Special | Pad | Glass Sweep | The Harmonic Sweep table drifting under Env 2 and a slow LFO, a touch of Warp FM breathing with it, each note's spread a little different. Creep moves the position, Shine opens the filter, Drift adds vibrato, Depth the FM. | Creep (2.9), Shine (4.4), Drift (2.0), Depth (4.4) |
| Cicada | Special | Pad | Kudzu Bloom | A pad that blooms: Env 2 slides the Comb table and warps it while the LFO breathes and every note starts a little differently. Bloom sets Env 2's sweep, Breath the LFO, Air the filter, Width the spread. | Bloom (1.6), Breath (1.6), Air (6.0), Width (4.3) |
| Cicada | Special | Pad | Drawbar Haze | A drawbar pad that changes its registration with the mod wheel and as notes are held, wide and slow. Stops moves through the registrations, Swell sets the slow fade, Width the spread. | Stops (10.9), Swell (5.2), Width (2.4) |
| Cicada | Expected | Keys | Drawbar Keys | Organ registrations from the Drawbars table, fuller with velocity, and a quick key click from noise. | – |
| Cicada | Expected | Keys | Glass Keys | Phase Bend falling from bright to soft over each note: a bell-like attack that settles. | – |
| Cicada | Expected | Keys | Tin Keys | The Steps table's stepped, digital waves with a short decay: lo-fi glassy keys for chords and riffs. | – |
| Cicada | Expected | Keys | Drawbar Organ | A full drawbar registration held steady, a quick click and a slight rotary wobble from the LFO. | – |
| Cicada | Expected | Keys | Digital Piano | Phase Bend falling from bright to soft over each note and harder with velocity: clean digital electric keys. | – |
| Cicada | Special | Keys | Bent Keys | Warp Bend on the Drawbars table, bent further by harder keys, the filter following the keyboard. Bend adds more to every note. | Bend (1.9) |
| Cicada | Special | Keys | Sunday Bells | Electric bell-keys: velocity sets how much Warp FM rings, the higher notes softer, each note's ring slightly different. Ring sets the FM, Shine the brightness, Age the per-note detune. | Ring (2.0), Shine (7.8), Age (3.7) |
| Cicada | Wild | Keys | Thrift Store Keys | Broken lo-fi keys: the Steps table at its coarsest, each note landing a little off pitch and tone, the LFO wobbling the tuning as a worn tape would. Wear takes it from tired to ruined. | Wear (2.8) |
| Cicada | Simple | Pluck | Plain Pluck | The Classic saw through a low-pass that Env 2 snaps shut: the basic pluck. | – |
| Cicada | Expected | Pluck | Step Pluck | The Steps table falling from fine to coarse on each note, short and dry. | – |
| Cicada | Expected | Pluck | Sync Pluck | Sync Square snapping down from its top on each note, with a filter pluck. | – |
| Cicada | Expected | Pluck | Creek Glass | A bell pluck: Phase Bend's bright end dropping away in a blink with Warp FM ringing on top, then a soft tail. | – |
| Cicada | Expected | Pluck | Harp Pluck | Odd to Even near its full end, a soft attack and a long ring that darkens: harp-like arpeggios. | – |
| Cicada | Expected | Pluck | Comb Pluck | The Comb table's notched spectrum plucked short: a hollow, dulcimer-like twang. | – |
| Cicada | Special | Pluck | Fold Pluck | A pluck that folds harder the harder it is struck, Env 2 letting the fold go at once: soft notes round, hard ones snarl. Fold sets the fold, Tone the filter, Snap Env 2's share. | Fold (3.6), Tone (3.2), Snap (1.7) |
| Cicada | Expected | Texture | Dust Devil | Grit's rough spectra spinning under the LFO with a breath of noise, through a band-pass that wanders: a sweeping, grainy texture. | – |
| Cicada | Special | Texture | Rain Barrel | The Comb table's notches combed by the LFO and the key, a little noise, very wide: a hollow, wet texture under a melody. Drip sets the LFO's sweep, Depth the notch movement per key, Hiss the noise. | Drip (5.5), Depth (12.0), Hiss (8.4) |
| Cicada | Wild | Texture | Cicada Chorus | A summer-night drone: Grit's rough frames trilling under a fast LFO with Warp FM buzzing, every note a different shade. Swarm takes it from a distant hum to a deafening chorus; the wheel speeds the trill. | Swarm (2.3) |
| Cicada | Wild | Texture | Heat Shimmer | A self-evolving drone: Sync Square swept by a slow LFO and by Env 2 over many seconds, the copies drifting apart and back. Melt takes it from a still tone to a wavering mirage; the wheel speeds the LFO. | Melt (3.5) |
| Cicada | Wild | Texture | Pulse Gate | A rhythmic gate: a square LFO chops a wide pulse chord on and off while sweeping the pulse width with it. Chop takes it from a gentle throb to a hard stutter; the wheel speeds it. | Chop (2.7) |
| Cicada | Expected | Effects | Harmonic Riser | Harmonic Sweep climbing from dull to bright over five seconds, seven copies spreading: hold a chord into a drop. | – |
| Cicada | Expected | Effects | Noise Sweep | Noise and the Grit table through a resonant band-pass that Env 2 sweeps down over two seconds: a whoosh for transitions. | – |
| Cicada | Special | Effects | Sweeping Steps | The Steps table stepping through its frames on the LFO while the wheel speeds it: a rhythmic, digital sweep. Steps sets the sweep, Speed the rate, Bite the resonance. | Steps (3.2), Speed (5.7), Bite (2.4) |
| Cicada | Wild | Effects | Glitch Jar | Sample-and-hold-like glitches: every note lands on a random frame of the Steps table, a random pitch and a random cut, chopped by a square LFO. Shatter takes it from a few skips to a full glitch storm. | Shatter (3.8) |
| Cicada | Wild | Effects | Tornado Siren | A siren and riser: the pitch climbs an octave over four seconds while the LFO winds a sync sweep faster with the wheel. Wind takes it from a slow wail to a howling spin. | Wind (6.8) |
| Cicada | Wild | Effects | Anvil Strike | A metallic clang: the Classic sine heavily phase-modulated by Warp FM a moment after the strike, falling away into a ring. Clang takes it from a soft knock to scrap iron. | Clang (8.4) |
| Steeple | Simple | Basics | Init | The synth as it starts, at the suite's level: one operator modulating another at the same ratio, a simple bright tone that settles. Start here. | – |
| Steeple | Simple | Basics | Init for Patching | Init with the four macros named and wired to one thing each, all at zero so it sounds like Init: Bright raises the modulator, Bite the carrier's feedback, Ratio nudges it off the harmonic, Vibrato brings in the LFO. | Bright (6.1), Bite (14.9), Ratio (2.2), Vibrato (3.1) |
| Steeple | Simple | Basics | Plain Sine | One operator alone, no modulation: a pure sine that holds while the key does. | – |
| Steeple | Simple | Basics | Plain FM | One operator modulating another at the same ratio, held steady: the plain FM tone, a little like a muted saw. | – |
| Steeple | Simple | Basics | Feedback Saw | One operator feeding back on itself: the sine turns toward a saw. Raise Op 1 Feedback for more buzz. | – |
| Steeple | Special | Patched | Pecan Bass | A woody FM bass that follows your hands: play harder for more bark (velocity on the modulator), low notes are brighter than high ones so the line stays even (key on the modulator, downwards), every note starts with a click of extra overtones (Env 2 on the top modulator), and the wheel sets the tone wobbling. Bark sets the brightness, Click the attack, Growl the feedback, Wobble the wobble before the wheel. | Bark (3.2), Click (18.3), Growl (2.2), Wobble (2.0) |
| Steeple | Special | Patched | Rattlesnake Bass | A gritty feedback bass: harder notes rattle with more feedback (velocity on the feedback), each note snaps in with an off-ratio overtone that settles (Env 2 on the modulator's ratio), high notes are smoother (key on the feedback, downwards), every note bites a little differently (random on the modulator), and the wheel pushes the modulator harder. Rattle sets the feedback, Snap how far the ratio jumps, Weight the low octave, Edge the brightness. | Rattle (13.9), Snap (3.5), Weight (2.2), Edge (16.7) |
| Steeple | Special | Patched | Steam Whistle Lead | A reedy lead that sings as you hold it: the vibrato grows in after a moment (Env 2 raises the vibrato's depth), harder notes are reedier (velocity on the feedback), high notes calmer (key on the modulator, downwards), and the wheel brightens it like a harder breath. Reed sets the brightness, Bloom the late vibrato, Steam a breathy third operator, Flutter a faster, wider vibrato. | Reed (18.8), Bloom (1.3), Steam (15.2), Flutter (1.4) |
| Steeple | Special | Patched | Riverboat Calliope | A steam-organ lead from three pipes: each note starts with a puff of reedy chiff on the top pipe (Env 2 on its modulator), harder notes open the octave pipe (velocity), high notes thin out (key on the top pipe, downwards), every pipe's reed is voiced a little differently (random), and the wheel sets the pipes trembling. Pipes sets the upper stops, Chiff the puff, Tremble the tremolo before the wheel, Reed bends the top pipe's reed off true. | Pipes (1.5), Chiff (6.1), Tremble (2.8), Reed (2.4) |
| Steeple | Special | Patched | Sanctified Tine | An electric piano that plays like a real one: harder notes bark and ring (velocity on both modulators), high notes are sweeter (key on the bark, downwards), every key's tine rings a little differently (random on the tine's level), and the wheel brings in a tremolo. Bark sets the edge, Tine the bell, Tremolo the tremolo before the wheel, Warm darkens the body. | Bark (3.7), Tine (6.8), Tremolo (2.4), Warm (6.2) |
| Steeple | Special | Patched | Hammered Dulcimer | Struck strings: harder notes ring brighter and longer in the overtones (velocity on the modulator), every string is a touch out from its neighbour (random on the detune), each strike starts with a metallic shimmer that fades (Env 2 on an inharmonic operator), and the wheel sets the strings trembling. Strike sets the brightness, Shimmer the metal, Course the double-string detune, Sway a slow sway in the overtones. | Strike (2.5), Shimmer (3.9), Course (2.2), Sway (1.1) |
| Steeple | Special | Patched | Cumberland Pad | A warm FM pad that moves on its own and under your hands: the LFO swells the shared modulator so the chord breathes, every voice of a chord is tuned its own way (random on a carrier's detune), harder chords are brighter (velocity on the modulator), and the wheel brightens it further. Breath deepens and quickens the breathing, Glow the brightness, Drift the detune and a little grain, Air a high shimmer. | Breath (2.6), Glow (10.0), Drift (11.8), Air (16.3) |
| Steeple | Special | Patched | Blue Ridge Strings | Bowed strings that swell like a section: every note bows in brighter as Env 2 swells the modulator, harder chords bow harder (velocity on the feedback's rosin), the LFO's chorus drifts the second string against the first, and the wheel adds vibrato. Bow sets the brightness, Rosin the grit, Section the detune, Vibrato the vibrato before the wheel. | Bow (5.1), Rosin (2.4), Section (1.6), Vibrato (3.2) |
| Steeple | Special | Patched | Second Line Brass | A street brass band: every note swells into its blare (Env 2 on the modulator), harder notes blare more (velocity on the feedback), the horns are never quite together (random on the detune), and the wheel brings a wide vibrato. Blare sets the brightness, Growl the feedback, Spread the detune between the horns, Vibrato the vibrato before the wheel. | Blare (12.7), Growl (3.1), Spread (1.6), Vibrato (2.6) |
| Steeple | Wild | Patched | Twister Bells | Bells caught in a storm: every strike lands on its own inharmonic ratio (random on the modulator's ratio), the LFO swings the modulator's depth, each bell falls in pitch as it rings (Env 2 on the pitch), and the wheel speeds the swing into a whirl. Storm takes it from struck bells to a twisting cloud; Swing adds a swinging tremolo, Clang the strike, Fall how far each bell drops. | Storm (1.2), Swing (2.0), Clang (4.1), Fall (12.7) |
| Steeple | Wild | Patched | Junkyard Choir | A choir of scrap metal: a fixed-frequency operator clangs under every note at its own random pitch, the LFO swings that clang up and down, harder notes clang louder, high notes buzz brighter (key on the modulator), and the wheel pushes the clang further. Scrap takes it from a hum to a junk pile; Bend sets the LFO's grip, Clank the feedback, Rust the low drone. | Scrap (4.3), Bend (2.3), Clank (5.0), Rust (5.1) |
| Steeple | Wild | Patched | Radio Preacher | A voice on a far-off AM station: the LFO sweeps the modulator's ratio so the tone talks, each note preaches in its own key of feedback (random), harder notes shout (velocity on the modulator), and the wheel turns up the static (wheel on the third operator). Sermon takes it from a murmur to a shout; Talk sets how far and fast it talks, Static the feedback, Distance fades the station away. | Sermon (15.2), Talk (1.2), Static (13.7), Distance (9.2) |
| Steeple | Wild | Patched | Hail on the Tin | Hail on a tin roof: every note lands at its own random pitch and brightness, Env 2 snaps each one with a bright strike, high notes ring longer in the metal (key on the inharmonic operator), and the wheel shakes it all with a fast LFO. Storm takes it from a few drops to a downpour; Rattle sets the LFO, Ring the metal, Pitch how far the drops scatter. | Storm (6.6), Rattle (2.8), Ring (8.9), Pitch (11.1) |
| Steeple | Expected | Keys | Tine Piano | Two pairs: a bright high modulator for the strike on one carrier, a soft one for the body on the other; harder keys bring more of both. | – |
| Steeple | Expected | Keys | Drawn Organ | All four heard at 0.5, 1, 2 and 4 times the key, held while the key is. | – |
| Steeple | Expected | Keys | Clav Pluck | A third-harmonic modulator dying away fast over a short carrier: a plucked, nasal key. | – |
| Steeple | Expected | Keys | Soft Tine Piano | A gentle electric piano: a soft same-ratio pair for the body and a quiet high pair for the tine, ringing long. | – |
| Steeple | Expected | Keys | Barking Tine | An electric piano that barks when struck hard: the body pair's modulator jumps with velocity and feeds back a little. | – |
| Steeple | Expected | Keys | Bright Tine | A glassy, bright electric piano: the tine pair forward, a sharp attack, the body thinner, for cutting through a mix. | – |
| Steeple | Expected | Keys | Percussive Organ | Drawbar-style organ at 1, 2 and 3 times the note with a quick second-harmonic click that dies away, held as long as the key. | – |
| Steeple | Expected | Keys | Pump Organ | A reedy pump organ: a feedback operator with a soft modulator, a slow breath in and a little wobble. | – |
| Steeple | Expected | Keys | Pickin' Clav | A clav: a fourth-harmonic modulator and some feedback snapping off a short carrier, brighter with velocity. | – |
| Steeple | Special | Keys | Velocity Tine | An electric-piano pair whose bark follows velocity and calms higher up the keyboard. Bark adds edge to every note. | Bark (5.1) |
| Steeple | Special | Keys | Sunday Tine | An electric piano that plays like one: velocity sets the bark and the tine, the high notes calmer, each note's tine very slightly different. Bark adds edge, Tine the bell on top, Mellow darkens the body, Shimmer a slow chorus. | Bark (5.2), Tine (3.1), Mellow (11.2), Shimmer (2.7) |
| Steeple | Wild | Keys | Waterlogged Piano | Broken lo-fi keys: an electric piano left in the rain, each note landing a little off pitch with a wobbling tine and a flat body. Rot takes it from tired to ruined. | Rot (2.4) |
| Steeple | Simple | Bells | Plain Bell | A modulator at three and a half times the note on a long-ringing carrier: the plain FM bell, inharmonic on purpose. | – |
| Steeple | Expected | Bells | Tubular Bell | Modulators at 3.5 and 1.4 times the key: inharmonic partials ringing down over seconds. | – |
| Steeple | Expected | Bells | Glass | A seventh-harmonic modulator, a little detuned, over a carrier that rings and fades. | – |
| Steeple | Expected | Bells | Tin Roof Bell | A bright, clangy bell: two inharmonic pairs ringing at different lengths, the high one fading first. | – |
| Steeple | Expected | Bells | Dinner Bell | A hand bell: a strike that brightens the first instant, then a warm hum with a slow beat between two near partials. | – |
| Steeple | Expected | Bells | Wind Chime | Small, high chimes: a short inharmonic strike and a thin ring, quieter low down. | – |
| Steeple | Special | Bells | Struck Bell | A bell whose inharmonic modulator is struck harder by harder keys and dies away on Env 2, each note's partials a little apart. Bell raises the high partial, Bite the strike, Shimmer a slow vibrato. | Bell (3.4), Bite (4.5), Shimmer (2.2) |
| Steeple | Special | Bells | Steeple Bells | Church bells: Env 2 lets the strike's inharmonic partials ring out and fade at their own pace, velocity sets the clang, every bell a little differently tuned. Clang raises the strike, Hum the low partial, Toll stretches the ratios. | Clang (4.2), Hum (11.9), Toll (2.6) |
| Steeple | Expected | Mallets | Marimba | A carrier and a fourth-harmonic modulator gone almost at once, with a soft second pair for the wood. | – |
| Steeple | Expected | Mallets | Porch Vibes | A vibraphone: a soft fourth-harmonic strike over a pure, long tone with the motor's tremolo from the LFO. | – |
| Steeple | Expected | Mallets | Glass Mallet | Struck glass: a high, inharmonic ping on a clear tone that rings a moment. | – |
| Steeple | Expected | Mallets | Thumb Piano | A kalimba-like tine: a short, woody tenth-harmonic click on a quick-fading tone. | – |
| Steeple | Special | Mallets | Velvet Marimba | A marimba whose mallets harden with velocity and soften up the keyboard, with Env 2 adding a knock that is not a filter. Mallet sets the hardness, Wood the body, Motor a vibraphone tremolo. | Mallet (2.9), Wood (8.4), Motor (3.4) |
| Steeple | Simple | Bass | Plain FM Bass | A carrier with a same-ratio modulator that falls quickly from bright to round: the basic FM bass. | – |
| Steeple | Expected | Bass | Solid Bass | One modulator at the key's own pitch with a quick drop: round with a short bite. | – |
| Steeple | Expected | Bass | Slap Bass | A carrier under two modulators, one at the octave snapping off fast for the slap. | – |
| Steeple | Expected | Bass | Rubber Band Bass | A round, elastic FM bass: a half-ratio modulator with feedback dropping fast, the sound bouncing back to a hum. | – |
| Steeple | Expected | Bass | Fretless FM | A soft, singing fretless bass: a gentle modulator that swells a little after the note, with vibrato from the wheel. | – |
| Steeple | Expected | Bass | Low Slap | A slapped bass: a bright, feedback-edged strike falling into a solid low tone. | – |
| Steeple | Special | Bass | Wheel Growl Bass | A bass that growls with the wheel: the wheel drives every modulator harder and opens the feedback. Growl sets the bite before the wheel, Weight the half-ratio body, Thump the attack's snap. | Growl (3.7), Weight (5.2), Thump (2.9) |
| Steeple | Expected | Brass | Brass Section | A modulator that swells in slightly behind the carrier, with feedback for the edge; the mod wheel adds bite. | – |
| Steeple | Expected | Brass | Horn Stab | A short, punchy brass stab: the modulator rising quickly with the note and falling back, as a section's accent. | – |
| Steeple | Expected | Brass | Soft Horns | Mellow horns for pads and long chords: a slow, dark swell with a touch of vibrato from the wheel. | – |
| Steeple | Special | Brass | Swelling Brass | A ratio-1 pair that brightens as Env 2 swells in, as brass does; the mod wheel adds vibrato. Blare opens it further. | Blare (6.8) |
| Steeple | Special | Brass | Brass Choir | A brass section that breathes: Env 2 swells the modulators after each note starts, velocity adds blare, the wheel brings in vibrato. Blare raises the brightness, Breath the feedback's rasp, Vibrato the wheel's depth. | Blare (8.5), Breath (7.2), Vibrato (2.6) |
| Steeple | Expected | Pluck | Koto Pluck | A koto: a bright third-harmonic pluck whose pitch dips into the note, ringing down. | – |
| Steeple | Expected | Pluck | Harp Strings | A harp: a pure carrier with a whisper of second harmonic, softly plucked and ringing long. | – |
| Steeple | Expected | Pluck | Porch Guitar | A nylon-string guitar: a chain of three operators for a soft, woody pluck that rounds off as it rings. | – |
| Steeple | Special | Pluck | Koto Garden | A koto that bends into each note by velocity, its pick brighter up the keyboard, each string a hair differently tuned. Pick sets the brightness, Bend how far its pitch sways, Ring a metallic ring in the pick. | Pick (4.4), Bend (5.0), Ring (1.6) |
| Steeple | Expected | Lead | Feedback Lead | One carrier feeding back on itself towards a saw, the LFO moving its depth; vibrato on the mod wheel. | – |
| Steeple | Expected | Lead | Whistle Lead | A pure, breathy lead: a sine with a faint second harmonic and vibrato from the wheel. | – |
| Steeple | Special | Lead | Steeple Lead | An expressive lead that opens with the wheel: the wheel raises the modulator and the feedback and brings vibrato. Edge sets the brightness before the wheel, Vibrato its depth, Reed a hollow third-harmonic layer. | Edge (3.7), Vibrato (2.6), Reed (17.5) |
| Steeple | Wild | Lead | Feedback Storm | Audio-rate chaos on a leash: every operator feeding back, the chain's depth swinging with the LFO, the wheel speeding it. Storm takes it from a gritty lead to a howling squall that never runs away. | Storm (6.9) |
| Steeple | Simple | Pad | Plain Pad | Two operators fading in slowly, the modulator soft: the basic FM pad. | – |
| Steeple | Expected | Pad | Warm Pad | Three carriers a few cents apart, shared by one slow modulator that the LFO breathes. | – |
| Steeple | Expected | Pad | Frost Pad | A glassy pad: two pairs at different ratios fading in slowly, the high one a little detuned. | – |
| Steeple | Expected | Pad | Evening Strings | FM strings: one modulator driving three carriers spread a few cents apart, bowed in slowly, a gentle vibrato. | – |
| Steeple | Special | Pad | Drifting Glass | Four operators heard together, their fine ratios wandering with the LFO and a little per note: a glassy, moving bed. Drift deepens the wandering. | Drift (2.7) |
| Steeple | Special | Pad | Choir Loft | A breathing pad: the LFO sways the modulators, Env 2 slowly lifts the upper pair and every note's tuning drifts a little. Sway sets the LFO's reach, Lift Env 2's, Air the upper pair, Drift the per-note tuning. | Sway (2.2), Lift (7.4), Air (2.0), Drift (1.9) |
| Steeple | Expected | Percussion | Metal Hit | A stack of modulators at unrelated ratios over a fixed 220 Hz carrier: a struck metal sound the same on every key. | – |
| Steeple | Expected | Percussion | Triangle Ting | An orchestral triangle: a fixed, high inharmonic ring whatever the key, with a long, thin tail. | – |
| Steeple | Expected | Percussion | Hollow Tom | A tom: a low carrier whose pitch drops as it is struck, a short noisy edge from feedback. | – |
| Steeple | Expected | Percussion | Tin Hat | A metallic hat: feedback noise on a high fixed operator, a quick tick that rises with velocity. | – |
| Steeple | Special | Percussion | Cowbell Kit | A struck metal kit on one patch: the key picks the pitch, velocity the clang, and Env 2 a knock at the start. Clang raises the modulator, Ring a harsher, ringing clang, Pitch tunes the fixed operator. | Clang (4.2), Ring (7.6), Pitch (14.6) |
| Steeple | Wild | Percussion | Junk Drawer | Metallic clangs: fixed, inharmonic operators struck with a random pitch on every note, rattling and dying fast. Rattle takes it from a soft tap to a drawer of scrap tipped out. | Rattle (6.1) |
| Steeple | Wild | Texture | Bell Tower Drone | A self-evolving drone of slowly beating bells: the LFO and Env 2 move the ratios over tens of seconds so the partials grind against each other. Toll takes it from a calm hum to a clanging, unstable tower; the wheel speeds the sway. | Toll (5.8) |
| Steeple | Wild | Texture | Telegraph Wire | A rhythmic gate: the square LFO chops a buzzing feedback tone on and off and flips its brightness with it, the wheel speeding it up. Chop takes it from a gentle pulse to a hard stutter. | Chop (3.4) |
| Steeple | Wild | Texture | Ratio Roulette | Sample-and-hold-like FM: every note draws its own modulator ratio and depth at random, the LFO stepping the depth like a sequencer. Spin takes it from gentle variation to a different creature on every key. | Spin (5.1) |
| Steeple | Wild | Effects | Church Bell Fall | A bell that falls through the floor: struck, then its pitch sinks an octave over three seconds while the partials smear. Fall sets how far and how much it smears, from a gentle sag to a collapse. | Fall (10.4) |
| Steeple | Wild | Effects | Steeple Riser | A riser: the carriers climb over four seconds while the modulators open and the LFO winds faster with the wheel. Climb sets how high it goes and how hard it churns. | Climb (8.1) |
| Steeple | Wild | Effects | Lightning Strike | An impact: a burst of feedback noise and a falling, clanging partial over a low thud. Strike takes it from a distant rumble to a crack overhead. | Strike (9.6) |
