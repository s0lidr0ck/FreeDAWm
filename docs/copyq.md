# CopyQ

CopyQ (`fs_copyq`) is a match equaliser: it measures a sound as a **profile**, and makes another
sound's tone meet it. Like Manners it is written here in Rust and has no C++ original, so there
is no fidelity test; what says it is right is its own tests and listening.

It copies tonal balance and nothing else: not compression, saturation, the room or the
performance. It works best between similar sources.

## Using it

1. **Capture.** Put CopyQ on the sound to copy, switch **Capture** on and play it. Everything
   that plays is averaged (silence and bleed are left out); the audio passes untouched. Type a
   name and press **Save**: the profile goes into the library.
   - **From File…** measures an audio file instead, and **From Selection** the edit selection on
     the selected audio track. Neither needs a module on the source.
2. **Match.** On the sound to change, click a profile in the **Library**. CopyQ measures this
   track as it plays (**Follow**) and applies the difference. The display shows this track
   (grey), the target (the accent line) and the response applied.
   - A profile's **+** blends it in with the ones already matched to (up to four), each with a
     weight.
   - **Freeze** keeps this track's sound as measured so far, so the match holds still and is the
     same every time the session opens. **Unfreeze** goes back to following. Freeze before a
     bounce that has to come out the same every time: while following, the curve at any moment
     depends on what the module has heard up to then.
3. **Link.** Put several CopyQs in one link group (the **Group** button):
   - **Send** publishes its sound and passes its audio untouched;
   - **Match** takes the group's sound as its target (in place of its library profiles);
   - **Group** does both: every Group member comes to the group's average, or to the Send
     members when there are any.

### The controls

| Control | What it does |
|---|---|
| Amount | How much of the difference is applied: 100 % all of it, over 100 % more than all, below zero away from the target (to make room for it) |
| Detail | How fine the curve is: 0 % follows the broad tilt only (about two octaves), 100 % every twelfth of an octave |
| Max Gain | The most any frequency is turned up or down. Untouched up to 60 % of it, then it bends over |
| Dynamics | How much the curve follows the target's loud and quiet passages: the curve leans to the target's loud tone when this track is loud, its quiet tone when it is quiet. Needs a target and a source that both have loud and quiet passages |
| Window | How long this track is averaged over while following |
| Gate | Frames this far under the programme level are not measured (silence, tails, bleed) |
| Low / Low Mid / High Mid / High | Each range's own amount; the display's three splits drag |
| The display's edges | The working range: nothing is changed outside it |
| Channels | Stereo: one curve for both sides. Mid/Side or Left/Right: a curve each, so the width (or a lopsided pair) carries over. Needs a stereo target and a stereo track |
| Phase | Linear: by STFT, exact, a frame of latency (4096 samples at 44.1/48 kHz). Minimum: no latency, through 31 third-octave bells, so third-octave fine |
| Match Level | Gives back the level the curve changes (a K-weighted estimate) |
| Delta | Plays only what the curve changes |
| Output | A trim after everything |
| Send to CleanQ | Turns the response being applied right now into a CleanQ after this module, as bells that can be edited (up to 16, or 12 a side), and bypasses CopyQ |

## Profiles

A profile is fifteen curves on one grid (121 points, a twelfth of an octave apart from 20 Hz):
the sum, left, right, mid and side of the sound, each for all of it, its loud passages (3 dB or
more over its mean level) and its quiet ones (3 dB or more under). A value is the power in that
twelfth of an octave, so pink noise is a flat line. One file per profile, JSON
(`"format": "copyq-profile"`), the curves packed as base64.

- **The library** is the folder `CopyQ Profiles` under the app's preset folder. A profile is
  named after its file. Deleting a profile's file does not change the modules that match to it:
  each module keeps its own copy in the session.
- **Factory profiles** are straight spectral slopes made in code (Flat, Bright, Pink, Warm,
  Dark). They are measured from no recording. A library of real sounds has to come from audio
  that may be used that way; see the tool below.
- This is not Scout's sound profile (`scout.profile_capture`): that one holds Scout's
  measurements for its mix feedback, this one what CopyQ matches. Neither reads the other's
  files yet.

### Making profiles from files

```sh
cargo run --release -p soundcraft-dsp --example copyq_profile -- make recordings/ profiles/
cargo run --release -p soundcraft-dsp --example copyq_profile -- show profiles/snare.json
cargo run --release -p soundcraft-dsp --example copyq_profile -- render in.wav out.wav profiles/snare.json amount=100
```

`make` measures a file, or every audio file in a folder, as the app does. `show` prints a
profile. `render` runs a file through CopyQ matching to a profile, to hear a match without the
app. Copy the profiles into the `CopyQ Profiles` folder and they are in every CopyQ's library.

## Commands

| Command | |
|---|---|
| `mix.copyq_measure` | `{path}` \| `{from?, start?, end?}` \| `{factory: name}`, `name?`, `gate?` → `{profile, name, seconds, stereo, loud, quiet, factory}`. Measures a file, a range of an audio track (default the edit selection) or a factory profile. Changes nothing |
| `mix.module_copyq` | `{track?, module, targets?: [{profile, weight?}] \| null, source?: profile \| null}`. What a CopyQ module matches to (up to four profiles) and its frozen source. Undoable |
| `mix.module_action` | `{action: "reset_measure"}` forgets what a module measured of its input |

Everything else is a parameter (`mix.module_params`). A preset never sets Capture, Link or
Bypass.

## How it works

Per frame (an STFT, a new frame every quarter of it):

1. **Measure.** The frame's power per twelfth of an octave, for the five views, goes into the
   running average of this module's input, kept three ways (all, loud, quiet). A frame is left
   out when it is under -70 dB, or more than Gate under the programme level (which rises at
   once and falls 4 dB a second).
2. **Compare.** Target minus source, in dB, with the overall level difference taken out (one
   number for every view, so how far the side sits under the mid carries over).
3. **Shape.** Smoothed over frequency (Detail), scaled (Amount and the ranges), held under Max
   Gain, faded out outside the working range, smoothed in time (90 ms). A live source is not
   trusted in full until two seconds of it were heard.
4. **Level.** The level the curve changes is given back.
5. **Apply.** Linear phase: the gains go onto the FFT bins and the frame is resynthesised.
   Minimum phase: the curve goes to a bank of 31 bells in series, whose gains are solved so the
   bank's real response meets the curve at every band centre.

The module's targets and frozen source are its stored state. The engine makes them ready off
the audio thread (the profiles blended into one target) and the plugin copies them into fixed
storage between blocks; `process` never allocates.

## What is tested and what is not

Tested (`crates/dsp/src/plugins/fs/copyq/tests.rs` and the editor's tests): pink noise measures
flat and white rises 3 dB an octave, at any sample rate; a match brings a sound within 1 dB
(linear phase) or 1.6 dB (minimum phase) of the target's shape; Amount, the ranges, Max Gain,
blending, mid/side and left/right, loud and quiet, the link roles, Capture, Freeze, Delta, mono,
hostile input. `crates/mix/tests/copyq_link.rs` runs Send, Match and Group across tracks in the
mix engine.

Not yet done:

- **Listening.** Nothing here was tuned by ear. The time constants, the trust ramp, the
  loud/quiet margin and Dynamics in particular are first values and want a session with real
  material.
- **A library of real sounds.** It ships with the five slopes only.
