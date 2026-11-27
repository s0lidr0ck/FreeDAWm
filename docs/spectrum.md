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
reads it. Aux inputs and routing folders (buses) have their own section in the list.

The curves are read exactly as CleanQ's analyser reads its own, with the same code, so a sound
looks the same in both:

- Across: 20 Hz to 20 kHz on a log scale. Up: -96 to 0 dBFS (a full-scale sine reads 0), with no
  tilt or weighting: what is drawn in each pixel column is the loudest FFT bin in it.
- About 30 times a second a 4096-point transform of the newest audio; each bin keeps 70% of its
  last value and takes 30% of the new one; then a hold that rises at once and falls 35% of the way
  back every drawn frame.
- It works whenever the mixer runs: playing, and stopped while a track monitors its input. With
  the transport stopped and nothing monitored the curves fall away.

The names of the loudest few curves are written at their peaks, never over one another; the list
names the rest. **Hover** a curve, a track's row or its dots in the scope, and that curve is drawn
on top with a wash under it and its name, the others dimmed. The readout in the toolbar (on the
plot, when the dock is narrow) gives the frequency and level under the pointer, unweighted
("Bass   80 Hz   -42.0 dBFS").

## Showing and hiding

The list on the left is in three sections, **Tracks** (audio, MIDI and instrument tracks),
**Buses** (Aux inputs and routing folders) and **Master**, each with a header saying how many of
it are shown and a Show / Hide that switches the whole section. Each row has the curve's colour
swatch, the track's name and its switch; click the row. Tracks are shown at first; buses and the
master are not (a bus carries the tracks fed into it, so showing both at first would draw much of
the mix twice), and their sections are one click away.

- **Show all** and **Hide all** in the toolbar switch every row.
- **Only selected tracks** shows just the tracks selected in the Edit window and follows the
  selection as it changes. While it is on the switches are greyed out; turning it off brings back
  what they say.
- At most **24 curves** are drawn at once. Past that the graph stops being readable; the list says
  "Showing the first 24 of N", Show all switches on the first 24, and a further switch is greyed
  with the reason until another is hidden.

What is shown (the switches, Only selected tracks) is kept with the session; sessions saved before
the editor existed open with the defaults above. Whether the tab is open is kept with your other
window settings, as the Vocal and MIDI editors' are. None of it is an undo step.

**Same colours.** Tracks of the same (or nearly the same) colour are drawn in shades of that
colour, from deep to pale, each a clear step from the next and every one standing out from the
graph's dark ground (whatever the app's theme: the graph is dark in both). The track whose own
colour is nearest a shade keeps that one. The list's swatch and the scope's dots use the same
shades. Four tracks of one colour are told apart by shade alone; a fifth to eighth are drawn
dashed and any more dotted, reusing the shades. A colour too dark to see on the graph is drawn a
little lighter. Your track colours are never changed.

## Stereo scope

**Stereo scope** in the toolbar puts a vectorscope beside the graph, drawn the way Sprawl's is: up is
mid, across is side, so a mono track is a vertical line, a wide one a cloud, and one whose sides
cancel (out of phase) a horizontal line. Every shown track is drawn in it, about 40 ms of its sound
as dots in the colour (and shade) of its curve, all on one zoom so their widths and levels
compare. It uses the same list: a hidden track
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
- The window analyses at most twelve curves a frame, taking turns (at 60 frames a second, 24
  curves each still get CleanQ's 30 analyses a second), and adds about 1.1 to 2.6 ms a frame with
  8 to 24 curves shown.
- The stereo scope, shown: its taps carry left and right only while it is drawn, about 20 to 40 µs
  more per block on the audio thread with 8 to 40 tracks shown, and 0.2 to 1.2 ms more per UI
  frame. In a dock too small to fit it beside the graph it is not drawn and costs nothing; its
  button says why.

For agents and scripts the commands are `window.spectrum`, `spectrum.show`
(`{track | tracks | master: true | section: "tracks" | "buses" | "master", shown?}`), `spectrum.show_all`, `spectrum.hide_all`,
`spectrum.only_selected` (`{on?}`) and `spectrum.scope` (`{on?}`).
