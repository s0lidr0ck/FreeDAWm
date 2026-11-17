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

- **Zones.** Every sample is a zone: the keys and velocities it answers, the note it was recorded
  at, its level and tuning, where in the file it starts and ends, an optional loop (with a
  crossfade), and what sets it off: a key going down, a key coming up (release samples), or the
  sustain pedal going down or up (pedal noises).
- **Parts.** A zone belongs to one of eight parts (a piano's notes, its release samples, its
  pedal noises, a pad under it), each with its own level knob; turned all the way down, a part
  is off. Zones of different parts that answer a note sound together. Within a part, zones in a
  sequence (round-robins) take turns.
- **Samples** are session sources (`Insert::samples`), played from disk (see below).
  A library's zone is keyed by its whole description (`ZoneSpec::key`). A captured sample has a
  shorter key, `n{note}.v{velocity}.r{robin}` (the note it was recorded at, the velocity it was
  played with, its take 1–4), and the instrument works its zone out: a key plays the nearest
  recorded note (the lower of two equally near), a layer covers the velocities from just above
  the layer below it up to its own, and a note holds up to 16 layers.
- **Velocity.** A zone plays as recorded at its reference velocity (a captured sample's own; 127
  for a library's) and **Velocity Curve** turns it down below that (the level falls as the
  velocity ratio to that power; 0 plays every hit as recorded), as far as the zone tracks
  velocity. **Layer Blend** mixes in the layer just below, of the same part, over the bottom of
  each layer's range, at equal power: at 100% every hit between two layers is a mix of both, at
  0% the layers switch.
- **Notes** have an amp ADSR. The sustain pedal holds released notes; pitch bend is ±2
  semitones. 128 voices (a blended hit, or a note of two parts, uses two).
- Level, Pan, Transpose and Fine Tune.

## The window

Pantry opens in its own window from the track's instrument slot: the wordmark and what is on the
shelves (samples, notes, layers, seconds, megabytes), **Load…**, **Save…** and **Clear**
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
| `release` | false (on in the dialog) | also captures what sounds when a key comes up |
| `pedal` | false (on in the dialog) | also captures the sustain pedal's own noises |
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

With `release`, each note is also struck briefly (a soft and a hard strike when there are layers
to tell apart) and what sounds from the moment its key comes up is kept as a release sample: a
zone of a **Release** part that plays when a note is let go, over that note's keys. With `pedal`,
one more render presses and lifts the sustain pedal with no key down; whatever the instrument
sounds each way is kept in a **Pedal** part. An instrument that makes neither sound gets
neither part. The notes are then the **Notes** part, and each part has its level knob: a release
sample plays over the note's own fading tail, so its level is the one to set by ear.

## Where an instrument is kept

A Pantry's samples are sources of its session: captured ones are written to the session's
`Audio Files/` when it is saved, named for the track, the note and the velocity
(`Piano Pantry D#1 v95.wav`, on disk `Piano Pantry D_1 v95.wav`), and the mapping is in the
session file. To keep an instrument for other sessions, save it to the Library.

**Save…** (`mix.multisample_save`) writes the instrument to the Library's `Pantry` folder: an
`.sfz` file of its name, and beside it a folder of that name with every sample as a 32-bit float
WAV (`039_Ds1_v095_rr1.wav`: the note's number and name, the velocity, the take). The file maps
each sample over its keys and velocities, so sfizz, Sforzando and other SFZ players play it; a
line of it carries Pantry's own settings and each region the velocity its sample was played at
(`pantry_velocity`), so **Load…** brings the instrument back exactly as it was.

**Load…** lists the Library's instruments, takes any SFZ file, and imports a folder of captured
samples (`mix.multisample_import`). Import needs no mapping file: each sample goes where its name
says, read as capture names them (the files of a saved session's `Audio Files/` work as they are,
a trailing number and all), as Save writes them, or as `tools/sampling/sampling.py` does. It adds
to what the Pantry holds; files whose names give no note and velocity are left out and counted.
Sample files and folders dropped on Pantry's window, or on its track, import the same way.

## Loading other libraries

`mix.multisample_load` (**Load…**, or the file dropped on Pantry) reads an instrument made for
another sampler into the track's Pantry, in place of what it holds. Each of its samples becomes
a zone as its file describes it: keys, velocities, the note it was recorded at, level, tuning,
start and end, loop, round-robin turn, and whether a key going down, a key coming up or the
sustain pedal sets it off. Its groups become parts. Since a library's levels follow the velocity
from full scale and its layers switch as it was made to, a load sets Velocity Curve to 2 and
Layer Blend to 0; raise Layer Blend to mix its layers. Not read: interfaces, effects, filters,
envelopes, and anything driven by a controller other than the sustain pedal.

- **SFZ** (`.sfz`): `sample`, `lokey`/`hikey`/`key`, `pitch_keycenter`, `lovel`/`hivel`,
  `seq_position`, `volume`, `tune`, `transpose`, `offset`, `end`, `loop_mode` with
  `loop_start`/`loop_end`, `pitch_keytrack`, `amp_veltrack`, `trigger` (attack, release) and
  `on_locc64`/`on_hicc64` (the pedal), with `default_path`, `<global>`, `<master>` and `<group>`
  inherited, `#include` (anywhere on a line, eight deep) and `#define`d variables. A part is a
  `master_label`, else Notes, Release or Pedal by what sets the zone off. Of an instrument with
  keyswitched versions (`sw_last`), the one it starts on (`sw_default`, else the first) is
  loaded. Checked against the Salamander Grand Piano V3 file.
- **Decent Sampler preset** (`.dspreset`): each `<sample>`'s `path`, `rootNote`,
  `loNote`/`hiNote`, `loVel`/`hiVel`, `seqPosition`, `start`, `end`, `loopEnabled` with
  `loopStart`/`loopEnd`/`loopCrossfade`, `pitchKeyTrack`, `ampVelTrack`, `trigger` and
  `onLoCC64`/`onHiCC64`, a value it lacks taken from its `<group>`, then from `<groups>`;
  `volume` and `tuning` add up down that chain. A part is a group's first tag (else its name),
  and starts at the level of the control bound to that tag's volume (so a preset's Notes, RT,
  Pedal and Texture faders arrive as Pantry's part knobs, a part at zero switched off). The file
  is read leniently, as Decent Sampler does (an attribute given twice keeps its first value).
- **Decent Sampler library** (`.dslibrary`, a zip archive): one of its presets (`preset`; its
  first by name otherwise), the samples read out of the archive. They have no files of their own
  afterwards, so they are written into the session's `Audio Files/`, or with **Save…** to the
  Library.

**Save…** keeps all of it: a library's zones are written out in full, with their parts.

Known gap: per-group envelopes are not read (a pad's slow attack and release come from Pantry's
one envelope).

## Playing from disk

Pantry does not hold its samples in memory. A sample plays from its file through the session's
disk cache (`soundcraft_model::disk`, the one audio clips play through): the first 16,384 frames
of each (a third of a second at 48 kHz, `spill::SAMPLER_HEAD`) are kept in memory so a note
starts at once, a voice asks for what it will play about a second before it gets there, and the
disk thread reads it in. What has been played stays cached while the cache has room (a quarter
of the computer's memory by default, shared with the session's audio, `FREEDAWM_DISK_CACHE_MB`
to set it), so notes played before play again from memory. The window's top line gives the
instrument's size and how much of it is in memory.

- A library's WAV file at the session's sample rate is played from where it is: loading reads
  only its header and its waveform, and the session names the file, so the library must stay
  where it was.
- Any other sample (FLAC or another compressed format, a WAV at another rate, one out of a
  `.dslibrary`, a captured note) is decoded, brought to the session's rate and written as a
  32-bit float WAV to play from: into the project's `Audio Files/`, or for a session not saved
  yet its autosave folder (copied into the project at the first save). A load writes each as it
  goes, so a library larger than memory loads; a load that fails removes what it wrote.
- Samples under 65,536 values (a pedal noise, a short release) stay in memory.
- An offline render (bounce, freeze, capture) waits for the disk and never drops a note. Live,
  a note that outruns the disk (a slow or sleeping drive) plays silence until its audio
  arrives, and counts as a disk underrun.
- With `FREEDAWM_STREAM=0`, or in a headless engine with nowhere to write, samples are held in
  memory as before.

A capture is still held in memory until it finishes, then written out (about 1.4 GB for a piano
every minor third at 12 layers).

## Not built yet

- Writing a capture to disk as it goes (it is held in memory until it finishes).
- An envelope per part (a pad's slow attack and release); choosing a library's preset in the
  Load dialog.
- Editing on the shelves: moving, replacing or removing a single sample, a zone's level.
- Menu entries for loading and saving outside Pantry's window.
- Capturing only down to a chosen module (Freeze's "up to here").
- Third-party instruments as capture sources are untested: capture renders through instances of
  its own, as an engine-side Freeze does.
