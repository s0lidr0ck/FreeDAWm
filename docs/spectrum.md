# Spectrum

The Spectrum editor draws the live frequency spectrum of your tracks in one shared graph, each
track's curve in its own colour with its name on it, so you can see which tracks crowd the same
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
reads it. Aux inputs (busses) and routing folders can be switched on in the list.

- Across: 20 Hz to 20 kHz on a log scale. Up: level in dB per third of an octave (each point is
  the average power of the bins within a third of an octave around it), 0 dB at the top.
- Curves rise quickly and fall at about 30 dB a second, so a kick drawn on one beat is still there,
  lower, at the next, and the curves read as shapes rather than flicker.
- It works whenever the mixer runs: playing, and stopped while a track monitors its input. With
  the transport stopped and nothing monitored the curves fall away.

**Hover** a curve, or a track's row in the list, and that curve is drawn on top with a wash under
it, the others dimmed, and its name shown. Under the pointer the readout gives the frequency and
level there ("Bass · 80 Hz · -42.0 dB").

## Showing and hiding

The list on the left has a row per track (and the master): its line swatch, its name and a
switch. Sources (audio, MIDI and instrument tracks) are shown at first; busses and the master are
not.

- **Show all** switches every row on; **Hide all** switches every row off.
- **Only selected tracks** shows just the tracks selected in the Edit window and follows the
  selection as it changes. While it is on the switches are greyed out; turning it off brings back
  what they say.
- At most **24 curves** are drawn at once. Past that the graph stops being readable; the list says
  "Showing the first 24 of N", Show all switches on the first 24, and a further switch is greyed
  with the reason until another is hidden.

What is shown (the switches, Only selected tracks) is kept with the session; sessions saved before
the editor existed open with the defaults above. Whether the tab is open is kept with your other
window settings, as the Vocal and MIDI editors' are. None of it is an undo step.

**Same colours.** Two tracks of the same (or nearly the same) colour are still told apart: the
first is drawn solid, the next dashed, the next dotted (a fourth solid again, a little lighter), and
the list's swatch shows the same line. Your track colours are not changed.

## Stereo scope

**Stereo scope** in the list puts a vectorscope beside the graph, drawn the way Sprawl's is: up is
mid, across is side, so a mono track is a vertical line, a wide one a cloud, and one whose sides
cancel (out of phase) a horizontal line. Every shown track is drawn in it, about 40 ms of its sound
as dots in the colour of its curve (solid lines as filled dots, dashed as small crosses, dotted as
rings), all on one zoom so their widths and levels compare. It uses the same list: a hidden track
is not in it. Point at a track's dots (or its curve, or its row) and it comes to the front with
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
- The window analyses at most eight curves a frame, taking turns, and adds about 1 to 1.7 ms a
  frame with 8 to 24 curves shown.
- The stereo scope, shown: its taps carry left and right only while it is, about 20 to 40 µs more
  per block on the audio thread with 8 to 40 tracks shown, and 0.3 to 1.8 ms more per UI frame.

For agents and scripts the commands are `window.spectrum`, `spectrum.show`
(`{track | tracks | master: true, shown?}`), `spectrum.show_all`, `spectrum.hide_all`,
`spectrum.only_selected` (`{on?}`) and `spectrum.scope` (`{on?}`).
