# Sampling an instrument through effects

`sampling.py` makes a multisampled instrument out of any instrument and effects chain: it writes a
MIDI file that plays each note alone, and cuts the audio rendered from it into one file per note.
It needs nothing but Python.

## 1. Make the MIDI file

```sh
python3 -I tools/sampling/sampling.py midi target/sampling/piano
```

This writes `sampling.mid` and `plan.json` (what the file plays, and when; the splitter needs it).
The defaults suit a piano: A0 to C8 every minor third, six velocities, each note held 8 seconds
and left to ring for 4 more, 36 minutes in all.

| Option | Default | |
|---|---|---|
| `--low`, `--high` | `A0`, `C8` | range, as note names or MIDI numbers |
| `--step` | `3` | semitones between sampled notes |
| `--vels` | `24,48,72,96,112,127` | velocity layers |
| `--layers` | | this many evenly spaced velocities up to 127 (16: 8, 16, … 127), instead of `--vels` |
| `--robins` | `1` | strikes per note, for instruments that vary each take |
| `--hold` | `8` | seconds each note is held |
| `--hold-high` | as `--hold` | seconds the highest note is held; the notes between shorten evenly |
| `--tail` | `4` | seconds left to ring after the release |
| `--bpm` | `120` | the file's tempo |

Give `--tail` room for the longest effect in the chain (a long reverb or delay needs more).

## 2. Render it

1. Import `sampling.mid` at the start of a session, with its tempo map (the default). The session
   must play it at the file's tempo.
2. Put the instrument on the track, then the effects.
3. Export the whole clip, from its start to its end, as a WAV (16-, 24- or 32-bit, or 32-bit
   float). The file starts with 2 seconds of silence: keep them, the splitter measures the chain's
   noise there.

Leave headroom: a render that reaches full scale gets a warning.

## 3. Split the render

```sh
python3 -I tools/sampling/sampling.py split target/sampling/piano/plan.json render.wav target/sampling/amp-piano
```

The splitter lines the render up by its first note (so an effect's delay doesn't matter), checks
that the last note is where the plan puts it, cuts each note from its start to where it falls
under the chain's noise (or `--silence`, -72 dBFS), and fades the end. It writes one WAV per note
(`060_C4_v096_rr1.wav`), in the render's own format and at its own level, and an `.sfz` file that
maps them across the keys and velocities; sfizz or Sforzando plays it as is.

It warns when notes were silent, still sounding when the next began, or clipped.
