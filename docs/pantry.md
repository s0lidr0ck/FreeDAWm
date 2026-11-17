# Pantry: the multisampler, and capturing instruments into it

> **Status:** the instrument with its own window, SFZ loading and capture (on the Track menu,
> with a settings dialog and a progress window) are built · **Written:** 2026-11-17 · **Owner decisions (2026-11-17):** a separate instrument
> from Lift; the name Pantry; twelve velocity layers by default; velocity layers that blend;
> capture done inside the app, each note rendered on its own, with the settings below

**Pantry** plays an instrument recorded note by note, each note at several strengths. The name
follows the FireStation line's Southern theme: the pantry is where a good variety of things is
put up and kept. It is listed as **"Pantry - Multisampler"**, `Pantry` in insert slots, plugin id
`multisampler`.

Its samples come from two places: an SFZ file (`mix.multisample_load`), or any instrument track
in the session, played through its effects and captured (`track.capture_multisample`). Capture
is the point: put a piano on a track, a guitar amp after it, and keep the result as an instrument
of its own.

## The instrument

- **Samples** are session sources (`Insert::samples`), saved in `Audio Files/` with the session,
  keyed `n{note}.v{velocity}.r{robin}`: the note the sample was recorded at (0–127), the velocity
  it was played with (1–127) and its round-robin take (1–4). A note holds up to 16 velocity
  layers.
- **Keys.** A key plays the nearest recorded note (the lower of two equally near), pitched to
  the key.
- **Velocity.** A layer covers the velocities from just above the layer below it up to its own.
  **Velocity Curve** turns a softer hit than the layer was recorded at down (the level falls as
  the velocity ratio to that power; 0 plays every hit as recorded). **Layer Blend** mixes in the
  layer below over the bottom of each layer's range, at equal power, so the tone changes
  gradually: at 100% every hit between two layers is a mix of both, at 0% the layers switch.
- **Round-robins.** Each hit of a recorded note plays its next take.
- **Notes** have an amp ADSR. The sustain pedal holds released notes; pitch bend is ±2
  semitones. 96 voices (a blended hit uses two).
- Level, Pan, Transpose and Fine Tune.

## The window

Pantry opens in its own window from the track's instrument slot: the wordmark and what is on the
shelves (samples, notes, layers, seconds, megabytes), **Load SFZ…** and **Clear**
(`mix.multisample_clear`); the **Shelves**, every sample as a block over the keys it plays (left
to right) and the velocities it answers (bottom to top), brighter the harder it was played, the
part of a layer that blends with the one below shaded, a dot per take when a layer has several,
and a strip of keys underneath with the recorded notes in amber; and knobs for Dynamics (Velocity
Curve, Layer Blend), Envelope, Pitch and Output. Pressing the shelves plays the key under the
pointer at the velocity under it on the Musical Typing port, so a track that hears the keyboard
sounds.

## Capture

**Track › Capture to Pantry…** (`track.capture_multisample`) plays the track's instrument a note
at a time through the track's modules (pre-fader, offline, the way Freeze renders) and makes a new instrument track whose
Pantry plays the result. The source track is not changed.

| Setting | Default | |
|---|---|---|
| `low`, `high` | 21, 108 | the range, as MIDI notes (A0 to C8 on a piano) |
| `step` | 3 | semitones between captured notes (1 captures every key) |
| `layers` | 12 | evenly spaced velocities up to 127, 16 at most |
| `velocities` | | the velocities to play instead, as a list |
| `robins` | 1 | strikes of each note and velocity, for instruments that vary each take |
| `hold` | 8 | seconds the lowest note is held |
| `hold_high` | 3 | seconds the highest is held; the notes between shorten evenly |
| `tail` | 4 | seconds left to ring after each release, before the next strike |
| `silence` | -72 | the level (dBFS) a note has died away at |
| `mono` | false | keeps one channel (half the memory) |
| `name` | the track's, with "Pantry" | the new track's name |

From the menu the notes render a slice per frame behind a progress window that names the note
being played, with Cancel (or Esc), which leaves the session untouched; `engine.execute` runs the
capture at once.

Each recorded note is one render: half a second of silence (the chain's own noise is measured
there, so a hissing amp still ends its notes), then its strikes one after another. A strike is
cut from its note-on to where it last rose above the silence level, plus a short fade. Notes that
made no sound are left out and counted (`silent`); notes still sounding when the next strike
began are counted too (`ringing`: give them a longer `tail`). Levels are kept as rendered, so the
layers keep their balance.

## Loading an SFZ file

`mix.multisample_load` reads an SFZ file's mapping into the track's Pantry: the `sample`,
`pitch_keycenter` (or `key`), `hivel` and `seq_position` of each key-down region, with
`default_path` and the values inherited from `<global>`, `<master>` and `<group>`. A second
region for a place already taken becomes that layer's next take. Nothing else of the file is read
(no envelopes, filters, key ranges, release samples, `#include` or `#define`): Pantry's own
controls shape the sound. `tools/sampling/sampling.py split` writes files it reads whole.

## Not built yet

- A menu entry for loading an SFZ file outside Pantry's window, and its browser thumbnail.
- Editing on the shelves: moving, replacing or removing a single sample, per-layer levels.
- Capturing only down to a chosen module (Freeze's "up to here").
- Third-party instruments as capture sources are untested: capture renders through instances of
  its own, as an engine-side Freeze does.
- Playing from disk: every sample is held in memory.
- Decent Sampler (`.dspreset`) loading, release samples, pedal-down samples, loops for sustained
  instruments.
