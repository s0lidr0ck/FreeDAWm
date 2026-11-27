# Spectrum

The Spectrum editor draws the live frequency spectrum of your tracks in one shared graph, each
track's curve in its own colour, so you can see which tracks crowd the same
frequency range: a kick and a bass fighting over 60 to 100 Hz, two guitars stacked in the
low mids. Open it with **Window › Editors › Spectrum** (also in Search), or the **SPECTRUM** tab of
the lower dock's strip. It is a tab of the lower dock beside the Vocal and MIDI editors, and can
be moved, floated or closed like them.

"Spectrum" is a working name.

## What it shows

Each curve is what the track actually sends into the mix: after its plug-ins, its fader, its
mute (a muted or solo-muted track reads silence) and its pan. Stereo tracks are summed to one
curve; the pan only changes a curve by the pan law's level change, not by which side it leans to.
The master can be shown too: that is the main mix after the master faders, as the output meter
reads it. Aux inputs and routing folders (buses) have their own group of chips.

The curves keep CleanQ's analyser's pace, dB range and fall-off (CleanQ itself is unchanged):

- Across: 20 Hz to 20 kHz on a log scale. Up: -96 to 0 dBFS (a full-scale sine reads 0).
- About 30 times a second a 4096-point transform of the newest audio; each bin keeps 70% of its
  last value and takes 30% of the new one; then a hold that rises at once and falls 35% of the way
  back every drawn frame.
- Then, unlike CleanQ's picture, each point of a curve is the average power over a third of an
  octave around it, taken over the spectrum drawn as straight lines between its bins, so the
  curves are clean shapes, with no steps in the lows. (A sixth of an octave was tried with nine
  curves over each other and read as busier.)
- **Tilt** in the toolbar (off at first, kept with the session) raises the drawn curves 4.5 dB an
  octave about 1 kHz (-9 dB at 250 Hz, +9 dB at 4 kHz), so a mix reads level instead of sloping
  down to the highs. Only the drawing moves: the reading under the pointer stays the true level
  and says "(untilted)" while Tilt is on.
- It works whenever the mixer runs: playing, and stopped while a track monitors its input. With
  the transport stopped and nothing monitored the curves fall away.

Every shown track is a soft filled area under a thin, slightly see-through line in its colour.
**Point at** a curve, or at its chip, and that track brightens; the tracks that overlap it keep
their colours and are named, on the curves and in words at the top left of the graph ("Vox
overlaps Keys, Pad", or "Vox: nothing in its way"); every other track fades to a quiet grey. The
readout at the right of the buttons (on the plot, when the dock is narrow) gives the frequency and
level under the pointer, unweighted ("Bass   80 Hz   -42.0 dBFS").

**What counts as overlapping** comes from the curves as drawn, only for the track pointed at: of
the part of its curve within 30 dB of its own peak, the share another track's curve also reaches
(where the other is as loud or louder it covers that part fully; where it is quieter, only up to
its own height). A track that covers at least **40%** is named, most first. The master is never
named (it holds every track). The 40% was set on the demo session: there the pad and keys name
each other, the bass names the keys that sit over its upper harmonics, and the hi-hats, alone up
top, name nothing.

## Showing and hiding

The tracks are chips in a row along the top of the editor (a coloured dot and the name), so the
graph has the full width. Click a chip to show or hide its track; point at it to see its track as
if you pointed at its curve. The chips are grouped **Tracks** (audio, MIDI and instrument tracks),
**Buses** (Aux inputs and routing folders) and **Master**, each group with a Show / Hide that
switches the whole group. The chips wrap onto up to three rows; when there are more tracks than
that holds, they sit in one row that scrolls sideways. Tracks are shown at first; buses and the
master are not (a bus carries the tracks fed into it, so showing both at first would draw much of
the mix twice), and their groups are one click away.

- **Show all** and **Hide all** switch every chip.
- **Only selected tracks** shows just the tracks selected in the Edit window and follows the
  selection as it changes. While it is on the switches are greyed out; turning it off brings back
  what they say.
- At most **24 curves** are drawn at once. Past that the graph stops being readable; the bar says
  "Showing the first 24 of N", Show all switches on the first 24, and a further chip is greyed
  with the reason until another is hidden.

What is shown (the switches, Only selected tracks) is kept with the session; sessions saved before
the editor existed open with the defaults above. Whether the tab is open is kept with your other
window settings, as the Vocal and MIDI editors' are. None of it is an undo step.

**Same colours.** Tracks of the same (or nearly the same) colour are drawn in shades of that
colour, from deep to pale, each a clear step from the next and every one standing out from the
graph's dark ground (whatever the app's theme: the graph is dark in both). The track whose own
colour is nearest a shade keeps that one. The chip's dot and the scope's dots use the same
shades. Four tracks of one colour are told apart by shade alone; a fifth to eighth are drawn
dashed and any more dotted, reusing the shades. A colour too dark to see on the graph is drawn a
little lighter. Your track colours are never changed.

## Stereo scope

**Stereo scope** in the toolbar puts a vectorscope beside the graph, drawn the way Sprawl's is: up is
mid, across is side, so a mono track is a vertical line, a wide one a cloud, and one whose sides
cancel (out of phase) a horizontal line. Every shown track is drawn in it, about 40 ms of its sound
as dots in the colour (and shade) of its curve, all on one zoom so their widths and levels
compare. It uses the same chips: a hidden track
is not in it. It takes the same tones as the graph: point at a track's dots (or its curve, or its
chip) and it comes to the front, the tracks that overlap it stay in colour, the rest go grey, with
its correlation underneath: +1 mono, 0 unrelated sides, below 0 sides that cancel in mono.

It shows each track's left and right as they reach the mix, after the pan (a mono track panned
left sits on the L diagonal). On a surround main mix it shows the front left and right. Whether
it is shown is kept with the session.

## Cost

The audio thread only copies samples; the analysis is done by the window.

- Editor closed: one check per audio block, nothing else. Hidden curves have no tap at all.
- Open, measured on a 40-track session at 48 kHz and a 512-sample block (10.7 ms of audio): about
  60 to 150 µs more per block with 8 to 40 curves shown (at most 1.5 % of the block), within the
  run-to-run noise at 8 and 12.
- The window analyses at most twelve curves a frame, taking turns (at 60 frames a second, 24
  curves each still get CleanQ's 30 analyses a second), and, with the smoothing and the filled areas, adds about 1.2 to 4 ms a frame with
  8 to 24 curves shown.
- The stereo scope, shown: its taps carry left and right only while it is drawn, about 20 to 40 µs
  more per block on the audio thread with 8 to 40 tracks shown, and 0.5 to 1.2 ms more per UI
  frame. In a dock too small to fit it beside the graph it is not drawn and costs nothing; its
  button says why.

For agents and scripts the commands are `window.spectrum`, `spectrum.show`
(`{track | tracks | master: true | section: "tracks" | "buses" | "master", shown?}`), `spectrum.show_all`, `spectrum.hide_all`,
`spectrum.only_selected`, `spectrum.scope` and `spectrum.tilt` (each `{on?}`).
