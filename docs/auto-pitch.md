# Auto Pitch

Auto Pitch is FreeDAWm's real-time pitch corrector: an insert effect that pulls a voice onto the
notes of a key as it plays. Put it on a recorded vocal track like any other effect (Mix window,
an insert slot, Auto Pitch - Pitch Correction).

It works on **one voice at a time** (a lead, a single backing part). On two voices at once it
hears no clear pitch and lets the sound through untouched; put it on each voice's own track. A
stereo vocal is corrected as one voice, both sides moved alike, so its stereo image holds.

## Studio and Live

Auto Pitch has two modes, switched at the top right of its editor:

- **Studio** (the default) uses the Vocal Editor's own pitch shifter, which keeps the voice's
  character and each sung period's shape. It delays the voice by about 43 ms; the mixer lines
  every other track up with that delay, so playback and bounces stay in time. The cleanest sound,
  for recorded tracks.
- **Live** is for singing through it (monitoring, live streaming). It delays the voice by only a
  few milliseconds: it plays the voice back a touch faster or slower and, now and then, skips or
  repeats exactly one sung cycle. Its delay depends on **Voice range** (the lowest note it listens
  for): about 8 ms for Low (from 70 Hz), 6 ms for Middle (from 100 Hz, the default) and 4 ms for
  High (from 150 Hz); on average it is less. What it costs: at Hard tune about half of the vibrato
  survives (it hears the pitch a little late), the vowel colour moves slightly with the correction,
  and Formant does not apply.

With **Low Latency Monitoring** on, every effect that delays the sound is taken out of an
armed or monitored track, but Auto Pitch in Live mode stays in: you hear yourself corrected, later
by its few milliseconds plus your audio interface's own round trip (typically 5 to 10 ms at a
128-sample buffer). In Studio mode it is taken out there like any other delaying effect. (The rule:
a module made to be heard live stays in while its delay is at most 15 ms.)

## The controls

| Control | What it does |
|---|---|
| **Natural / Hard tune** | The character in one switch. Natural: gentle, keeps the vibrato, long notes corrected more gently still. Hard tune: every note snaps at once and the vibrato is flattened (the obvious, stepped sound). It sets Retune speed, Humanize, Keep vibrato and Note hold; moving any of those afterwards shows **Custom**. It can be automated, to switch character within a song. |
| **Key** | **Follow the song**: the Key ruler's key at each moment, including key changes later in the song (before the first change, the song's key at bar 1). Or a key of its own. |
| **Scale** | With a key of its own: the scale (major, the minors, the modes, the pentatonics, blues, chromatic). Picking one fills the twelve notes. |
| **The twelve notes** | Each on or off. A voice near a note that is off goes to the nearest note that is on. Switching one makes the scale Custom. |
| **Retune speed** | How fast a note is pulled onto the key: Instant to 200 ms. |
| **Humanize** | Notes held longer than half a second are corrected more gently. |
| **Keep vibrato** | On: the note's centre is corrected and the vibrato kept. Off: the vibrato is flattened too. |
| **Note hold** | How much closer (in cents) the voice must get to another note before the target changes. Too low and a wide vibrato flips between two neighbouring notes (a warble); the defaults do not flip on a wide vibrato. |
| **Formant** | Keep (the voice's character as sung), or shift it up or down in semitones (Studio mode). |
| **Mix** | The corrected voice against the original (both delayed alike; 0 % is the original exactly). |
| **Studio / Live** | The mode (above). Switching it changes the delay; the mixer re-aligns the other tracks with a short crossfade. |
| **Voice range** | Live mode only: Low, Middle or High (see above). |
| **Bypass** | Takes Auto Pitch out (the voice still comes out delayed alike, so nothing moves in time). |

Presets: **Gentle** (the defaults: Natural), **Medium** (firmer, vibrato kept) and **Hard**
(Hard tune). Presets never change the key, the scale, the notes, the mode or the voice range.

## The display

The scrolling display shows the sung pitch (grey) and the corrected pitch (in colour) against the
lanes of the notes in use (the key's tonic highlighted), with the note being aimed at and how far
off it was sung, in cents.

## What it cannot do

- **See ahead.** It corrects a note once it has heard some of it, so a slide or a run is
  corrected after it starts. Fast runs and melismas are left much as sung at Natural; Hard tune
  keeps up with them.
- **Blue notes and bends** outside the scale are pulled into it. Switch those notes on, use a
  slower setting, or tune that passage in the Vocal Editor, which sees the whole take.
- **Harmonies or chords** (one voice only), and target notes played on a keyboard (not yet).

For transparent, note-by-note tuning use the Vocal Editor; Auto Pitch is for speed and for the
hard-tune sound.
