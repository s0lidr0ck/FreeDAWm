# Making a wavetable from your own audio

The Wavetable Synth (Cicada) plays a table of single-cycle waves; the Position control (and the
envelope, LFO, velocity and mod wheel that move it) sweeps through them. Besides its thirteen
built-in tables it can play a table made from any audio file of yours: a sung note, a field
recording, a drum loop, or a wavetable file written by another synth.

## Importing

Select the synth's track and choose **Track › Import Wavetable...**, or drop an audio file on the
wave display in the synth's window (double-click or right-click the wave to open the window
without a file). The **Import Wavetable** window opens with the file (or **Choose File...** to
pick one; dropping a file on the window replaces it). Choose how the file is cut into waves:

- **Fixed frames.** The file is already a row of single-cycle frames, as wavetable files are
  (2048 samples a frame by convention). The frame size comes from the file's `clm ` chunk when it
  has one, else it is guessed from the length; choose 256 to 8192 if the waves look wrong. A file
  of more than 32 frames gives 32 spread evenly through it. A file shorter than one frame (a
  single-cycle wave of any length) becomes one wave.
- **Pitch-tracked.** For a played, sung or hummed note: the pitch is tracked through the file
  (the Vocal Editor's tracker) and one cycle is cut at each of up to 32 points spread through the
  pitched part, at the pitch found there. Every wave is turned to start at the same phase, so
  neighbouring waves morph without a jump. Needs a pitch between 70 Hz and 1 kHz.
- **Spectral.** For anything: noise, speech, drums, chords. At up to 32 points the spectrum is
  measured and one clean cycle is made from its loudness at each harmonic: of the recording's own
  pitch when it has a steady one, else one harmonic per band of the spectrum (about 23 Hz apart
  at 48 kHz). All waves share the same phases, so they morph smoothly.

The window shows the waves the import would make as a stack; step through them with the arrows.
Choose a name (the file's name by default), then **Import**. That is one undo step.

## What is kept

The table itself (up to 32 waves of 2048 samples) is saved in the session with the synth, not a
path to the file: the session plays the same after the file is moved or deleted. While an
imported table is loaded the synth plays it instead of the built-in table (the Table control has
no effect); **Clear Table** goes back to the built-in tables.

Every wave is band-limited the same way as the built-in tables (18 levels half an octave apart),
so high notes do not alias. The table as a whole is scaled to full level; the relative level of
its waves is kept.

The top of the window says what the synth plays now and has **Load Table...**, **Save
Table...** and **Clear Table**. **Save Table...** writes the table as a plain 32-bit float WAV of 32 frames × 2048 samples with
a `clm ` chunk, which other wavetable synths read as a table; **Load Table...** reads one back.

## Limits

- Files up to 256 MB and 10 minutes long; only the first minute is read (a note says so).
- At most 32 waves.
- Stereo files are mixed to mono.
- A silent file (or one that is only a constant level) is refused.

## Commands

`mix.wavetable_import`, `mix.wavetable_preview`, `mix.wavetable_clear`, `mix.wavetable_save`,
`mix.wavetable_load`, `mix.wavetable_info`: each takes `track` (else the first selected track
playing the synth); the command list (`engine.commands` on the control channel) gives their
parameters.

## How it was checked

Nothing here could be listened to; it was checked by measurement. A known harmonic series comes
back as that series in every mode; a sound that brightens gives waves that brighten; imported
tables measured -114 to -124 dB of aliasing on a 2.6 kHz note, by the same measure as the
built-in tables; a swap while a note sounds steps no further between samples than the steady
sound does; a session without an imported table renders as before.
